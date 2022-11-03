use std::borrow::{Borrow, Cow};
use std::collections::{BTreeMap, VecDeque};
use std::fmt::Display;
use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::rc::Rc;
use std::{env, fs, io, process, thread};

use armrest::app;
use armrest::app::{Applet, Component, Sender};
use armrest::ink::Ink;

use anyhow;
use armrest::libremarkable::framebuffer::cgmath::Vector2;
use armrest::libremarkable::framebuffer::common::{DISPLAYHEIGHT, DISPLAYWIDTH};
use armrest::ui::{Side, Text, View, Widget};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use xdg::BaseDirectories;

use font::*;
use grid_ui::*;
use hwr::*;
use ink_type::InkType;
use text_buffer::*;
use text_window::Selection;
use text_window::TextMessage;
use text_window::TextWindow;
use widgets::*;

mod font;
mod grid_ui;
mod hwr;
mod ink_type;
mod text_buffer;
mod text_window;
mod util;
mod widgets;

static BASE_DIRS: Lazy<BaseDirectories> =
    Lazy::new(|| BaseDirectories::with_prefix(env!("CARGO_PKG_NAME")).unwrap());

const SCREEN_HEIGHT: i32 = DISPLAYHEIGHT as i32;
const SCREEN_WIDTH: i32 = DISPLAYWIDTH as i32;

const TOP_MARGIN: i32 = 100;
const LEFT_MARGIN: i32 = 100;
const DEFAULT_CHAR_HEIGHT: i32 = 40;

const TEMPLATE_FILE: &str = "templates.json";
const CONFIG_FILE: &str = "sill.toml";
const BASH_RC_FILE: &str = "sill.bashrc";

const HELP_TEXT: &str = include_str!("../README.md");

