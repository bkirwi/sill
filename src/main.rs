use std::borrow::Cow;
use std::cmp::Ordering;

use std::collections::VecDeque;
use std::fmt::Display;
use std::fs::File;
use std::io::ErrorKind;

use std::path::PathBuf;
use std::{env, fs, io};

use armrest::app;
use armrest::app::{Applet, Component};

use armrest::ink::Ink;
use armrest::ui::canvas::Fragment;
use armrest::ui::{Side, Text, TextFragment, View, Widget};
use clap::Arg;
use libremarkable::framebuffer::cgmath::Vector2;
use libremarkable::framebuffer::common::{DISPLAYHEIGHT, DISPLAYWIDTH};
use once_cell::sync::Lazy;
use rusttype::Scale;

use xdg::BaseDirectories;

use font::*;
use grid_ui::*;
use hwr::*;
use text_buffer::*;

mod font;
mod grid_ui;
mod hwr;
mod text_buffer;

static BASE_DIRS: Lazy<BaseDirectories> =
    Lazy::new(|| BaseDirectories::with_prefix("armrest-editor").unwrap());

const SCREEN_HEIGHT: i32 = DISPLAYHEIGHT as i32;
const SCREEN_WIDTH: i32 = DISPLAYWIDTH as i32;
const TOP_MARGIN: i32 = 100;
const LEFT_MARGIN: i32 = 100;

const TEMPLATE_FILE: &str = "templates.json";

const HELP_TEXT: &str = "Welcome to armrest-edit!

It's a nice editor.
";

#[derive(Clone)]
pub enum Msg {
    SwitchTab { tab: Tab },
    Write { row: usize, ink: Ink },
    Erase { row: usize, ink: Ink },
    Swipe { towards: Side },
    Save,
    Open { path: PathBuf },
    Rename,
    New,
}

#[derive(Hash, Clone)]
struct EditChar {
    value: char,
    rendered: Option<TextFragment>,
}

// TODO: split out the margin widths.
#[derive(Hash, Clone)]
pub struct Metrics {
    height: i32,
    width: i32,
    baseline: i32,
    rows: usize,
    cols: usize,
}

impl Metrics {
    fn new(height: i32) -> Metrics {
        let scale = Scale::uniform(height as f32);
        let v_metrics = FONT.v_metrics(scale);
        let h_metrics = FONT.glyph(' ').scaled(scale).h_metrics();
        let width = h_metrics.advance_width.ceil() as i32;

        let rows = (SCREEN_HEIGHT - TOP_MARGIN * 2) / height;
        let cols = (SCREEN_WIDTH - LEFT_MARGIN * 2) / width;

        Metrics {
            height,
            width,
            baseline: v_metrics.ascent as i32 + 1,
            rows: rows as usize,
            cols: cols as usize,
        }
    }
}

#[derive(Clone)]
pub enum Tab {
    Meta {
        path_buffer: TextBuffer,
        suggested: Vec<PathBuf>,
    },
    Edit,
    Template,
}

type Coord = (usize, usize);

struct TextWindow {
    buffer: TextBuffer,
    insert: Option<(Coord, TextBuffer)>,
    dimensions: Coord,
    origin: Coord,
}

impl TextWindow {
    fn new(buffer: TextBuffer, dimensions: Coord) -> TextWindow {
        TextWindow {
            buffer,
            insert: None,
            dimensions,
            origin: (0, 0),
        }
    }

    fn page_relative(&mut self, (row_d, col_d): (isize, isize)) {
        let (row, col) = &mut self.origin;
        *row = (*row as isize + row_d * self.dimensions.0 as isize).max(0) as usize;
        *col = (*col as isize + col_d * self.dimensions.1 as isize).max(0) as usize;
    }

    fn insert_coords(&self, (row, col): Coord) -> Option<Coord> {
        self.insert
            .as_ref()
            .and_then(|((r, c), _)| match row.cmp(r) {
                Ordering::Less => None,
                Ordering::Equal => {
                    if col < *c {
                        None
                    } else {
                        Some((row - r, col - c))
                    }
                }
                Ordering::Greater => Some((row - r, col)),
            })
    }

