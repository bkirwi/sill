use std::borrow::{Borrow, Cow};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::fmt::Display;
use std::fs::File;
use std::io::{BufRead, BufReader, ErrorKind, Read};
use std::path::PathBuf;
use std::process::{Child, Stdio};
use std::rc::Rc;
use std::{env, fs, io, mem, process, thread};

use armrest::app;
use armrest::app::{Applet, Component};
use armrest::dollar::Points;
use armrest::ink::Ink;
use armrest::libremarkable::cgmath::Zero;
use armrest::libremarkable::framebuffer::cgmath::Vector2;
use armrest::libremarkable::framebuffer::common::{color, DISPLAYHEIGHT, DISPLAYWIDTH};
use armrest::ui::{Canvas, Fragment, Side, Text, View, Widget};
use clap::Arg;
use once_cell::sync::Lazy;
use xdg::BaseDirectories;

use font::*;
use grid_ui::*;
use hwr::*;
use ink_type::InkType;
use text_buffer::*;

mod font;
mod grid_ui;
mod hwr;
mod ink_type;
mod text_buffer;

static BASE_DIRS: Lazy<BaseDirectories> =
    Lazy::new(|| BaseDirectories::with_prefix("armrest-editor").unwrap());

const SCREEN_HEIGHT: i32 = DISPLAYHEIGHT as i32;
const SCREEN_WIDTH: i32 = DISPLAYWIDTH as i32;

const TOP_MARGIN: i32 = 100;
const LEFT_MARGIN: i32 = 100;
const DEFAULT_CHAR_HEIGHT: i32 = 40;

const TEMPLATE_FILE: &str = "templates.json";

const HELP_TEXT: &str = include_str!("intro.md");

#[derive(Clone)]
pub enum Msg {
    SwitchToMeta { current_path: Option<String> },
    SwitchTab { tab: Tab },
    Write { row: usize, ink: Ink },
    Erase { row: usize, ink: Ink },
    Swipe { towards: Side },
    Save { id: usize },
    Open { path: PathBuf },
    OpenShell {},
    SaveAs { id: usize, path: PathBuf },
    New,
}

#[derive(Clone)]
pub enum Tab {
    Meta {
        path_window: TextWindow,
        suggested: Vec<PathBuf>,
    },
    Edit {
        id: usize,
    },
    Template,
}

type Coord = (usize, usize);

#[derive(Clone)]

pub struct Carat {
    coord: Coord,
    ink: Ink,
}

#[derive(Clone)]
pub enum Selection {
    Normal,
    Single { carat: Carat },
    Range { start: Carat, end: Carat },
}

impl Default for Selection {
    fn default() -> Self {
        Selection::Normal
    }
}

#[derive(Clone)]
pub struct TextWindow {
    buffer: TextBuffer,
    atlas: Rc<Atlas>,
    grid_metrics: Metrics,
    selection: Selection,
    dimensions: Coord,
    origin: Coord,
    tentative_recognitions: VecDeque<Recognition>,
}

impl TextWindow {
    fn new(
        buffer: TextBuffer,
        atlas: Rc<Atlas>,
        metrics: Metrics,
        dimensions: Coord,
    ) -> TextWindow {
        TextWindow {
            buffer,
            atlas,
            grid_metrics: metrics,
            selection: Selection::Normal,
            dimensions,
            origin: (0, 0),
            tentative_recognitions: VecDeque::new(),
        }
    }

    fn page_relative(&mut self, (row_d, col_d): (isize, isize)) {
        let (row, col) = &mut self.origin;
        fn page_round(current: usize, delta: isize, size: usize) -> usize {
            // It's useful to stride less than a whole page, to preserve some context.
            // This is probably not quite the right number for "mid-size" panes...
            let stride = (size as isize - 5).max(1);
            (current as isize + delta * stride).max(0) as usize
        }
        *row = page_round(*row, row_d, self.dimensions.0);
        *col = page_round(*col, col_d, self.dimensions.1);
    }

    pub fn write(&mut self, coord: Coord, c: char) {
        self.buffer.write(coord, c);
    }

