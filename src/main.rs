use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs::File;
use std::io;
use std::io::ErrorKind;
use std::path::PathBuf;

use armrest::app;
use armrest::app::{Applet, Component};
use armrest::dollar::Points;
use armrest::ink::Ink;
use armrest::ui::canvas::Fragment;
use armrest::ui::{Side, Text, TextFragment, View, Widget};
use clap::Arg;
use libremarkable::framebuffer::cgmath::Vector2;
use libremarkable::framebuffer::common::{DISPLAYHEIGHT, DISPLAYWIDTH};
use once_cell::sync::Lazy;
use rusttype::{Font, Scale};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

use grid_ui::*;
use hwr::*;

mod grid_ui;
mod hwr;

static FONT: Lazy<Font<'static>> = Lazy::new(|| {
    let font_bytes: &[u8] = include_bytes!("../fonts/Inconsolata-Regular.ttf");
    Font::from_bytes(font_bytes).unwrap()
});

fn text_literal(height: i32, text: &str) -> TextFragment {
    // NB: Inconsolata has zero line gap.
    Text::builder(height, &*FONT)
        .literal(text)
        .into_text()
        .to_fragment()
}

static BASE_DIRS: Lazy<BaseDirectories> =
    Lazy::new(|| BaseDirectories::with_prefix("armrest-editor").unwrap());

const SCREEN_HEIGHT: i32 = DISPLAYHEIGHT as i32;
const SCREEN_WIDTH: i32 = DISPLAYWIDTH as i32;
const TOP_MARGIN: i32 = 100;
const LEFT_MARGIN: i32 = 100;

const DEFAULT_CHAR_HEIGHT: i32 = 40;

const TEMPLATE_FILE: &str = "templates.json";

const HELP_TEXT: &str = "Welcome to armrest-edit!

It's a nice editor.
";

#[derive(Clone, Debug)]
pub enum Msg {
    SwitchTab { tab: Tab },
    Write { row: usize, col: usize, ink: Ink },
    Erase { row: usize, ink: Ink },
    Swipe { towards: Side },
}

#[derive(Hash, Clone)]
struct EditChar {
    value: char,
    rendered: Option<TextFragment>,
}

#[derive(Hash, Clone)]
pub struct Metrics {
    height: i32,
    width: i32,
    baseline: i32,
    rows: usize,
    cols: usize,
    left_margin: i32,
    right_margin: i32,
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
            left_margin: LEFT_MARGIN,
            right_margin: SCREEN_WIDTH - LEFT_MARGIN - cols * width,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Tab {
    Edit,
    Template,
}

struct Editor {
    path: Option<PathBuf>,

    template_path: PathBuf,

    metrics: Metrics,

    tab: Tab,

    tab_buttons: Vec<Text<Msg>>,

    template_offset: usize,
    templates: Vec<CharData>,

    // a couple vectors for doing dollar matches
    char_recognizer: CharRecognizer,

    row_offset: usize,
    contents: Vec<Vec<EditChar>>,
}

/// Convert an ink to a point cloud.
///
/// This differs from the suggested behaviour for $P, since it recenters and scales based on a
/// bounding box instead of the data itself. This is important for textual data, since the only
/// difference between an apostrophe and a comma is the position in the grid.
fn ink_to_points(ink: &Ink, metrics: &Metrics) -> Points {
    let mut points = Points::resample(ink);

    let mut center = points.centroid();
    center.y = metrics.height as f32 / 2.0;
    points.recenter_on(center);

    points.scale_by(1.0 / metrics.width as f32);

    points
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
        // TODO: keep the whole file around maybe, to save the clone.
        let file_contents = TemplateFile::new(&self.templates);
        serde_json::to_writer(File::create(&self.template_path)?, &file_contents)?;
        Ok(())
    }

    fn draw_grid(
        &self,
        view: &mut View<Msg>,
        mut draw_label: impl FnMut(usize, View<Msg>),
        mut draw_cell: impl FnMut(usize, usize, View<Msg>),
    ) {
        view.split_off(Side::Top, 2).draw(&Border {
            side: Side::Bottom,
            width: 2,
            color: 100,
            start_offset: self.metrics.left_margin - 4,
            end_offset: self.metrics.right_margin - 2,
        });
        for row in 0..self.metrics.rows {
            let mut line_view = view.split_off(Side::Top, self.metrics.height);
            let mut margin_view = line_view.split_off(Side::Left, LEFT_MARGIN);
            margin_view.split_off(Side::Right, 10).draw(&Border {
                side: Side::Right,
                width: 4,
                color: 100,
                start_offset: 0,
                end_offset: 0,
            });
            draw_label(row, margin_view);
            for col in 0..self.metrics.cols {
                let char_view = line_view.split_off(Side::Left, self.metrics.width);
                draw_cell(row, col, char_view);
            }
            line_view.draw(&Border {
                side: Side::Left,
                width: 2,
                start_offset: 0,
                end_offset: 0,
                color: 100,
            });
        }
        view.split_off(Side::Top, 2).draw(&Border {
            side: Side::Top,
            width: 2,
            color: 100,
            start_offset: self.metrics.left_margin - 2,
            end_offset: self.metrics.right_margin - 2,
        });
    }
}

impl Widget for Editor {
    type Message = Msg;

    fn size(&self) -> Vector2<i32> {
        Vector2::new(SCREEN_WIDTH, SCREEN_HEIGHT)
    }