    pub fn write(&mut self, coord: Coord, c: char) {
        if let Some(insert_coords) = self.insert_coords(coord) {
            let (_, b) = self.insert.as_mut().unwrap();
            b.write(insert_coords, c);
        } else {
            self.buffer.write(coord, c);
        }
    }

    pub fn open_insert(&mut self, coord: Coord) {
        // FIXME: off by one.
        self.buffer.pad(coord.0, coord.1);
        self.insert = Some((coord, TextBuffer::empty()));
    }

    pub fn close_insert(&mut self, coord: Coord) {
        if let Some(coord) = self.insert_coords(coord) {
            self.insert.as_mut().unwrap().1.pad(coord.0, coord.1);
        }

        if let Some(((row, col), mut buffer)) = self.insert.take() {
            self.buffer.pad(row, col);
            // Awkward little dance: split `row`, push the end of it onto the end of the insert,
            // push the beginning of the insert on the end of row, then splice the rest in.
            let insert_row = &mut self.buffer.contents[row];
            let trailer = insert_row.split_off(col);
            buffer
                .contents
                .last_mut()
                .expect("last line from a non-empty vec")
                .extend(trailer);
            let mut contents = buffer.contents.into_iter();
            insert_row.extend(contents.next().expect("first line from a non-empty vec"));
            let next_row = row + 1;
            self.buffer.contents.splice(next_row..next_row, contents);
        }
    }

    fn fragment(&self, coord: Coord, metrics: &Metrics) -> Option<TextFragment> {
        if let Some(insert_coords) = self.insert_coords(coord) {
            let (_, b) = self.insert.as_ref().unwrap();
            fragment_at(b, insert_coords, metrics)
        } else {
            fragment_at(&self.buffer, coord, metrics)
        }
    }
}

/// This stores data from a recent recognition attempt, and the number of times it was overwritten
/// within the window we maintain. Idea being, if we have to go back and rewrite a char just after
/// we wrote it, we probably guessed wrong and should use it as a template.
struct Recognition {
    coord: Coord,
    ink: Ink,
    best_char: char,
    overwrites: usize,
}

struct Editor {
    metrics: Metrics,

    error_string: String,

    // tabs
    tab: Tab,

    // template stuff
    template_path: PathBuf,
    template_offset: usize,
    templates: Vec<CharTemplates>,
    char_recognizer: CharRecognizer,

    tentative_recognitions: VecDeque<Recognition>,

    // text editor stuff
    path: Option<PathBuf>, // None if we haven't chosen a name yet.
    text: TextWindow,
    dirty: bool,
}

impl Editor {
    fn load_templates(&mut self) -> io::Result<()> {
        let data = match File::open(&self.template_path) {
            Ok(file) => serde_json::from_reader(file)?,
            Err(e) if e.kind() == ErrorKind::NotFound => TemplateFile::new(&[]),
            Err(e) => return Err(e),
        };

        self.templates = data.to_templates(self.metrics.height);
        self.char_recognizer = CharRecognizer::new(&self.templates, &self.metrics);

        Ok(())
    }

    fn save_templates(&self) -> io::Result<()> {
        let file_contents = TemplateFile::new(&self.templates);
        serde_json::to_writer(File::create(&self.template_path)?, &file_contents)?;
        Ok(())
    }

    fn left_margin(&self) -> i32 {
        LEFT_MARGIN
    }

    fn right_margin(&self) -> i32 {
        SCREEN_WIDTH - LEFT_MARGIN - self.metrics.cols as i32 * self.metrics.width
    }

