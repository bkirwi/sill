use crate::{Metrics, TextBuffer};
use armrest::dollar::Points;
use armrest::ink::Ink;

use serde::Deserialize;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::BTreeMap;

/// A set of characters that we always include in the template, even when not explicitly configured.
/// Aside from being very common, this lets us use these characters in other places; eg. allowing
/// the user to enter a character by writing out the code point.
pub const PRINTABLE_ASCII: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz.!\"#$%&'()*+,-/:;<=>?@[\\]^_`{|}~";

/// Convert an ink to a point cloud.
///
/// This differs from the suggested behaviour for $P, since it recenters and scales based on a
/// bounding box instead of the data itself. This is important for textual data, since the only
/// difference between an apostrophe and a comma is the position in the grid.
pub fn ink_to_points(ink: &Ink, metrics: &Metrics) -> Points {
    let mut points = Points::resample(ink);

    let mut center = points.centroid();
    center.y = metrics.height as f32 / 2.0;
    points.recenter_on(center);

    points.scale_by(1.0 / metrics.width as f32);

    points
}

/// The format of the template file, used only for persistence. The cows allow us to save some
/// copying when writing the file; see `new` for the borrow.
#[derive(Serialize, Deserialize)]
pub struct TemplateFile<'a> {
    templates: BTreeMap<char, Vec<Cow<'a, str>>>,
}

impl<'a> TemplateFile<'a> {
    pub fn new(templates: &'a [CharTemplates]) -> TemplateFile<'a> {
        let mut entries = BTreeMap::new();
        for ts in templates {
            let strings: Vec<Cow<str>> = ts
                .templates
                .iter()
                .filter(|t| !t.serialized.is_empty())
                .map(|t| Cow::Borrowed(t.serialized.as_ref()))
                .collect();

            if !strings.is_empty() {
                entries.insert(ts.char, strings);
            }
        }
        TemplateFile { templates: entries }
    }

    pub fn to_templates(mut self, _size: i32) -> Vec<CharTemplates> {
        let char_data = |ch: char, strings: Vec<Cow<str>>| CharTemplates {
            char: ch,
            templates: strings
                .into_iter()
                .map(|s| Template::from_string(s.into_owned()))
                .collect(),
        };

        let mut result = vec![];

        for ch in PRINTABLE_ASCII.chars() {
            result.push(char_data(
                ch,
                self.templates.remove(&ch).unwrap_or_default(),
            ))
        }

        for (ch, strings) in self.templates {
            result.push(char_data(ch, strings));
        }

        result
    }
}

/// Why both? We don't want to constantly lose precision reserializing.
pub struct Template {
    pub ink: Ink,
    pub serialized: String,
}

impl Template {
    pub fn from_ink(ink: Ink) -> Template {
        let serialized = ink.to_string();
        Template { ink, serialized }
    }

    pub fn from_string(serialized: String) -> Template {
        let ink = Ink::from_string(&serialized);
        Template { ink, serialized }
    }

    pub fn clear(&mut self) {
        self.ink.clear();
        self.serialized.clear();
    }
}

/// All the templates that correspond to a particular char, plus any metadata.
pub struct CharTemplates {
    pub char: char,
    pub templates: Vec<Template>,
}

pub struct CharRecognizer {
    templates: Vec<Points>,
    chars: Vec<char>,
}

impl CharRecognizer {
    pub fn new(input: impl IntoIterator<Item = (Points, char)>) -> CharRecognizer {
        let mut templates = vec![];
        let mut chars = vec![];
        for (p, c) in input {
            templates.push(p);
            chars.push(c);
        }
        CharRecognizer { templates, chars }
    }

    pub fn best_match(&mut self, query: &Points, threshold: f32) -> Option<char> {
        if self.templates.is_empty() {
            return None;
        }

        let (best, score) = query.recognize(&self.templates);
        // Put good matches at the beginning of the vec. This makes matching faster:
        // if we find a good match early on, we can abandon bad ones sooner.
        self.promote(best);
        if score > threshold {
            None
        } else {
            Some(self.chars[0])
        }
    }

    pub fn promote(&mut self, index: usize) {
        if index == 0 || index >= self.templates.len() {
            return;
        }
        self.templates.swap(0, index);
        self.chars.swap(0, index)
    }
}

pub struct TextStuff {
    pub templates: Vec<CharTemplates>,
    pub char_recognizer: CharRecognizer,
    pub big_recognizer: CharRecognizer,
    pub clipboard: Option<TextBuffer>,
}

impl TextStuff {
    pub fn new() -> TextStuff {
        TextStuff {
            templates: vec![],
            char_recognizer: CharRecognizer::new([]),
            big_recognizer: CharRecognizer::new([]),
            clipboard: None,
        }
    }

    pub fn init_recognizer(&mut self, metrics: &Metrics) {
        self.char_recognizer = CharRecognizer::new(self.templates.iter().flat_map(|ct| {
            let c = ct.char;
            ct.templates
                .iter()
                .map(move |t| (ink_to_points(&t.ink, metrics), c))
        }));
        self.big_recognizer = CharRecognizer::new(
            self.templates
                .iter()
                .filter(|ct| ['X', 'C', 'V', 'S'].contains(&ct.char))
                .flat_map(|ct| {
                    let c = ct.char;
                    ct.templates
                        .iter()
                        .map(move |t| (Points::normalize(&t.ink), c))
                }),
        );
    }
}
