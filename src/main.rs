use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{BufReader, ErrorKind};
use std::path::PathBuf;
use std::time::Instant;
use std::{fs, io};

use libremarkable::cgmath::Point2;
use libremarkable::framebuffer::cgmath::Vector2;
use libremarkable::framebuffer::common::{color, DISPLAYHEIGHT, DISPLAYWIDTH};
use libremarkable::framebuffer::FramebufferDraw;
use once_cell::sync::Lazy;
use rusttype::{Font, Scale};
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

use armrest::app;
use armrest::app::{Applet, Component};
use armrest::dollar::Points;
use armrest::ink::Ink;
use armrest::ui::canvas::{Canvas, Fragment};
use armrest::ui::{Region, Side, Text, View, Widget};
use clap::Arg;

static FONT: Lazy<Font<'static>> = Lazy::new(|| {
    let font_bytes: &[u8] = include_bytes!("../fonts/Inconsolata-Regular.ttf");
    Font::from_bytes(font_bytes).unwrap()
});

// A set of characters that we always include in the template, even when not explicitly configured.
const PRINTABLE_ASCII: &str = "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

static BASE_DIRS: Lazy<BaseDirectories> =
    Lazy::new(|| BaseDirectories::with_prefix("armrest-editor").unwrap());

const SCREEN_HEIGHT: i32 = DISPLAYHEIGHT as i32;
const SCREEN_WIDTH: i32 = DISPLAYWIDTH as i32;
const TOP_MARGIN: i32 = 100;
const LEFT_MARGIN: i32 = 100;

const DEFAULT_CHAR_HEIGHT: i32 = 50;

const TEMPLATE_FILE: &str = "templates.json";

const HELP_TEXT: &str = "Welcome to armrest-edit!

It's a nice editor.
";

#[derive(Clone, Debug)]
enum Msg {
    SwitchTab(Tab),
    Write { row: usize, col: usize, ink: Ink },
}

struct EditChar {
    value: char,
    rendered: Text<Msg>,
}

#[derive(Hash)]
struct Metrics {
    height: i32,
    width: i32,
    baseline: i32,
    rows: usize,
    cols: usize,
}

impl Metrics {
    fn new(height: i32) -> Metrics {
        let scale = Scale::uniform(height as f32);
        let h_metrics = FONT.glyph(' ').scaled(scale).h_metrics();
        let v_metrics = FONT.v_metrics(scale);

        let width = h_metrics.advance_width.ceil() as i32;
        let baseline = (v_metrics.ascent as f32).ceil() as i32;

        let rows = (SCREEN_HEIGHT - TOP_MARGIN) / height;
        let cols = (SCREEN_WIDTH - LEFT_MARGIN) / width;

        Metrics {
            height,
            width,
            baseline,
            rows: rows as usize,
            cols: cols as usize,
        }
    }
}

#[derive(Hash)]
struct Grid(i32);