    fn draw_grid(
        &self,
        view: &mut View<Msg>,
        rows: usize,
        row_offset: usize,
        col_offset: usize,
        mut draw_label: impl FnMut(usize, View<Msg>),
        mut draw_cell: impl FnMut(usize, usize, View<Msg>),
    ) {
        const LEFT_MARGIN_BORDER: i32 = 4;
        const MARGIN_BORDER: i32 = 2;
        view.split_off(Side::Top, 2).draw(&Border {
            side: Side::Bottom,
            width: MARGIN_BORDER,
            color: 100,
            start_offset: self.left_margin() - LEFT_MARGIN_BORDER,
            end_offset: self.right_margin() - MARGIN_BORDER,
        });
        for row in row_offset..(row_offset + rows) {
            let mut line_view = view.split_off(Side::Top, self.metrics.height);
            let mut margin_view = line_view.split_off(Side::Left, LEFT_MARGIN);
            margin_view
                .split_off(Side::Right, LEFT_MARGIN_BORDER)
                .draw(&Border {
                    side: Side::Right,
                    width: LEFT_MARGIN_BORDER,
                    color: 100,
                    start_offset: 0,
                    end_offset: 0,
                });
            draw_label(row, margin_view);
            line_view.handlers().on_ink(|ink| Msg::Write { row, ink });
            for col in (0..self.metrics.cols).map(|c| c + col_offset) {
                let char_view = line_view.split_off(Side::Left, self.metrics.width);
                draw_cell(row, col, char_view);
            }
            line_view.draw(&Border {
                side: Side::Left,
                width: MARGIN_BORDER,
                start_offset: 0,
                end_offset: 0,
                color: 100,
            });
        }
        view.split_off(Side::Top, 2).draw(&Border {
            side: Side::Top,
            width: MARGIN_BORDER,
            color: 100,
            start_offset: self.left_margin() - LEFT_MARGIN_BORDER,
            end_offset: self.right_margin() - MARGIN_BORDER,
        });
    }

    pub fn report_error<A, E: Display>(&mut self, result: Result<A, E>) -> Option<A> {
        match result {
            Ok(a) => Some(a),
            Err(e) => {
                self.error_string = format!("Error: {}", e);
                None
            }
        }
    }

    /// Our character recognizer is extremely fallible. To help improve it, we
    /// track the last few recognitions in a buffer. If we have to overwrite a
    /// recent recognition within the buffer window, we assume that the old ink
    /// should actually have been recognized as the new character. This helps
    /// bootstrap the template database; though it's still necessary to go look
    /// at the templates every once in a while and prune useless or incorrect
    /// ones.
    pub fn record_recognition(&mut self, coord: Coord, ink: Ink, best_char: char) {
        for r in &mut self.tentative_recognitions {
            // Assume we got it wrong the first time!
            if r.coord == coord {
                r.best_char = best_char;
                r.overwrites += 1;
            }
        }

        self.tentative_recognitions.push_back(Recognition {
            coord,
            ink,
            best_char,
            overwrites: 0,
        });

        while self.tentative_recognitions.len() > NUM_RECENT_RECOGNITIONS {
            if let Some(r) = self.tentative_recognitions.pop_front() {
                if r.overwrites > 0 {
                    if let Some(t) = self.templates.iter_mut().find(|c| c.char == r.best_char) {
                        t.templates.push(Template::from_ink(r.ink));
                        self.error_string = format!(
                            "NB: saved template for char '{}' at coordinates {:?}",
                            r.best_char, r.coord
                        );
                    }
                }
            }
        }
    }
}

fn fragment_at(
    buffer: &TextBuffer,
    (row, col): (usize, usize),
    metrics: &Metrics,
) -> Option<TextFragment> {
    let line = buffer.contents.get(row);
    line.and_then(|l| match col.cmp(&l.len()) {
        Ordering::Less => l
            .get(col)
            .map(|c| font::text_literal(metrics.height, &c.to_string()).with_weight(TEXT_WEIGHT)),
        Ordering::Equal => {
            let end_char = if row + 1 < buffer.contents.len() {
                "⏎"
            } else {
                "⌧"
            };
            Some(font::text_literal(metrics.height, end_char).with_weight(0.5))
        }
        _ => None,
    })
}

impl Widget for Editor {
    type Message = Msg;

    fn size(&self) -> Vector2<i32> {
        Vector2::new(SCREEN_WIDTH, SCREEN_HEIGHT)
    }