    pub fn carat(&mut self, carat: Carat) {
        // self.buffer.pad(carat.coord.0, carat.coord.1);
        self.selection = match mem::take(&mut self.selection) {
            Selection::Normal => Selection::Single { carat },
            Selection::Single { carat: original } => {
                let (start, end) = if carat.coord < original.coord {
                    (carat, original)
                } else {
                    (original, carat)
                };
                Selection::Range { start, end }
            }
            Selection::Range { .. } => {
                // Maybe eventually I'll prevent this case, but for now let's just reset.
                Selection::Normal
            }
        };
    }

    /// Our character recognizer is extremely fallible. To help improve it, we
    /// track the last few recognitions in a buffer. If we have to overwrite a
    /// recent recognition within the buffer window, we assume that the old ink
    /// should actually have been recognized as the new character. This helps
    /// bootstrap the template database; though it's still necessary to go look
    /// at the templates every once in a while and prune useless or incorrect
    /// ones.
    pub fn record_recognition(
        &mut self,
        coord: Coord,
        ink: Ink,
        best_char: char,
    ) -> Option<Recognition> {
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

        if self.tentative_recognitions.len() > NUM_RECENT_RECOGNITIONS {
            self.tentative_recognitions.pop_front()
        } else {
            None
        }
    }

    fn ink_row(&mut self, ink: Ink, row: usize, text_stuff: &mut TextStuff) {
        let ink_type = InkType::classify(&self.grid_metrics, ink);
        match ink_type {
            InkType::Scratch { col } => {
                let col = self.origin.1 + col;
                self.write((row, col), ' ');
                self.tentative_recognitions
                    .retain(|r| r.coord != (row, col));
            }
            InkType::Glyphs { tokens } => {
                if matches!(self.selection, Selection::Normal) {
                    for (col, ink) in tokens {
                        let col = col + self.origin.1;
                        if let Some(c) = text_stuff
                            .char_recognizer
                            .best_match(&ink_to_points(&ink, &self.grid_metrics), f32::MAX)
                        {
                            self.write((row, col), c);
                            if let Some(r) = self.record_recognition((row, col), ink, c) {
                                if r.overwrites > 0 {
                                    if let Some(t) = text_stuff
                                        .templates
                                        .iter_mut()
                                        .find(|c| c.char == r.best_char)
                                    {
                                        t.templates.push(Template::from_ink(r.ink));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    let ink = tokens.into_iter().next().unwrap().1;
                    match text_stuff
                        .big_recognizer
                        .best_match(&Points::normalize(&ink), f32::MAX)
                    {
                        Some('X') => {
                            if let Selection::Range { start, end } = &self.selection {
                                let trailing = self.buffer.split_off(end.coord);
                                let selection = self.buffer.split_off(start.coord);
                                text_stuff.clipboard = Some(selection);
                                self.buffer.append(trailing);
                                self.tentative_recognitions
                                    .retain(|r| r.coord < start.coord);
                            }
                            self.selection = Selection::Normal;
                        }
                        Some('C') => {
                            if let Selection::Range { start, end } = &self.selection {
                                // Regrettable!
                                let trailing = self.buffer.split_off(end.coord);
                                let selection = self.buffer.split_off(start.coord);
                                text_stuff.clipboard = Some(selection.clone());
                                self.buffer.append(selection);
                                self.buffer.append(trailing);
                            }
                            self.selection = Selection::Normal;
                        }
                        Some('V') => {
                            if let Selection::Single { carat } = &self.selection {
                                if let Some(buffer) = text_stuff.clipboard.take() {
                                    let trailing = self.buffer.split_off(carat.coord);
                                    self.buffer.append(buffer);
                                    self.buffer.append(trailing);
                                    self.tentative_recognitions
                                        .retain(|r| r.coord < carat.coord);
                                }
                            }
                            self.selection = Selection::Normal;
                        }
                        Some('S') => {
                            if let Selection::Range { start, end } = &self.selection {
                                self.buffer.pad(start.coord.0, start.coord.1);
                                let (lines, spaces) = if start.coord.0 == end.coord.0 {
                                    (0, end.coord.1 - start.coord.1)
                                } else {
                                    (end.coord.0 - start.coord.0, end.coord.1)
                                };
                                let mut string = String::new();
                                for _ in 0..lines {
                                    string.push('\n');
                                }
                                for _ in 0..spaces {
                                    string.push(' ');
                                }
                                self.buffer
                                    .splice(start.coord, TextBuffer::from_string(&string));
                                self.tentative_recognitions
                                    .retain(|r| r.coord < start.coord);
                            }
                            self.selection = Selection::Normal;
                        }
                        _ => {}
                    }
                }
            }
            InkType::Strikethrough { start, end } => {
                let start = self.origin.1 + start;
                let end = self.origin.1 + end;
                self.buffer.remove((row, start), (row, end));
                self.tentative_recognitions
                    .retain(|r| r.coord < (row, start));
            }
            InkType::Carat { col, ink } => {
                let col = self.origin.1 + col;
                self.carat(Carat {
                    coord: (row, col),
                    ink,
                });
            }
            InkType::Junk => {}
        };
    }
}

pub enum TextMessage {
    Write(usize, Ink),
}

impl Widget for TextWindow {
    type Message = TextMessage;

    fn size(&self) -> Vector2<i32> {
        let (rows, cols) = self.dimensions;
        let width = self.grid_metrics.width * cols as i32 + GRID_BORDER * 2;
        let height = self.grid_metrics.height * rows as i32 + GRID_BORDER * 6;
        Vector2::new(width, height)
    }

    fn render(&self, view: View<Self::Message>) {
        let (row_origin, col_origin) = self.origin;
        draw_grid(
            view,
            &self.grid_metrics,
            self.dimensions,
            |row_offset, view| {
                let row = row_origin + row_offset;
                view.handlers()
                    .map_region(|mut r| {
                        r.top_left.x -= GRID_BORDER;
                        r
                    })
                    .on_ink(|i| TextMessage::Write(row, i));
            },
            |row_offset, col_offset, mut view| {
                let row = row_origin + row_offset;
                let col = col_origin + col_offset;
                let coord = (row, col);

                let is_selected = match &self.selection {
                    Selection::Normal => false,
                    Selection::Single { carat } => {
                        if coord == carat.coord {
                            view.annotate(&carat.ink);
                        }
                        coord >= carat.coord
                    }
                    Selection::Range { start, end } => {
                        if coord == start.coord {
                            view.annotate(&start.ink);
                        }
                        if coord == end.coord {
                            view.annotate(&end.ink);
                        }
                        coord >= start.coord && coord < end.coord
                    }
                };

                let line = self.buffer.contents.get(row);
                let (char, background) = line
                    .map(|l| match col.cmp(&l.len()) {
                        Ordering::Less => (l.get(col).copied(), false),
                        Ordering::Equal => {
                            let char = if row + 1 == self.buffer.contents.len() {
                                '⌧'
                            } else {
                                '⏎'
                            };
                            (Some(char), true)
                        }
                        _ => (None, false),
                    })
                    .unwrap_or((None, false));

                let fragment = self.atlas.get_cell(char, is_selected, background);
                view.draw(&*fragment);
            },
        );
    }
}

/// This stores data from a recent recognition attempt, and the number of times it was overwritten
/// within the window we maintain. Idea being, if we have to go back and rewrite a char just after
/// we wrote it, we probably guessed wrong and should use it as a template.
#[derive(Clone)]
pub struct Recognition {
    coord: Coord,
    ink: Ink,
    best_char: char,
    overwrites: usize,
}

enum TabType {
    Text(TextTab),
    Shell(ShellTab),
}

struct ShellTab {
    child: Child,
    shell_output: TextWindow,
    command_window: TextWindow,
}

impl ShellTab {
    pub fn new(atlas: Rc<Atlas>, metrics: Metrics) -> io::Result<ShellTab> {
        // Launch a bash shell, wiring up everything.
        let mut child = process::Command::new("/bin/bash")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut stdout = child.stdout.take().expect("taking child stdout");
        thread::spawn(move || {
            let mut buffer = [0; 1024];
            loop {
                let read = match stdout.read(&mut buffer) {
                    Ok(size) => size,
                    Err(_) => {
                        break;
                    }
                };

                if read == 0 {
                    break;
                }

                // We're assuming that each chunk is itself valid utf8,
                // which might not be true! Correct would be to use from_utf8
                // and match on the error case to decide whether to insert a
                // replacement char or to wait for more input.
                let contents = String::from_utf8_lossy(&buffer[..read]);

                eprintln!("Got line: {}", contents);
            }
        });

        Ok(ShellTab {
            child,
            shell_output: TextWindow::new(
                TextBuffer::empty(),
                atlas.clone(),
                metrics.clone(),
                (10, 10),
            ),
            command_window: TextWindow::new(TextBuffer::empty(), atlas, metrics, (10, 10)),
        })
    }
}

struct TextTab {
    path: Option<PathBuf>,
    text: TextWindow,
    dirty: bool,
}

impl TextTab {
    fn save(&mut self) -> io::Result<()> {
        if let Some(path) = &self.path {
            let write_result = std::fs::write(path, self.text.buffer.content_string());
            if write_result.is_ok() {
                self.dirty = false;
            }
            write_result
        } else {
            Ok(())
        }
    }
}

struct Editor {
    metrics: Metrics,

    error_string: String,

    atlas: Rc<Atlas>,

    // tabs
    tab: Tab,

    // template stuff
    template_path: PathBuf,
    template_offset: usize,

    text_stuff: TextStuff,

    next_tab_id: usize,
    tabs: BTreeMap<usize, TabType>,
}

impl Editor {
    fn load_templates(&mut self) -> io::Result<()> {
        let data = match File::open(&self.template_path) {
            Ok(file) => serde_json::from_reader(file)?,
            Err(e) if e.kind() == ErrorKind::NotFound => TemplateFile::new(&[]),
            Err(e) => return Err(e),
        };

        self.text_stuff.templates = data.to_templates(self.metrics.height);
        self.text_stuff.init_recognizer(&self.metrics);

        Ok(())
    }

    fn save_templates(&self) -> io::Result<()> {
        let file_contents = TemplateFile::new(&self.text_stuff.templates);
        serde_json::to_writer(File::create(&self.template_path)?, &file_contents)?;
        Ok(())
    }

    fn left_margin(&self) -> i32 {
        LEFT_MARGIN
    }

    fn right_margin(&self) -> i32 {
        SCREEN_WIDTH - LEFT_MARGIN - 1200
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
}

impl Editor {
    fn load_meta(&self, path: Option<String>) -> Tab {
        let path = path
            .map(Cow::Owned)
            .or(env::var("HOME").ok().map(|mut s| {
                // HOME often doesn't have a trailing slash, but multiples are OK.
                s.push('/');
                Cow::Owned(s)
            }))
            .unwrap_or(Cow::Borrowed("/"));
        Tab::Meta {
            path_window: TextWindow::new(
                TextBuffer::from_string(&path),
                self.atlas.clone(),
                self.metrics.clone(),
                (1, self.max_dimensions().1),
            ),
            suggested: suggestions(&path).unwrap_or_default(),
        }
    }
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
                let head_text = Text::literal(DEFAULT_CHAR_HEIGHT, &*FONT, "armrest-edit v0.0.1");
                head_text.render_placed(header, 0.0, 0.5);
            }
            Tab::Edit { id } => {
                match &self.tabs[&id] {
                    TabType::Text(text_tab) => {
                        let path_str = text_tab
                            .path
                            .as_ref()
                            .map(|p| p.to_string_lossy())
                            .unwrap_or(Cow::Borrowed("<unnamed file>"));

                        button(
                            &path_str,
                            Msg::SwitchToMeta {
                                current_path: text_tab
                                    .path
                                    .as_ref()
                                    .and_then(|p| p.to_str().map(String::from)),
                            },
                            true,
                        )
                        .render_split(&mut header, Side::Left, 0.5);

                        Spaced(
                            40,
                            &[
                                button(
                                    "save",
                                    Msg::Save { id },
                                    text_tab.path.is_some() && text_tab.dirty,
                                ),
                                button("template", Msg::SwitchTab { tab: Tab::Template }, true),
                            ],
                        )
                        .render_placed(header, 1.0, 0.5);
                    }
                    TabType::Shell(_) => {
                        let name = format!("Shell #{}", id);
                        button(&name, Msg::SwitchToMeta { current_path: None }, true).render_split(
                            &mut header,
                            Side::Left,
                            0.5,
                        );
                        header.leave_rest_blank();
                    }
                };
            }
            Tab::Template => {
                button(
                    "edit",
                    Msg::SwitchTab {
                        tab: Tab::Edit { id: 0 },
                    },
                    true,
                )
                .render_split(&mut header, Side::Right, 0.5);
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
                path_window,
                suggested,
            } => {
                view.split_off(Side::Left, self.left_margin());

                path_window
                    .borrow()
                    .map(|message| match message {
                        TextMessage::Write(row, ink) => Msg::Write { row, ink },
                    })
                    .render_split(&mut view, Side::Top, 0.0);

                view.split_off(Side::Right, self.right_margin());

                let written_path: PathBuf = path_window.buffer.content_string().into();
                let entry_height = self.metrics.height * 3 / 2;

                let mut buttons = view.split_off(Side::Top, entry_height);

                Spaced(40, &[button("create", Msg::New, true)]).render_split(
                    &mut buttons,
                    Side::Right,
                    0.5,
                );

                buttons.leave_rest_blank();
                view.split_off(Side::Top, entry_height);

                Text::literal(self.metrics.height, &*FONT, "Tabs:").render_split(
                    &mut view,
                    Side::Top,
                    0.0,
                );

                for (tab_id, tab) in &self.tabs {
                    match &self.tabs[&tab_id] {
                        TabType::Text(tab) => {
                            let path_str = tab
                                .path
                                .as_ref()
                                .map(|p| p.to_string_lossy())
                                .unwrap_or(Cow::Borrowed("<unnamed file>"));
                            let tab_label = button(
                                &path_str,
                                Msg::SwitchTab {
                                    tab: Tab::Edit { id: *tab_id },
                                },
                                true,
                            );
                            let mut tab_view = view.split_off(Side::Top, entry_height);
                            tab_view.split_off(Side::Left, 20);
                            tab_label.render_split(&mut tab_view, Side::Left, 0.5);

                            button(
                                "save as",
                                Msg::SaveAs {
                                    id: *tab_id,
                                    path: written_path.clone(),
                                },
                                true,
                            )
                            .render_split(
                                &mut tab_view,
                                Side::Right,
                                0.5,
                            );
                        }
                        TabType::Shell(shell_tab) => {
                            let name = format!("Shell #{}", tab_id);
                            let tab_label = button(
                                &name,
                                Msg::SwitchTab {
                                    tab: Tab::Edit { id: *tab_id },
                                },
                                true,
                            );
                            let mut tab_view = view.split_off(Side::Top, entry_height);
                            tab_view.split_off(Side::Left, 20);
                            tab_label.render_split(&mut tab_view, Side::Left, 0.5);
                        }
                    }
                }

                view.split_off(Side::Top, entry_height);

                Text::literal(self.metrics.height, &*FONT, "Paths:").render_split(
                    &mut view,
                    Side::Top,
                    0.0,
                );

                for s in suggested {
                    let mut suggest_view = view.split_off(Side::Top, entry_height);
                    let path_string = if s.is_dir() {
                        let mut owned = s.to_string_lossy().into_owned();
                        owned.push('/');
                        owned.into()
                    } else {
                        s.to_string_lossy()
                    };

                    let msg = if s.is_file() {
                        Msg::Open { path: s.clone() }
                    } else {
                        Msg::SwitchToMeta {
                            current_path: Some(path_string.to_string()),
                        }
                    };

                    button(&path_string, msg, s.exists()).render_split(
                        &mut suggest_view,
                        Side::Left,
                        0.5,
                    );

                    button(
                        "copy",
                        Msg::SwitchToMeta {
                            current_path: Some(path_string.to_string()),
                        },
                        true,
                    )
                    .render_split(&mut suggest_view, Side::Right, 0.5);
                }
            }
            Tab::Edit { id } => {
                match &self.tabs[&id] {
                    TabType::Text(text_tab) => {
                        // Run the line numbers down the margin!
                        let mut margin_view = view.split_off(Side::Left, self.left_margin());
                        margin_view.split_off(Side::Right, 20);
                        // Based on the top margin of the text area and the baseline height.
                        // TODO: calculate this from other metrics.
                        margin_view.split_off(Side::Top, 7);
                        for row in (text_tab.text.origin.0..).take(text_tab.text.dimensions.0) {
                            let mut view =
                                margin_view.split_off(Side::Top, text_tab.text.grid_metrics.height);
                            let text = Text::literal(30, &*FONT, &format!("{}", row));
                            text.render_placed(view, 1.0, 1.0);
                        }
                        margin_view.leave_rest_blank();

                        text_tab
                            .text
                            .borrow()
                            .map(|message| match message {
                                TextMessage::Write(row, ink) => Msg::Write { row, ink },
                            })
                            .render_split(&mut view, Side::Top, 0.0);

                        let text = Text::literal(
                            DEFAULT_CHAR_HEIGHT,
                            &*FONT,
                            &format!(
                                "{}:{} [{}]",
                                text_tab.text.origin.0, text_tab.text.origin.1, self.error_string
                            ),
                        );
                        text.render_placed(view, 0.0, 0.4);
                    }
                    TabType::Shell(shell_tab) => {
                        shell_tab
                            .shell_output
                            .borrow()
                            .map(|message| match message {
                                TextMessage::Write(row, ink) => Msg::Write { row, ink },
                            })
                            .render_split(&mut view, Side::Top, 0.5);
                    }
                }
            }
            Tab::Template => {
                let mut margin_view = view.split_off(Side::Left, self.left_margin());
                let margin_placement = self.metrics.baseline as f32 / self.metrics.height as f32;
                for ct in self.text_stuff.templates[self.template_offset..]
                    .iter()
                    .take(self.max_dimensions().0)
                {
                    let mut view = margin_view.split_off(Side::Top, self.metrics.height);
                    view.split_off(Side::Right, 20);
                    let text = Text::literal(30, &*FONT, &format!("{}", ct.char));
                    text.render_placed(view, 1.0, margin_placement);
                }
                margin_view.leave_rest_blank();

                let (height, width) = self.max_dimensions();
                let height = height.min(self.text_stuff.templates.len() - self.template_offset);

                draw_grid(
                    view,
                    &self.metrics,
                    (height, width),
                    |row, view| {
                        let row = row + self.template_offset;
                        view.handlers()
                            .map_region(|mut r| {
                                r.top_left.x -= GRID_BORDER;
                                r
                            })
                            .on_ink(|ink| Msg::Write { row, ink });
                    },
                    |row, col, mut template_view| {
                        let row = self.template_offset + row;
                        let maybe_char = self.text_stuff.templates.get(row);
                        let grid = self
                            .atlas
                            .get_cell(maybe_char.map(|ct| ct.char), false, true);
                        if let Some(char_data) = maybe_char {
                            if let Some(template) = char_data.templates.get(col) {
                                template_view.annotate(&template.ink);
                            }
                        }
                        template_view.draw(&*grid);
                    },
                );
            }
        }
    }
}

const TEXT_WEIGHT: f32 = 0.9;

const NUM_SUGGESTIONS: usize = 16;
const MAX_DIR_ENTRIES: usize = 1024;

fn suggestions(current_path: &str) -> io::Result<Vec<PathBuf>> {
    if !current_path.starts_with('/') {
        // All paths must be absolute.
        return Ok(vec![]);
    }
    let (dir, file) = current_path.rsplit_once('/').expect("splitting /path by /");
    let dir = if dir.is_empty() { "/" } else { dir };
    let read = fs::read_dir(dir)?;
    let mut results: Vec<_> = read
        .filter_map(|r| r.ok())
        .filter_map(|de| {
            de.file_name()
                .to_str()
                .filter(|s| s.starts_with(file))
                .map(|_| de.path())
        })
        .take(MAX_DIR_ENTRIES)
        .collect();

    // NB: if this is slow, pull in the partial sort crate.
    results.sort();
    results.truncate(NUM_SUGGESTIONS);

    Ok(results)
}

fn max_dimensions(metrics: &Metrics) -> Coord {
    let rows = (SCREEN_HEIGHT - TOP_MARGIN * 2) / metrics.height;
    let cols = (SCREEN_WIDTH - LEFT_MARGIN * 2) / metrics.width;
    (rows as usize, cols as usize)
}

impl Editor {
    fn max_dimensions(&self) -> Coord {
        max_dimensions(&self.metrics)
    }
}

impl Applet for Editor {
    type Upstream = ();

    fn update(&mut self, message: Self::Message) -> Option<Self::Upstream> {
        match message {
            Msg::Write { row, ink } => match &mut self.tab {
                Tab::Meta {
                    path_window,
                    suggested,
                } => {
                    path_window.ink_row(ink, row, &mut self.text_stuff);
                    *suggested =
                        suggestions(&path_window.buffer.content_string()).unwrap_or_default();
                }
                Tab::Edit { id } => match self.tabs.get_mut(&id).unwrap() {
                    TabType::Text(text_tab) => {
                        text_tab.dirty = true;
                        text_tab.text.ink_row(ink, row, &mut self.text_stuff);
                    }
                    TabType::Shell(_) => {
                        todo!("write...");
                    }
                },
                Tab::Template => {
                    let ink_type = InkType::classify(&self.metrics, ink);
                    if let Some(char_data) = self.text_stuff.templates.get_mut(row) {
                        match ink_type {
                            InkType::Strikethrough { start, end } => {
                                let line_len = char_data.templates.len();
                                for t in
                                    &mut char_data.templates[start.min(line_len)..end.min(line_len)]
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
                            InkType::Glyphs { tokens } => {
                                for (col, ink) in tokens {
                                    if col >= char_data.templates.len() {
                                        char_data
                                            .templates
                                            .resize_with(col + 1, || Template::from_ink(Ink::new()))
                                    }
                                    let mut prev = &mut char_data.templates[col];
                                    prev.ink.append(ink, 0.5);
                                    prev.serialized = prev.ink.to_string();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            },
            Msg::SwitchTab { tab } => {
                if !matches!(tab, Tab::Template) && matches!(self.tab, Tab::Template) {
                    self.report_error(self.save_templates());
                    self.text_stuff.init_recognizer(&self.metrics);
                }
                self.tab = tab;
            }
            Msg::Erase { .. } => {}
            Msg::Swipe { towards } => match self.tab {
                // TODO: abstract over the pattern here.
                Tab::Edit { id } => {
                    let movement = match towards {
                        Side::Top => (1, 0),
                        Side::Bottom => (-1, 0),
                        Side::Left => (0, 1),
                        Side::Right => (0, -1),
                    };
                    match self.tabs.get_mut(&id).unwrap() {
                        TabType::Text(text_tab) => {
                            text_tab.text.page_relative(movement);
                        }
                        TabType::Shell(_) => {
                            todo!("Implement swipe");
                        }
                    }
                }
                Tab::Template => {
                    let (rows, _) = self.max_dimensions();
                    match towards {
                        Side::Top => {
                            self.template_offset += rows - 1;
                        }
                        Side::Bottom => {
                            self.template_offset -= (rows - 1).min(self.template_offset);
                        }
                        _ => {}
                    }
                }
                Tab::Meta { .. } => {
                    // Nothing to swipe here!
                }
            },
            Msg::Open { path } => {
                if let Some(file_contents) = self.report_error(fs::read_to_string(&path)) {
                    let id = self.next_tab_id;
                    self.next_tab_id += 1;
                    self.tabs.insert(
                        id,
                        TabType::Text(TextTab {
                            path: Some(path),
                            dirty: false,
                            text: TextWindow::new(
                                TextBuffer::from_string(&file_contents),
                                self.atlas.clone(),
                                self.metrics.clone(),
                                self.max_dimensions(),
                            ),
                        }),
                    );
                    self.tab = Tab::Edit { id };
                }
            }
            Msg::SaveAs { id, path } => {
                if !path.exists() && path.parent().iter().any(|p| p.is_dir()) {
                    match self.tabs.get_mut(&id).unwrap() {
                        TabType::Text(tab) => {
                            tab.path = Some(path);
                            let saved = tab.save();
                            if self.report_error(saved).is_some() {
                                self.tab = Tab::Edit { id }
                            };
                        }
                        _ => {}
                    }
                }
            }
            Msg::New => {
                // TODO: thread a path through here from meta.
                let id = self.next_tab_id;
                self.next_tab_id += 1;
                self.tabs.insert(
                    id,
                    TabType::Text(TextTab {
                        path: None,
                        text: TextWindow::new(
                            TextBuffer::empty(),
                            self.atlas.clone(),
                            self.metrics.clone(),
                            self.max_dimensions(),
                        ),
                        dirty: false,
                    }),
                );
                self.tab = Tab::Edit { id }
            }
            Msg::Save { id } => match self.tabs.get_mut(&id).unwrap() {
                TabType::Text(text_tab) => {
                    let result = text_tab.save();
                    self.report_error(result);
                }
                _ => {}
            },
            Msg::SwitchToMeta { current_path } => {
                self.tab = self.load_meta(current_path);
            }
            Msg::OpenShell { .. } => {}
        }

        None
    }

    fn current_route(&self) -> &str {
        match self.tab {
            Tab::Meta { .. } => "meta",
            Tab::Edit { .. } => "edit",
            Tab::Template => "template",
        }
    }
}

#[derive(Hash)]
struct Underline(i32);
const UNDERLINE: i32 = 3;

impl Fragment for Underline {
    fn draw(&self, canvas: &mut Canvas) {
        let size = canvas.bounds().size();
        for y in 0..size.y.min(UNDERLINE) {
            for x in y..(size.x.min(self.0) - y) {
                canvas.write(x, y, color::GRAY(200));
            }
        }
    }
}

struct Button<T: Widget> {
    widget: T,
    on_tap: Option<T::Message>,
}

impl<T: Widget> Widget for Button<T>
where
    T::Message: Clone,
{
    type Message = T::Message;

    fn size(&self) -> Vector2<i32> {
        let mut size = self.widget.size();
        size.y += UNDERLINE;
        size
    }

    fn render(&self, mut view: View<Self::Message>) {
        if let Some(msg) = &self.on_tap {
            view.handlers().pad(10).on_tap(msg.clone());
        }
        self.widget.render_split(&mut view, Side::Top, 0.0);
        if self.on_tap.is_some() {
            view.split_off(Side::Top, UNDERLINE)
                .draw(&Underline(self.size().x))
        }
    }
}

fn button(text: &str, msg: Msg, active: bool) -> Button<Text<Msg>> {
    let builder = Text::builder(DEFAULT_CHAR_HEIGHT, &*FONT);
    let builder = if active {
        builder.weight(TEXT_WEIGHT)
    } else {
        builder.weight(0.5)
    };
    let text = builder.literal(text).into_text();

    Button {
        widget: text,
        on_tap: if active { Some(msg) } else { None },
    }
}

struct Spaced<'a, A>(i32, &'a [A]);

impl<'a, A: Widget> Widget for Spaced<'a, A> {
    type Message = A::Message;

    fn size(&self) -> Vector2<i32> {
        let mut size: Vector2<i32> = Vector2::zero();
        let Spaced(pad, widgets) = self;
        for (i, a) in widgets.iter().enumerate() {
            if i != 0 {
                size.x += *pad;
            }
            let a_size = a.size();
            size.x += a_size.x;
            size.y = size.y.max(a_size.y);
        }
        size
    }

    fn render(&self, mut view: View<Self::Message>) {
        let Spaced(pad, widgets) = self;
        for (i, a) in widgets.iter().enumerate() {
            if i != 0 {
                view.split_off(Side::Left, *pad);
            }
            a.render_split(&mut view, Side::Left, 0.0);
        }
    }
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

    let atlas = Rc::new(Atlas::new(metrics.clone()));

    let mut widget = Editor {
        template_path,
        metrics: metrics.clone(),
        error_string: "".to_string(),
        atlas: atlas.clone(),
        tab: Tab::Template,
        template_offset: 0,
        text_stuff: TextStuff::new(),
        next_tab_id: 0,
        tabs: BTreeMap::new(),
    };

    widget.update(Msg::SwitchToMeta { current_path: None });

    let load_result = widget.load_templates();
    widget.report_error(load_result);

    app.run(&mut Component::new(widget))
}