static APP_NAME: Lazy<String> =
    Lazy::new(|| format!("{} {}", env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")));

#[derive(Debug, Clone, Serialize, Deserialize, Eq, PartialEq)]
#[serde(default)]
struct Config {
    cell_height: i32,
}

impl Default for Config {
    fn default() -> Self {
        Config { cell_height: 40 }
    }
}

#[derive(Clone)]
pub enum Msg {
    MetaPath { current_path: String },
    SwitchTab { tab: Tab },
    Write { ink: Ink },
    Erase { ink: Ink },
    Swipe { towards: Side },
    Open { path: PathBuf },
    OpenShell { working_dir: PathBuf },
    Tab { id: usize, msg: TabMsg },
    New,
}

#[derive(Clone)]
pub enum TabMsg {
    ShellInput { stderr: bool, content: String },
    SubmitShell,
    SaveAs { path: PathBuf },
    Undo,
    Redo,
    Save,
    Quit,
}

pub struct Meta {
    path_window: TextWindow,
    suggested: Vec<String>,
}

impl Meta {
    fn new(path_window: TextWindow) -> Meta {
        let mut new = Meta {
            path_window,
            suggested: vec![],
        };

        new.reload_suggestions();

        new
    }

    pub fn reload_suggestions(&mut self) {
        self.suggested = suggestions(&self.path_window.buffer.content_string()).unwrap_or_default()
    }
}

#[derive(Clone)]
pub enum Tab {
    Meta,
    Template,
    Edit(usize),
}

type Coord = (usize, usize);

enum TabType {
    Text(TextTab),
    Shell(ShellTab),
}

struct ShellTab {
    child: Child,
    shell_output: TextWindow,
    history: VecDeque<TextBuffer>,
}

impl ShellTab {
    pub fn new(
        id: usize,
        atlas: Rc<Atlas>,
        metrics: Metrics,
        dimensions: Coord,
        sender: Sender<Msg>,
        working_dir: PathBuf,
    ) -> io::Result<ShellTab> {
        let rcfile = match BASE_DIRS.find_config_file(BASH_RC_FILE) {
            Some(found) => found,
            None => {
                let path = BASE_DIRS.place_config_file(BASH_RC_FILE)?;
                fs::write(&path, include_str!("default.bashrc"))?;
                path
            }
        };

        let (lines, columns) = dimensions;

        // Launch a bash shell, wiring up everything.
        let mut child = process::Command::new("/bin/bash")
            .args([
                // Disables readline... we're the ones implementing editing!
                "--noediting",
                // Use our custom rcfile, which can do things like
                "--rcfile",
            ])
            .arg(rcfile.into_os_string())
            .arg(
                // Run in interactive mode. This does ~many things, like
                // enabling the prompt, and gets us closer to a normal shell.
                "-i",
            )
            .current_dir(&working_dir)
            .env("LINES", lines.to_string())
            .env("COLUMNS", columns.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        fn tail<T: Read + Send + 'static>(
            mut stream: T,
            id: usize,
            sender: Sender<Msg>,
            stderr: bool,
        ) {
            thread::spawn(move || {
                eprintln!("Tailing stream!");
                let mut buffer = [0; 1024];
                loop {
                    let read = match stream.read(&mut buffer) {
                        Ok(size) => size,
                        Err(e) => {
                            eprintln!("Error! {:?}", e);
                            break;
                        }
                    };

                    if read == 0 {
                        eprintln!("Empty read: end of stream.");
                        break;
                    }

                    // We're assuming that each chunk is itself valid utf8,
                    // which might not be true! Correct would be to use from_utf8
                    // and match on the error case to decide whether to insert a
                    // replacement char or to wait for more input.
                    let contents = String::from_utf8_lossy(&buffer[..read]);

                    sender.send(Msg::Tab {
                        id,
                        msg: TabMsg::ShellInput {
                            stderr,
                            content: contents.to_string(),
                        },
                    });
                }
                eprintln!("Thread shutting down!");
            });
        }

        tail(
            child.stdout.take().expect("taking child stdout"),
            id,
            sender.clone(),
            false,
        );
        tail(
            child.stderr.take().expect("taking child stderr"),
            id,
            sender,
            true,
        );

        Ok(ShellTab {
            child,
            shell_output: TextWindow::new(TextBuffer::empty(), atlas, metrics, dimensions),
            history: Default::default(),
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
    sender: Sender<Msg>,
    metrics: Metrics,

    error_string: String,

    atlas: Rc<Atlas>,

    // tabs
    tab: Tab,

    meta: Meta,

    // template stuff
    template_path: PathBuf,
    template_offset: usize,

    text_stuff: TextStuff,

    next_tab_id: usize,
    tabs: BTreeMap<usize, TabType>,
}

impl Editor {
    fn load_templates(&mut self) -> io::Result<()> {
        let data: TemplateFile = match File::open(&self.template_path) {
            Ok(file) => serde_json::from_reader(file)?,
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // File does not exist, which is expected on first boot.
                TemplateFile::default()
            }
            Err(e) => return Err(e),
        };

        self.text_stuff.load_from_file(data, &self.metrics);

        Ok(())
    }

    fn save_templates(&self) -> io::Result<()> {
        let file_contents = TemplateFile::new(&self.text_stuff, self.metrics.height);
        // NB: because the bulk of the data is long string content,
        // we don't pay much extra to prettify this!
        serde_json::to_writer_pretty(File::create(&self.template_path)?, &file_contents)?;
        Ok(())
    }

    fn left_margin(&self) -> i32 {
        let (_, cols) = max_dimensions(&self.metrics);
        let width = cols as i32 * self.metrics.width;
        (DISPLAYWIDTH as i32 - width) / 2
    }

    fn right_margin(&self) -> i32 {
        self.left_margin()
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

impl Widget for Editor {
    type Message = Msg;

    fn size(&self) -> Vector2<i32> {
        Vector2::new(SCREEN_WIDTH, SCREEN_HEIGHT)
    }

    fn render(&self, mut view: View<Msg>) {
        let mut header = view.split_off(Side::Top, TOP_MARGIN);
        header.split_off(Side::Left, self.left_margin());
        header.split_off(Side::Right, self.right_margin());

        match self.tab {
            Tab::Meta { .. } => {
                let head_text = Text::literal(DEFAULT_CHAR_HEIGHT, &*FONT, &*APP_NAME);
                head_text.render_split(&mut header, Side::Left, 0.5);
                Spaced(
                    40,
                    &[Button::new(
                        "templates",
                        Msg::SwitchTab { tab: Tab::Template },
                        true,
                    )],
                )
                .render_placed(header, 1.0, 0.5);
            }
            Tab::Edit(id) => {
                match &self.tabs[&id] {
                    TabType::Text(text_tab) => {
                        let path_str = text_tab
                            .path
                            .as_ref()
                            .and_then(|p| p.file_name())
                            .map(|s| s.to_string_lossy())
                            .unwrap_or(Cow::Borrowed("<unnamed file>"));

                        Button::new(&path_str, Msg::SwitchTab { tab: Tab::Meta }, true)
                            .render_split(&mut header, Side::Left, 0.5);

                        Spaced(
                            40,
                            &[
                                Button::new(
                                    "undo",
                                    Msg::Tab {
                                        id,
                                        msg: TabMsg::Undo,
                                    },
                                    !text_tab.text.undos.is_empty(),
                                ),
                                Button::new(
                                    "redo",
                                    Msg::Tab {
                                        id,
                                        msg: TabMsg::Redo,
                                    },
                                    !text_tab.text.redos.is_empty(),
                                ),
                                Button::new(
                                    "save",
                                    Msg::Tab {
                                        id,
                                        msg: TabMsg::Save,
                                    },
                                    text_tab.path.is_some() && text_tab.dirty,
                                ),
                            ],
                        )
                        .render_placed(header, 1.0, 0.5);
                    }
                    TabType::Shell(_) => {
                        let name = format!("Shell #{}", id);
                        Button::new(&name, Msg::SwitchTab { tab: Tab::Meta }, true).render_split(
                            &mut header,
                            Side::Left,
                            0.5,
                        );

                        Spaced(
                            40,
                            &[Button::new(
                                "submit",
                                Msg::Tab {
                                    id,
                                    msg: TabMsg::SubmitShell,
                                },
                                true,
                            )],
                        )
                        .render_placed(header, 1.0, 0.5);
                    }
                };
            }
            Tab::Template => {
                let head_text = Button::new("templates", Msg::SwitchTab { tab: Tab::Meta }, true);
                head_text.render_split(&mut header, Side::Left, 0.5);
                header.leave_rest_blank();
            }
        }

        {
            let mut footer = view.split_off(Side::Bottom, TOP_MARGIN);
            footer.split_off(Side::Left, self.left_margin());
            footer.split_off(Side::Right, self.right_margin());

            let mut message = match self.tab {
                Tab::Meta => "".to_string(),
                Tab::Template => "".to_string(),
                Tab::Edit(id) => {
                    let (row, col) = match &self.tabs[&id] {
                        TabType::Text(text_tab) => text_tab.text.origin,
                        TabType::Shell(shell_tab) => shell_tab.shell_output.origin,
                    };
                    format!("[{row}:{col}] ")
                }
            };

            message.push_str(&self.error_string);

            let text = Text::literal(DEFAULT_CHAR_HEIGHT, &*FONT, &message);
            text.render_placed(footer, 0.0, 0.4);
        }

        for side in [Side::Top, Side::Bottom, Side::Left, Side::Right] {
            view.handlers()
                .pad(-100)
                .on_swipe(side, Msg::Swipe { towards: side });
        }

        match &self.tab {
            Tab::Meta => {
                view.split_off(Side::Left, self.left_margin());

                self.meta
                    .path_window
                    .borrow()
                    .map(|message| match message {
                        TextMessage::Write(ink) => Msg::Write { ink },
                        TextMessage::Erase(ink) => Msg::Erase { ink },
                    })
                    .render_split(&mut view, Side::Top, 0.0);

                view.split_off(Side::Right, self.right_margin());

                let written_path: PathBuf = self.meta.path_window.buffer.content_string().into();
                let entry_height = DEFAULT_CHAR_HEIGHT * 3 / 2;

                let mut buttons = view.split_off(Side::Top, entry_height);

                let written_dir = if written_path.is_dir() {
                    written_path.clone()
                } else {
                    written_path
                        .parent()
                        .map_or(PathBuf::from("/"), |p| p.to_path_buf())
                };

                Spaced(
                    40,
                    &[
                        Button::new("new file", Msg::New, !written_path.exists()),
                        Button::new(
                            "new shell",
                            Msg::OpenShell {
                                working_dir: written_dir,
                            },
                            true,
                        ),
                    ],
                )
                .render_split(&mut buttons, Side::Right, 0.5);

                buttons.leave_rest_blank();
                view.split_off(Side::Top, entry_height);

                Text::literal(DEFAULT_CHAR_HEIGHT, &*FONT, "Tabs:").render_split(
                    &mut view,
                    Side::Top,
                    0.0,
                );

                for (tab_id, _tab) in &self.tabs {
                    match &self.tabs[tab_id] {
                        TabType::Text(tab) => {
                            let path_str = tab
                                .path
                                .as_ref()
                                .and_then(|p| p.file_name())
                                .map(|p| p.to_string_lossy())
                                .unwrap_or(Cow::Borrowed("<unnamed file>"));
                            let tab_label = Button::new(
                                &path_str,
                                Msg::SwitchTab {
                                    tab: Tab::Edit(*tab_id),
                                },
                                true,
                            );
                            let mut tab_view = view.split_off(Side::Top, entry_height);
                            tab_view.split_off(Side::Left, 20);
                            tab_label.render_split(&mut tab_view, Side::Left, 0.5);

                            Spaced(
                                40,
                                &[
                                    Button::new(
                                        "save as",
                                        Msg::Tab {
                                            id: *tab_id,
                                            msg: TabMsg::SaveAs {
                                                path: written_path.clone(),
                                            },
                                        },
                                        !written_path.exists(),
                                    ),
                                    Button::new(
                                        "close",
                                        Msg::Tab {
                                            id: *tab_id,
                                            msg: TabMsg::Quit,
                                        },
                                        true,
                                    ),
                                ],
                            )
                            .render_split(
                                &mut tab_view,
                                Side::Right,
                                0.5,
                            );
                        }
                        TabType::Shell(_shell_tab) => {
                            let name = format!("Shell #{}", tab_id);
                            let tab_label = Button::new(
                                &name,
                                Msg::SwitchTab {
                                    tab: Tab::Edit(*tab_id),
                                },
                                true,
                            );
                            let mut tab_view = view.split_off(Side::Top, entry_height);
                            tab_view.split_off(Side::Left, 20);
                            tab_label.render_split(&mut tab_view, Side::Left, 0.5);
                            Spaced(
                                40,
                                &[Button::new(
                                    "close",
                                    Msg::Tab {
                                        id: *tab_id,
                                        msg: TabMsg::Quit,
                                    },
                                    true,
                                )],
                            )
                            .render_split(
                                &mut tab_view,
                                Side::Right,
                                0.5,
                            );
                        }
                    }
                }

                view.split_off(Side::Top, entry_height);

                Text::literal(DEFAULT_CHAR_HEIGHT, &*FONT, "Paths:").render_split(
                    &mut view,
                    Side::Top,
                    0.0,
                );

                for s in &self.meta.suggested {
                    if view.size().y < entry_height {
                        break;
                    }
                    let mut suggest_view = view.split_off(Side::Top, entry_height);
                    suggest_view.split_off(Side::Left, 20);

                    let msg = if s.ends_with('/') {
                        Msg::MetaPath {
                            current_path: s.clone(),
                        }
                    } else {
                        Msg::Open {
                            path: PathBuf::from(s),
                        }
                    };

                    Button::new(s, msg, true).render_split(&mut suggest_view, Side::Left, 0.5);
                }
            }
            Tab::Edit(id) => {
                match &self.tabs[id] {
                    TabType::Text(text_tab) => {
                        // Run the line numbers down the margin!
                        let mut margin_view = view.split_off(Side::Left, self.left_margin());
                        margin_view.split_off(Side::Right, 20);
                        // Based on the top margin of the text area and the baseline height.
                        // TODO: calculate this from other metrics.
                        margin_view.split_off(Side::Top, 7);
                        for row in (text_tab.text.origin.0..).take(text_tab.text.dimensions.0) {
                            let view =
                                margin_view.split_off(Side::Top, text_tab.text.grid_metrics.height);
                            let text = Text::literal(
                                text_tab.text.grid_metrics.height * 3 / 4,
                                &*FONT,
                                &format!("{}", row),
                            );
                            text.render_placed(view, 1.0, 1.0);
                        }
                        margin_view.leave_rest_blank();

                        text_tab
                            .text
                            .borrow()
                            .map(|message| match message {
                                TextMessage::Write(ink) => Msg::Write { ink },
                                TextMessage::Erase(ink) => Msg::Erase { ink },
                            })
                            .render_split(&mut view, Side::Top, 0.0);
                    }
                    TabType::Shell(shell_tab) => {
                        view.split_off(Side::Left, self.left_margin());
                        shell_tab
                            .shell_output
                            .borrow()
                            .map(|message| match message {
                                TextMessage::Write(ink) => Msg::Write { ink },
                                TextMessage::Erase(ink) => Msg::Erase { ink },
                            })
                            .render_split(&mut view, Side::Top, 0.0);
                    }
                }
            }
            Tab::Template => {
                let mut margin_view = view.split_off(Side::Left, self.left_margin());
                let margin_placement = self.metrics.baseline as f32 / self.metrics.height as f32;
                margin_view.split_off(
                    Side::Top,
                    self.metrics.height - self.metrics.baseline + GRID_BORDER,
                );
                for ct in self.text_stuff.templates[self.template_offset..]
                    .iter()
                    .take(self.max_dimensions().0)
                {
                    let mut view = margin_view.split_off(Side::Top, self.metrics.height);
                    view.split_off(Side::Right, 20);
                    let text = Text::literal(self.metrics.height, &*FONT, &format!("{}", ct.char));
                    text.render_placed(view, 1.0, margin_placement);
                }
                margin_view.leave_rest_blank();

                let (height, width) = self.max_dimensions();
                let height = height.min(self.text_stuff.templates.len() - self.template_offset);

                draw_grid(
                    view,
                    &self.metrics,
                    (height, width),
                    |view| {
                        view.handlers().pad(8).on_ink(|ink| Msg::Write { ink });
                    },
                    |row, col, mut template_view| {
                        let row = self.template_offset + row;
                        let maybe_char = self.text_stuff.templates.get(row);
                        let grid =
                            self.atlas
                                .get_cell(GridCell::new(&self.metrics, None, false, true));
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

const NUM_SUGGESTIONS: usize = 32;
const MAX_DIR_ENTRIES: usize = 1024;

fn full_path(path: &Path) -> Option<String> {
    let mut string = path.to_str()?.to_string();
    if path.is_dir() {
        string.push('/');
    }
    Some(string)
}

fn suggestions(current_path: &str) -> io::Result<Vec<String>> {
    if !current_path.starts_with('/') {
        // All paths must be absolute.
        return Ok(vec![]);
    }
    let (dir, file) = current_path.rsplit_once('/').expect("splitting /path by /");
    let dir = if dir.is_empty() { "/" } else { dir };
    let read = fs::read_dir(dir)?;
    let mut results: Vec<_> = read
        .filter_map(|r| r.ok())
        .filter(|de| {
            de.file_name()
                .to_str()
                .into_iter()
                .any(|s| s.starts_with(file))
        })
        .take(MAX_DIR_ENTRIES)
        .filter_map(|s| full_path(&s.path()))
        .collect();

    // NB: if this is slow, pull in the partial sort crate.
    results.sort();
    results.truncate(NUM_SUGGESTIONS);

    Ok(results)
}

fn max_dimensions(metrics: &Metrics) -> Coord {
    let rows = (SCREEN_HEIGHT - TOP_MARGIN * 2 - GRID_BORDER * 2) / metrics.height;
    let cols = (SCREEN_WIDTH - LEFT_MARGIN * 2 - GRID_BORDER * 2) / metrics.width;
    (rows as usize, cols as usize)
}

impl Editor {
    fn max_dimensions(&self) -> Coord {
        max_dimensions(&self.metrics)
    }

    fn take_id(&mut self) -> usize {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        id
    }

    fn template_at(&mut self, coord: Coord) -> &mut Template {
        let (row, col) = coord;
        let row = row + self.template_offset;
        let ct = &mut self.text_stuff.templates[row];
        if col >= ct.templates.len() {
            ct.templates
                .resize_with(col + 1, || Template::from_ink(Ink::new()));
        }
        &mut ct.templates[col]
    }

    fn new_text_tab(&mut self, path: Option<PathBuf>, contents: TextBuffer) {
        let id = self.take_id();
        self.tabs.insert(
            id,
            TabType::Text(TextTab {
                path,
                text: TextWindow::new(
                    contents,
                    self.atlas.clone(),
                    self.metrics.clone(),
                    self.max_dimensions(),
                ),
                dirty: false,
            }),
        );
        self.tab = Tab::Edit(id)
    }
}

impl Applet for Editor {
    type Upstream = ();

    fn update(&mut self, message: Self::Message) -> Option<Self::Upstream> {
        match message {
            Msg::Write { ink, .. } => match &mut self.tab {
                Tab::Meta => {
                    if let Some(ink_type) =
                        InkType::classify(&self.metrics, ink, &self.meta.path_window.selection())
                    {
                        self.meta
                            .path_window
                            .ink_row(ink_type, &mut self.text_stuff);
                        self.meta.suggested =
                            suggestions(&self.meta.path_window.buffer.content_string())
                                .unwrap_or_default();
                    }
                }
                Tab::Edit(id) => match self.tabs.get_mut(id).unwrap() {
                    TabType::Text(text_tab) => {
                        if let Some(ink_type) =
                            InkType::classify(&self.metrics, ink, &text_tab.text.selection())
                        {
                            text_tab.dirty = true;
                            text_tab.text.ink_row(ink_type, &mut self.text_stuff);
                        }
                    }

                    TabType::Shell(shell_tab) => {
                        if let Some(ink_type) = InkType::classify(
                            &self.metrics,
                            ink,
                            &shell_tab.shell_output.selection(),
                        ) {
                            shell_tab
                                .shell_output
                                .ink_row(ink_type, &mut self.text_stuff);
                        }
                    }
                },
                Tab::Template => {
                    if let Some(ink_type) =
                        InkType::classify(&self.metrics, ink, &Selection::Normal)
                    {
                        match ink_type {
                            InkType::Strikethrough { start, end } => {
                                if start.0 == end.0 {
                                    for col in start.1..end.1 {
                                        self.template_at((start.0, col)).clear();
                                    }
                                }
                            }
                            InkType::Scratch { at } => {
                                self.template_at(at).clear();
                            }
                            InkType::Glyphs { tokens } => {
                                for (coord, ink) in tokens {
                                    let tpl = self.template_at(coord);
                                    tpl.ink.append(ink, 0.5);
                                    tpl.serialized = tpl.ink.to_string();
                                }
                            }
                            _ => {}
                        }
                    }
                }
            },
            Msg::Erase { ink } => match self.tab {
                Tab::Meta => {
                    self.meta.path_window.erase(ink);
                }
                Tab::Template => {
                    // TODO: something about this?
                }
                Tab::Edit(id) => match self.tabs.get_mut(&id) {
                    Some(TabType::Text(tab)) => {
                        tab.text.erase(ink);
                    }
                    Some(TabType::Shell(tab)) => {
                        tab.shell_output.erase(ink);
                    }
                    _ => {}
                },
            },
            Msg::SwitchTab { tab } => {
                if matches!(self.tab, Tab::Template) {
                    self.report_error(self.save_templates());
                    self.text_stuff.init_recognizer(&self.metrics);
                }
                self.error_string.clear();
                self.tab = tab;
            }
            Msg::Swipe { towards } => match self.tab {
                // TODO: abstract over the pattern here.
                Tab::Edit(id) => {
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
                        TabType::Shell(shell_tab) => {
                            shell_tab.shell_output.page_relative(movement);
                        }
                    }
                }
                Tab::Template => {
                    let (rows, _) = self.max_dimensions();
                    match towards {
                        Side::Top => {
                            if self.template_offset + rows < self.text_stuff.templates.len() {
                                self.template_offset += rows - 1;
                            }
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
                // If we reopen meta, we're likely to want another file in the same dir.
                if let Some(dir) = path.parent().and_then(full_path) {
                    self.meta.path_window.buffer = TextBuffer::from_string(&dir);
                    self.meta.reload_suggestions();
                }

                if let Some(file_contents) = self.report_error(fs::read_to_string(&path)) {
                    self.new_text_tab(Some(path), TextBuffer::from_string(&file_contents));
                }
            }
            Msg::New => {
                self.new_text_tab(None, TextBuffer::empty());
                self.error_string.clear();
            }
            Msg::MetaPath { current_path } => {
                self.meta.path_window.buffer = TextBuffer::from_string(&current_path);
                self.meta.reload_suggestions();
                self.tab = Tab::Meta;
            }
            Msg::OpenShell { working_dir } => {
                let id = self.take_id();
                let shell = ShellTab::new(
                    id,
                    self.atlas.clone(),
                    self.metrics.clone(),
                    self.max_dimensions(),
                    self.sender.clone(),
                    working_dir,
                )
                .unwrap();
                self.tabs.insert(id, TabType::Shell(shell));
                self.tab = Tab::Edit(id);
            }
            Msg::Tab {
                id,
                msg: TabMsg::Quit,
            } => {
                self.tabs.remove(&id);
            }
            Msg::Tab { id, msg } => {
                if let Some(tab) = self.tabs.get_mut(&id) {
                    match (msg, tab) {
                        (TabMsg::ShellInput { stderr: _, content }, TabType::Shell(shell_tab)) => {
                            // TODO: visual marker of stderr lines? do we care?
                            let content_buffer = TextBuffer::from_string(&content);
                            let content_size = content_buffer.end();
                            shell_tab.shell_output.replace(Replace::splice(
                                shell_tab.shell_output.frozen_until,
                                content_buffer,
                            ));
                            shell_tab.shell_output.undos.clear();
                            shell_tab.shell_output.frozen_until =
                                add_coord(shell_tab.shell_output.frozen_until, content_size);
                        }
                        (TabMsg::SubmitShell, TabType::Shell(shell_tab)) => {
                            shell_tab.shell_output.replace(Replace::splice(
                                shell_tab.shell_output.buffer.end(),
                                TextBuffer::from_string("\n"),
                            ));
                            let buffer = shell_tab.shell_output.buffer.copy(
                                shell_tab.shell_output.frozen_until,
                                shell_tab.shell_output.buffer.end(),
                            );
                            let command = buffer.content_string();
                            if let Some(stdin) = &mut shell_tab.child.stdin {
                                if let Err(e) = stdin.write(command.as_bytes()) {
                                    self.error_string = e.to_string();
                                }
                            }
                            shell_tab.history.push_back(buffer);
                            shell_tab.shell_output.frozen_until =
                                shell_tab.shell_output.buffer.end();
                        }
                        (TabMsg::SaveAs { path }, TabType::Text(text_tab)) => {
                            if !path.exists() && path.parent().iter().any(|p| p.is_dir()) {
                                text_tab.path = Some(path);
                                let saved = text_tab.save();
                                if self.report_error(saved).is_some() {
                                    self.tab = Tab::Edit(id)
                                };
                            }
                        }
                        (TabMsg::Undo, TabType::Text(text_tab)) => {
                            text_tab.text.undo();
                            text_tab.dirty = true;
                        }
                        (TabMsg::Redo, TabType::Text(text_tab)) => {
                            text_tab.text.redo();
                            text_tab.dirty = true;
                        }
                        (TabMsg::Save, TabType::Text(text_tab)) => {
                            let result = text_tab.save();
                            self.report_error(result);
                        }
                        _ => {}
                    }
                } else {
                    // TODO: log?
                }
            }
        }

        None
    }

    fn current_route(&self) -> &str {
        match self.tab {
            Tab::Meta => "meta",
            Tab::Edit { .. } => "edit",
            Tab::Template => "template",
        }
    }
}

fn main() -> anyhow::Result<()> {
    let mut app = app::App::new();

    let template_path = BASE_DIRS.place_data_file(TEMPLATE_FILE)?;

    let config: Config = {
        let config_path = BASE_DIRS.place_config_file(CONFIG_FILE)?;

        let config_str = match fs::read(&config_path) {
            Ok(file) => Cow::Owned(file),
            Err(e) if e.kind() == ErrorKind::NotFound => {
                // File does not exist, which is expected on first boot.
                let bytes: &[u8] = include_bytes!("sill.toml");
                fs::write(config_path, bytes)?;
                Cow::Borrowed(bytes)
            }
            Err(e) => Err(e)?,
        };

        toml::from_slice(&config_str)?
    };

    let metrics = Metrics::new(config.cell_height.clamp(20, 80));

    let atlas = Rc::new(Atlas::new());

    let max_dimensions = max_dimensions(&metrics);

    let meta_path = env::var_os("HOME")
        .and_then(|os| full_path(Path::new(&os)))
        .unwrap_or_else(|| "/".to_string());

    let meta = Meta::new(TextWindow::new(
        TextBuffer::from_string(&meta_path),
        atlas.clone(),
        metrics.clone(),
        (1, max_dimensions.1),
    ));

    let mut component = Component::with_sender(app.wakeup(), |sender| {
        let mut widget = Editor {
            sender,
            template_path,
            metrics: metrics.clone(),
            error_string: "".to_string(),
            atlas: atlas.clone(),
            tab: Tab::Meta,
            template_offset: 0,
            text_stuff: TextStuff::new(),
            next_tab_id: 0,
            tabs: BTreeMap::new(),
            meta,
        };

        let load_result = widget.load_templates();
        widget.report_error(load_result);

        widget.new_text_tab(None, TextBuffer::from_string(HELP_TEXT));

        widget
    });

    app.run(&mut component);

    Ok(())
}

#[cfg(test)]
mod test {
    use crate::Config;

    #[test]
    fn test_default_config() {
        let conf = include_str!("sill.toml");
        let conf: Config = toml::from_str(&conf).expect("loading known_valid config");
        assert_eq!(conf, Config::default())
    }
}