    fn render(&self, mut view: View<Msg>) {
        let mut header = view.split_off(Side::Top, TOP_MARGIN);
        for button in &self.tab_buttons {
            header.split_off(Side::Left, 40);
            button.render_split(&mut header, Side::Left, 0.5);
        }
        header.leave_rest_blank();

        view.handlers().on_swipe(
            Side::Bottom,
            Msg::Swipe {
                towards: Side::Bottom,
            },
        );

        view.handlers()
            .on_swipe(Side::Top, Msg::Swipe { towards: Side::Top });

        match self.tab {
            Tab::Edit => {
                self.draw_grid(
                    &mut view,
                    |_n, _v| {},
                    |row, col, mut char_view| {
                        let row = row + self.row_offset;
                        let ch = self.contents.get(row).and_then(|l| l.get(col));
                        char_view
                            .handlers()
                            .on_ink(|ink| Msg::Write { row, col, ink });
                        let grid = GridCell {
                            baseline: self.metrics.baseline,
                            char: ch.and_then(|c| c.rendered.clone()),
                        };
                        char_view.draw(&grid);
                    },
                );
                let text = Text::literal(
                    DEFAULT_CHAR_HEIGHT,
                    &*FONT,
                    &format!(
                        "Lines {}-{}",
                        self.row_offset,
                        self.row_offset + self.metrics.rows
                    ),
                );
                text.render(view);
            }
            Tab::Template => {
                self.draw_grid(
                    &mut view,
                    |row, label_view| {
                        let row = row + self.template_offset;
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
                        let row = row + self.template_offset;
                        let maybe_char = self.templates.get(row);
                        let grid = GridCell {
                            baseline: self.metrics.baseline,
                            char: maybe_char.map(|char_data| {
                                text_literal(self.metrics.height, &char_data.char.to_string())
                                    .with_weight(0.2)
                            }),
                        };
                        if let Some(char_data) = maybe_char {
                            template_view
                                .handlers()
                                .on_ink(|ink| Msg::Write { row, col, ink });
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

impl Applet for Editor {
    type Upstream = ();

    fn update(&mut self, message: Self::Message) -> Option<Self::Upstream> {
        match message {
            Msg::Write { row, col, ink } => match self.tab {
                Tab::Edit => {
                    let best_char = if is_erase(&ink) {
                        ' '
                    } else {
                        match self.char_recognizer.best_match(&ink, f32::MAX) {
                            None => {
                                return None;
                            }
                            Some(c) => c,
                        }
                    };

                    if self.contents.len() <= row {
                        self.contents.resize(row + 1, vec![]);
                    }
                    let row = &mut self.contents[row];
                    if row.len() <= col {
                        row.resize(
                            col + 1,
                            EditChar {
                                value: ' ',
                                rendered: None,
                            },
                        );
                    }

                    let new_text = text_literal(self.metrics.height, &best_char.to_string())
                        .with_weight(TEXT_WEIGHT);
                    row[col] = EditChar {
                        value: best_char,
                        rendered: Some(new_text),
                    };
                }
                Tab::Template => {
                    if let Some(char_data) = self.templates.get_mut(row) {
                        if let Some(prev) = char_data.templates.get_mut(col) {
                            if is_erase(&ink) {
                                prev.ink.clear();
                                prev.serialized.clear();
                            } else {
                                prev.ink.append(ink, 0.5);
                                // TODO: put this off?
                                prev.serialized = prev.ink.to_string();
                            }
                        }
                    }
                }
            },
            Msg::SwitchTab { tab } => {
                if tab != Tab::Template && self.tab == Tab::Template {
                    self.save_templates().expect("saving template file");
                    self.char_recognizer = CharRecognizer::new(&self.templates, &self.metrics);
                }
                self.tab = tab;
            }
            Msg::Erase { .. } => {}
            Msg::Swipe { towards } => match self.tab {
                // TODO: abstract over the pattern here.
                Tab::Edit => match towards {
                    Side::Top => {
                        self.row_offset += self.metrics.rows - 1;
                    }
                    Side::Bottom => {
                        self.row_offset -= (self.metrics.rows - 1).min(self.row_offset);
                    }
                    _ => {}
                },
                Tab::Template => match towards {
                    Side::Top => {
                        self.template_offset += self.metrics.rows - 1;
                    }
                    Side::Bottom => {
                        self.template_offset -= (self.metrics.rows - 1).min(self.template_offset);
                    }
                    _ => {}
                },
            },
        }

        None
    }
}

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

    let contents = file_string
        .lines()
        .map(|line| {
            line.chars()
                .map(|ch| EditChar {
                    value: ch,
                    rendered: Some(
                        text_literal(DEFAULT_CHAR_HEIGHT, &ch.to_string()).with_weight(TEXT_WEIGHT),
                    ),
                })
                .collect()
        })
        .collect();

    let template_path = BASE_DIRS
        .place_data_file(TEMPLATE_FILE)
        .expect("placing the template data file");

    let metrics = Metrics::new(DEFAULT_CHAR_HEIGHT);

    fn button(text: &str, tab: Tab) -> Text<Msg> {
        Text::builder(DEFAULT_CHAR_HEIGHT, &*FONT)
            .message(Msg::SwitchTab { tab })
            .literal(text)
            .into_text()
    }
    let tab_buttons = vec![button("template", Tab::Template), button("edit", Tab::Edit)];

    let char_recognizer = CharRecognizer::new(&[], &metrics);

    let mut widget = Editor {
        path: None,
        template_path,
        metrics,
        tab: Tab::Template,
        tab_buttons,
        template_offset: 0,
        templates: vec![],
        char_recognizer,
        row_offset: 0,
        contents,
    };

    widget.load_templates().expect("loading template file");

    app.run(&mut Component::new(widget))
}