    fn render(&self, mut view: View<Msg>) {
        let mut header = view.split_off(Side::Top, TOP_MARGIN);
        header.split_off(Side::Left, LEFT_MARGIN);
        header.split_off(Side::Right, self.right_margin());

        match self.tab {
            Tab::Meta { .. } => {
                let head_text = Text::literal(DEFAULT_CHAR_HEIGHT, &*FONT, "Hi!");
                head_text.render_placed(header, 0.0, 0.5);
            }
            Tab::Edit => {
                let path_str = self
                    .path
                    .as_ref()
                    .map(|p| p.to_string_lossy())
                    .unwrap_or(Cow::Borrowed("unnamed file"));
                let path = self
                    .path
                    .as_ref()
                    .map(|f| f.to_string_lossy())
                    .or(env::var("HOME").ok().map(|mut s| {
                        // HOME often doesn't have a trailing slash, but multiples are OK.
                        s.push('/');
                        Cow::Owned(s)
                    }))
                    .unwrap_or(Cow::Borrowed("/"));
                let path_text = Text::builder(DEFAULT_CHAR_HEIGHT, &*FONT)
                    .message(Msg::SwitchTab {
                        tab: Tab::Meta {
                            path_buffer: TextBuffer::from_string(&path),
                            suggested: suggestions(&path).unwrap_or_default(),
                        },
                    })
                    .literal(&path_str)
                    .into_text();
                button("template", Msg::SwitchTab { tab: Tab::Template }, true).render_split(
                    &mut header,
                    Side::Right,
                    0.5,
                );
                button("save", Msg::Save, self.path.is_some() && self.dirty).render_split(
                    &mut header,
                    Side::Right,
                    0.5,
                );
                path_text.render_placed(header, 0.0, 0.5);
            }
            Tab::Template => {
                button("edit", Msg::SwitchTab { tab: Tab::Edit }, true).render_split(
                    &mut header,
                    Side::Right,
                    0.5,
                );
                header.leave_rest_blank();
            }
        }

        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            view.handlers()
                .pad(-100)
                .on_swipe(side, Msg::Swipe { towards: side });
        }

        match &self.tab {
            Tab::Meta {
                path_buffer,
                suggested,
            } => {
                self.draw_grid(
                    &mut view,
                    1,
                    0,
                    0,
                    |_n, _v| {},
                    |row, col, char_view| {
                        let ch = fragment_at(path_buffer, (row, col), &self.metrics);
                        let grid = GridCell {
                            baseline: self.metrics.baseline,
                            char: ch,
                            insert_area: false,
                        };
                        char_view.draw(&grid);
                    },
                );

                view.split_off(Side::Left, self.left_margin());
                view.split_off(Side::Right, self.right_margin());
                let mut buttons = view.split_off(Side::Top, TOP_MARGIN);

                for button in [
                    button("back", Msg::SwitchTab { tab: Tab::Edit }, true),
                    button("rename", Msg::Rename, true),
                    // TODO: disable if exists?
                    button("create", Msg::New, true),
                ]
                .into_iter()
                .rev()
                {
                    button.render_split(&mut buttons, Side::Right, 0.5)
                }

                buttons.leave_rest_blank();

                for s in suggested {
                    let mut suggest_view = view.split_off(Side::Top, self.metrics.height);
                    button("open", Msg::Open { path: s.clone() }, s.is_file()).render_split(
                        &mut suggest_view,
                        Side::Right,
                        0.5,
                    );
                    suggest_view.draw(&font::text_literal(
                        self.metrics.height,
                        &s.to_string_lossy(),
                    ));
                }
            }
            Tab::Edit => {
                self.draw_grid(
                    &mut view,
                    self.metrics.rows,
                    self.text.origin.0,
                    self.text.origin.1,
                    |_n, _v| {},
                    |row, col, char_view| {
                        let ch = self.text.fragment((row, col), &self.metrics);
                        let insert_area = self.text.insert_coords((row, col)).is_some();
                        let grid = GridCell {
                            baseline: self.metrics.baseline,
                            char: ch,
                            insert_area,
                        };
                        char_view.draw(&grid);
                    },
                );
                let text = Text::literal(
                    DEFAULT_CHAR_HEIGHT,
                    &*FONT,
                    &format!(
                        "{}:{} [{}]",
                        self.text.origin.0, self.text.origin.1, self.error_string
                    ),
                );
                text.render_placed(view, 0.5, 0.5);
            }
            Tab::Template => {
                self.draw_grid(
                    &mut view,
                    self.metrics.rows,
                    self.template_offset,
                    0,
                    |row, label_view| {
                        if let Some(templates) = self.templates.get(row) {
                            let char_text = Text::literal(
                                self.metrics.height,
                                &*FONT,
                                &format!("{} ", templates.char),
                            );
                            char_text.render_placed(label_view, 1.0, 0.0);
                        }
                    },
                    |row, col, mut template_view| {
                        let maybe_char = self.templates.get(row);
                        let grid = GridCell {
                            baseline: self.metrics.baseline,
                            // char: None,
                            char: maybe_char.map(|char_data| {
                                font::text_literal(self.metrics.height, &char_data.char.to_string())
                                    .with_weight(0.2)
                            }),
                            insert_area: false,
                        };
                        if let Some(char_data) = maybe_char {
                            if let Some(template) = char_data.templates.get(col) {
                                template_view.annotate(&template.ink);
                            }
                        }
                        template_view.draw(&grid);
                    },
                );
            }
        }
    }
}

