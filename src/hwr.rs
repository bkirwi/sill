use crate::{Metrics, Msg, FONT};
use armrest::dollar::Points;
use armrest::ink::Ink;
use armrest::ui::Text;
use serde::Deserialize;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::BTreeMap;

// A set of characters that we always include in the template, even when not explicitly configured.
pub const PRINTABLE_ASCII: &str = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

#[derive(Serialize, Deserialize)]
pub struct TemplateFile<'a> {
    templates: BTreeMap<char, Vec<Cow<'a, str>>>,
}

impl<'a> TemplateFile<'a> {
    pub fn new(templates: &'a [CharData]) -> TemplateFile<'a> {
        let mut entries = BTreeMap::new();
        for ts in templates {
            let strings: Vec<Cow<str>> = ts
                .templates
                .iter()
                .map(|t| Cow::Borrowed(t.serialized.as_ref()))
                .collect();
            if !strings.is_empty() {
                entries.insert(ts.char, strings);
            }
        }
        TemplateFile { templates: entries }
    }

    pub fn to_templates(mut self, size: i32) -> Vec<CharData> {
        let char_data = |ch: char, strings: Vec<Cow<str>>| CharData {
            char: ch,
            label: Text::literal(size, &*FONT, &format!("{}", ch)),
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
    fn from_ink(ink: Ink) -> Template {
        let serialized = ink.to_string();
        Template { ink, serialized }
    }

    fn from_string(serialized: String) -> Template {
        let ink = Ink::from_string(&serialized);
        Template { ink, serialized }
    }
}

pub struct CharData {
    pub char: char,
    pub label: Text<Msg>,
    pub templates: Vec<Template>,
}

pub struct CharRecognizer {
    templates: Vec<Points>,
    chars: Vec<char>,
    metrics: Metrics,
}

impl CharRecognizer {
    pub fn new(input: &[CharData], metrics: &Metrics) -> CharRecognizer {
        let mut templates = vec![];
        let mut chars = vec![];
        for ts in input {
            for template in &ts.templates {
                if template.ink.len() == 0 {
                    continue;
                }
                templates.push(crate::ink_to_points(&template.ink, metrics));
                chars.push(ts.char);
            }
        }
        CharRecognizer {
            templates,
            chars,
            metrics: metrics.clone(),
        }
    }

    pub fn best_match(&self, ink: &Ink, threshold: f32) -> Option<char> {
        if self.templates.is_empty() {
            return None;
        }

        let query = crate::ink_to_points(ink, &self.metrics);
        let (best, score) = query.recognize(&self.templates);
        if score > threshold {
            None
        } else {
            Some(self.chars[best])
        }
    }

    pub fn promote(&mut self, index: usize) {
        if index == 0 || index >= self.templates.len() {
            return;
        }
        self.templates.swap(0, index);
        self.chars.swap(0, index)
    }

    fn look_for_trouble(&mut self) {
        let count = self.templates.len();
        if count < 2 {
            return;
        }
        for i in 0..count {
            self.promote(i);
            let index = self.templates[0].recognize(&self.templates[1..]).0 + 1;
            let expected = self.chars[0];
            let actual = self.chars[index];
            if expected != actual {
                eprintln!(
                    "Yikes! Closest match for a {} is actually {}",
                    expected, actual
                );
            }
        }
    }
}