impl Fragment for Grid {
    fn draw(&self, canvas: &mut Canvas) {
        let size = canvas.bounds().size();
        for y in 0..size.y {
            canvas.write(0, y, color::GRAY(120));
        }
        for x in 1..size.x {
            canvas.write(x, self.0, color::GRAY(120));
            canvas.write(x, 4, color::GRAY(40));
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Tab {
    Edit,
    Template,
}

struct Editor {
    path: Option<PathBuf>,

    template_path: PathBuf,

    metrics: Metrics,

    tab: Tab,

    tab_buttons: Vec<Text<Msg>>,

    // for every char, a set of inks that represent it
    raw_templates: BTreeMap<char, Vec<Ink>>,

    // a couple vectors for doing dollar matches
    template_lookup: Vec<Points>,
    char_lookup: Vec<char>,

    // the contents of the file.
    contents: Vec<Vec<EditChar>>,
}

#[derive(Serialize, Deserialize)]
struct TemplateFile {
    templates: BTreeMap<char, Vec<Ink>>,
}

/// Convert an ink to a point cloud.
///
/// This differs from the suggested behaviour for $P, since it recenters and scales based on a
/// bounding box instead of the data itself. This is important for textual data, since the only
/// difference between an apostrophe and a comma is the position in the grid.
fn ink_to_points(ink: &Ink, metrics: &Metrics) -> Points {
    let mut points = Points::resample(ink);
    let bbox = Region::new(
        Point2::new(0, 0),
        Point2::new(metrics.width, metrics.height),
    );
    points.scale_region(bbox);
    points
}

impl Editor {
    fn load_templates(&mut self) -> io::Result<()> {
        let data = match File::open(&self.template_path) {
            Ok(file) => serde_json::from_reader(file)?,
            Err(e) if e.kind() == ErrorKind::NotFound => TemplateFile {
                templates: BTreeMap::new(),
            },
            Err(e) => Err(e)?,
        };

        self.raw_templates = data.templates;
        self.recalculate_lookups();

        Ok(())
    }

    fn save_templates(&self) -> io::Result<()> {
        // TODO: keep the whole file around maybe, to save the clone.
        let file_contents = TemplateFile {
            templates: self.raw_templates.clone(),
        };
        serde_json::to_writer(File::create(&self.template_path)?, &file_contents)?;
        Ok(())
    }

    fn recalculate_lookups(&mut self) {
        self.template_lookup.clear();
        self.char_lookup.clear();
        for (ch, inks) in &self.raw_templates {
            for ink in inks {
                if ink.len() == 0 {
                    continue;
                }
                self.template_lookup.push(ink_to_points(ink, &self.metrics));
                self.char_lookup.push(*ch);
            }
        }
    }
}

impl Widget for Editor {
    type Message = Msg;

    fn size(&self) -> Vector2<i32> {
        Vector2::new(SCREEN_WIDTH, SCREEN_HEIGHT)
    }

    fn render(&self, mut view: View<Self::Message>) {
        let mut header = view.split_off(Side::Top, TOP_MARGIN);
        for button in &self.tab_buttons {
            header.split_off(Side::Left, 40);
            button.render_split(&mut header, Side::Left, 0.5);
        }
        header.leave_rest_blank();

        match self.tab {
            Tab::Edit => {
                view.split_off(Side::Left, LEFT_MARGIN);
                for row in 0..self.metrics.rows {
                    let line = self.contents.get(row);
                    let mut line_view = view.split_off(Side::Top, self.metrics.height);
                    for col in 0..self.metrics.cols {
                        let ch = line.and_then(|l| l.get(col));
                        let mut char_view = line_view.split_off(Side::Left, self.metrics.width);
                        char_view
                            .handlers()
                            .on_ink(|ink| Msg::Write { row, col, ink });
                        if let Some(char) = ch {
                            char.rendered.render(char_view);
                        }
                    }
                }
            }
            Tab::Template => {
                let grid = Grid(self.metrics.baseline);
                for (row, (ch, templates)) in self
                    .raw_templates
                    .iter()
                    .take(self.metrics.rows)
                    .enumerate()
                {
                    let mut line_view = view.split_off(Side::Top, self.metrics.height);

                    // Put the char in the margin
                    let mut margin = line_view.split_off(Side::Left, LEFT_MARGIN);
                    let char_text = Text::literal(self.metrics.height, &*FONT, &format!("{} ", ch));
                    char_text.render_placed(margin, 1.0, 0.0);

                    for (col, template) in templates.iter().take(self.metrics.cols).enumerate() {
                        let mut template_view = line_view.split_off(Side::Left, self.metrics.width);
                        template_view
                            .handlers()
                            .on_ink(|ink| Msg::Write { row, col, ink });
                        template_view.annotate(template);
                        template_view.draw(&grid);
                    }
                }
            }
        }
    }
}

impl Applet for Editor {
    type Upstream = ();

    fn update(&mut self, message: Self::Message) -> Option<Self::Upstream> {
        match message {
            Msg::Write { row, col, ink } => match self.tab {
                Tab::Edit => {
                    if self.template_lookup.is_empty() {
                        return None;
                    }

                    let query = ink_to_points(&ink, &self.metrics);
                    let (best, score) = query.recognize(&self.template_lookup);
                    let best_char = self.char_lookup[best];

                    // TODO: not this.
                    while self.contents.len() <= row {
                        self.contents.push(vec![]);
                    }
                    let row = &mut self.contents[row];
                    while row.len() <= col {
                        row.push(EditChar {
                            value: ' ',
                            rendered: Text::literal(self.metrics.height, &*FONT, " "),
                        });
                    }
                    let new_text =
                        Text::literal(self.metrics.height, &*FONT, &best_char.to_string());
                    row[col] = EditChar {
                        value: best_char,
                        rendered: new_text,
                    };
                }
                Tab::Template => {
                    if let Some((ch, templates)) = self.raw_templates.iter_mut().nth(row) {
                        if let Some(prev) = templates.get_mut(col) {
                            prev.append(ink, 0.5);
                        }
                    }
                }
            },
            Msg::SwitchTab(tab) => {
                if tab != Tab::Template && self.tab == Tab::Template {
                    self.save_templates().expect("saving template file");
                    self.recalculate_lookups();
                }
                self.tab = tab;
            }
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
                    rendered: Text::literal(DEFAULT_CHAR_HEIGHT, &*FONT, &ch.to_string()),
                })
                .collect()
        })
        .collect();

    let template_path = BASE_DIRS
        .place_data_file(TEMPLATE_FILE)
        .expect("placing the template data file");

    let metrics = Metrics::new(DEFAULT_CHAR_HEIGHT);

    let mut raw_templates = BTreeMap::new();

    fn button(text: &str, tab: Tab) -> Text<Msg> {
        Text::builder(DEFAULT_CHAR_HEIGHT, &*FONT)
            .message(Msg::SwitchTab(tab))
            .literal(text)
            .into_text()
    }
    let tab_buttons = vec![button("template", Tab::Template), button("edit", Tab::Edit)];

    for ch in PRINTABLE_ASCII.chars() {
        raw_templates.insert(ch, vec![Ink::new(); metrics.cols]);
    }

    let mut widget = Editor {
        path: None,
        template_path,
        metrics,
        tab: Tab::Template,
        tab_buttons,
        raw_templates,
        template_lookup: vec![],
        char_lookup: vec![],
        contents,
    };

    widget.load_templates().expect("loading template file");

    app.run(&mut Component::new(widget))
}