const TEXT_WEIGHT: f32 = 0.9;

/// Naively, a mark is a "scratch out" if it has a lot of ink per unit area,
/// and also isn't extremely tiny.
fn is_erase(ink: &Ink) -> bool {
    let size = ink.bounds().size();
    let area = (size.x * size.y).max(500);
    let ratio = ink.ink_len() / area as f32;
    ratio >= 0.2
}

/// What sort of ink is this?
/// The categorization here is fairly naive / hardcoded, but should do for broad classes of inputs.
enum InkType {
    // A horizontal strike through the current line: typically, delete.
    Strikethrough { start: usize, end: usize },
    // A scratch-out of a single cell: typically, replace with whitespace.
    Scratch { col: usize },
    // Something that appears to be one or more characters.
    Glyphs { col: usize, parts: Vec<Ink> },
    // A line between characters; typically represents an insertion point.
    Carat { col: usize },
    // None of the above: typically, ignore.
    Junk,
}

impl InkType {
    fn classify(metrics: &Metrics, ink: Ink) -> InkType {
        if ink.len() == 0 {
            return InkType::Junk;
        }

        let min_x = ink.x_range.min / metrics.width as f32;
        let max_x = ink.x_range.max / metrics.width as f32;
        let min_y = ink.y_range.min / metrics.height as f32;
        let max_y = ink.y_range.max / metrics.height as f32;

        // Roughly: a strikethrough should be a single stroke that's mostly horizontal.
        if (max_x - min_x) > 1.5 && ink.strokes().count() == 1 {
            if ink.ink_len() / (ink.x_range.max - ink.x_range.min) < 1.2 {
                return InkType::Strikethrough {
                    start: (min_x.round().max(0.0) as usize),
                    end: max_x.round().max(0.0) as usize,
                };
            } else {
                // TODO: could just be a single char!
                // Maybe fall through and handle this case as part of char splitting?
                return InkType::Junk;
            }
        }

        let center = (min_x + max_x) / 2.0;

        // Detect the carat!
        // Vertical, and very close to a cell boundary.
        if min_y < 0.1
            && max_y > 0.9
            && (max_x - min_x) < 0.3
            && (center - center.round()).abs() < 0.3
            && center.round() >= 0.0
        {
            return InkType::Carat {
                col: center.round() as usize,
            };
        }

        if center < 0.0 {
            // Out of bounds!
            return InkType::Junk;
        }

        if is_erase(&ink) {
            let col = center as usize;
            return InkType::Scratch { col };
        }

        // Try and partition into multiple glyphs.
        let indices: Vec<_> = ink
            .strokes()
            .map(|stroke| {
                let sum_x: f32 = stroke.iter().map(|p| p.x as f32).sum();
                let center_x = sum_x / stroke.len() as f32;
                let center_x = center_x / metrics.width as f32;
                center_x.max(0.0) as usize
            })
            .collect();

        let min_col = indices.iter().copied().min().unwrap();
        let max_col = indices.iter().copied().max().unwrap();
        let mut inks = vec![Ink::new(); (max_col - min_col) + 1];
        for (stroke, col) in ink.strokes().zip(indices) {
            // Find the midpoint, bucket, and translate to an index.
            let ink = &mut inks[col - min_col];
            let x_offset = col as f32 * metrics.width as f32;
            for p in stroke {
                ink.push(p.x - x_offset, p.y, p.z);
            }
            ink.pen_up();
        }
        InkType::Glyphs {
            col: min_col,
            parts: inks,
        }
    }
}

const NUM_SUGGESTIONS: usize = 16;

fn suggestions(current_path: &str) -> io::Result<Vec<PathBuf>> {
    if let Some((dir, file)) = current_path.rsplit_once('/') {
        let dir = if dir.is_empty() { "/" } else { dir };
        let read = fs::read_dir(dir)?;
        let results = read
            .filter_map(|r| r.ok())
            .filter(|de| de.file_name().to_string_lossy().starts_with(file))
            .map(|de| de.path())
            .take(NUM_SUGGESTIONS)
            .collect();
        Ok(results)
    } else {
        Ok(vec![])
    }
}

impl Editor {
    fn update_path_from_meta(&mut self) {
        if let Tab::Meta { path_buffer, .. } = &mut self.tab {
            let path_string = path_buffer.content_string();
            let path_buf = PathBuf::from(path_string);
            if self.path.as_ref() != Some(&path_buf) {
                self.path = Some(path_buf);
                self.dirty = true;
            }
            self.tab = Tab::Edit;
        }
    }
}

impl Applet for Editor {
    type Upstream = ();

    fn update(&mut self, message: Self::Message) -> Option<Self::Upstream> {
        match message {
            Msg::Write { row, ink } => {
                let ink_type = InkType::classify(&self.metrics, ink);
                match &mut self.tab {
                    Tab::Meta {
                        path_buffer,
                        suggested,
                    } => {
                        match ink_type {
                            InkType::Scratch { col } => {
                                path_buffer.write((row, col), ' ');
                            }
                            InkType::Glyphs { mut col, parts } => {
                                for ink in parts {
                                    if let Some(c) = self.char_recognizer.best_match(&ink, f32::MAX)
                                    {
                                        path_buffer.write((row, col), c);
                                    }
                                    col += 1;
                                }
                            }
                            InkType::Strikethrough { start, end } => {
                                path_buffer.remove((row, start), (row, end));
                            }
                            InkType::Junk => {}
                            InkType::Carat { .. } => {}
                        }

                        *suggested = suggestions(&path_buffer.content_string()).unwrap_or_default();
                    }
                    Tab::Edit => {
                        self.dirty = true;
                        match ink_type {
                            InkType::Scratch { col } => {
                                let col = self.text.origin.1 + col;
                                self.text.write((row, col), ' ');
                                self.tentative_recognitions.clear();
                            }
                            InkType::Glyphs { col, parts } => {
                                let mut col = self.text.origin.1 + col;
                                for ink in parts {
                                    if let Some(c) = self.char_recognizer.best_match(&ink, f32::MAX)
                                    {
                                        self.text.write((row, col), c);
                                        self.record_recognition((row, col), ink, c);
                                    }
                                    col += 1;
                                }
                            }
                            InkType::Strikethrough { start, end } => {
                                let start = self.text.origin.1 + start;
                                let end = self.text.origin.1 + end;
                                self.text.buffer.remove((row, start), (row, end));
                                self.tentative_recognitions.clear();
                            }
                            InkType::Carat { col } => {
                                let col = self.text.origin.1 + col;
                                if self.text.insert.is_none() {
                                    self.text.open_insert((row, col));
                                } else {
                                    self.text.close_insert((row, col));
                                };
                                self.tentative_recognitions.clear();
                            }
                            InkType::Junk => {}
                        };
                    }
                    Tab::Template => {
                        if let Some(char_data) = self.templates.get_mut(row) {
                            match ink_type {
                                InkType::Strikethrough { start, end } => {
                                    let line_len = char_data.templates.len();
                                    for t in &mut char_data.templates
                                        [start.min(line_len)..end.min(line_len)]
                                    {
                                        t.serialized.clear();
                                        t.ink.clear();
                                    }
                                }
                                InkType::Scratch { col } => {
                                    if let Some(prev) = char_data.templates.get_mut(col) {
                                        prev.ink.clear();
                                        prev.serialized.clear();
                                    }
                                }
                                InkType::Glyphs { mut col, parts } => {
                                    char_data.templates.resize_with(
                                        char_data.templates.len().max(col + parts.len()),
                                        || Template::from_ink(Ink::new()),
                                    );
                                    for ink in parts {
                                        let mut prev = &mut char_data.templates[col];
                                        prev.ink.append(ink, 0.5);
                                        // TODO: put this off?
                                        prev.serialized = prev.ink.to_string();
                                        col += 1;
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
            Msg::SwitchTab { tab } => {
                if !matches!(tab, Tab::Template) && matches!(self.tab, Tab::Template) {
                    self.report_error(self.save_templates());
                    self.char_recognizer = CharRecognizer::new(&self.templates, &self.metrics);
                }
                self.tab = tab;
            }
            Msg::Erase { .. } => {}
            Msg::Swipe { towards } => match self.tab {
                // TODO: abstract over the pattern here.
                Tab::Edit => {
                    let movement = match towards {
                        Side::Top => (1, 0),
                        Side::Bottom => (-1, 0),
                        Side::Left => (0, 1),
                        Side::Right => (0, -1),
                    };
                    self.text.page_relative(movement);
                }
                Tab::Template => match towards {
                    Side::Top => {
                        self.template_offset += self.metrics.rows - 1;
                    }
                    Side::Bottom => {
                        self.template_offset -= (self.metrics.rows - 1).min(self.template_offset);
                    }
                    _ => {}
                },
                Tab::Meta { .. } => {
                    // Nothing to swipe here!
                }
            },
            Msg::Open { path } => {
                if let Some(file_contents) = self.report_error(fs::read_to_string(&path)) {
                    self.text = TextWindow::new(
                        TextBuffer::from_string(&file_contents),
                        (self.metrics.rows, self.metrics.cols),
                    );
                    self.path = Some(path);
                    self.tab = Tab::Edit;
                    self.dirty = false;
                    self.tentative_recognitions.clear();
                }
            }
            Msg::Rename => {
                self.update_path_from_meta();
            }
            Msg::New => {
                // Feels like there's a better way to chop this up.
                self.update_path_from_meta();
                self.text =
                    TextWindow::new(TextBuffer::empty(), (self.metrics.rows, self.metrics.cols));
            }
            Msg::Save => {
                if let Some(path) = &self.path {
                    let write_result = std::fs::write(path, self.text.buffer.content_string());
                    if write_result.is_ok() {
                        self.dirty = false;
                    }
                    self.report_error(write_result);
                }
            }
        }

        None
    }
}

fn button(text: &str, msg: Msg, active: bool) -> Text<Msg> {
    let builder = Text::builder(DEFAULT_CHAR_HEIGHT, &*FONT).literal("    ");
    let builder = if active {
        builder.message(msg).weight(TEXT_WEIGHT)
    } else {
        builder.weight(0.5)
    };
    builder.literal(text).into_text()
}

const NUM_RECENT_RECOGNITIONS: usize = 16;

fn main() {
    let mut app = app::App::new();

    let args = clap::Command::new("armrest-editor")
        .arg(Arg::new("file"))
        .get_matches();

    let file_string = if let Some(os_path) = args.value_of_os("file") {
        std::fs::read_to_string(os_path).expect("Unable to read specified file!")
    } else {
        HELP_TEXT.to_string() // Unnecessary cost, but not a big deal?
    };

    let template_path = BASE_DIRS
        .place_data_file(TEMPLATE_FILE)
        .expect("placing the template data file");

    let metrics = Metrics::new(DEFAULT_CHAR_HEIGHT);

    let char_recognizer = CharRecognizer::new(&[], &metrics);

    let dimensions = (metrics.rows, metrics.cols);

    let mut widget = Editor {
        path: None,
        template_path,
        metrics,
        error_string: "".to_string(),
        tab: Tab::Edit,
        template_offset: 0,
        templates: vec![],
        char_recognizer,
        text: TextWindow::new(TextBuffer::from_string(&file_string), dimensions),
        dirty: false,
        tentative_recognitions: VecDeque::with_capacity(NUM_RECENT_RECOGNITIONS),
    };

    let load_result = widget.load_templates();
    widget.report_error(load_result);

    app.run(&mut Component::new(widget))
}
