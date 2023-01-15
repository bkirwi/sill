use crate::{config::Config, font::Metrics, text_buffer::TextBuffer};
use armrest::dollar::Points;
use armrest::ink::Ink;

use crate::util::rotate_queue;
use serde::Deserialize;
use serde::Serialize;
use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::{BTreeMap, VecDeque};

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

    let mut max = f32::MIN;
    let mut min = f32::MAX;
    for p in points.points() {
        max = max.max(p.y);
        min = min.min(p.y);
    }

    fn clamp(value: f32, baseline: f32, height: f32) -> f32 {
        let grid = height / 8.0;
        let value = value - baseline;
        let value = (value / grid).floor() * grid;
        value + baseline
    }
    let max = clamp(max, metrics.baseline as f32, metrics.height as f32);
    let min = clamp(min, metrics.baseline as f32, metrics.height as f32);
    let average = (max + min) / 2.0;

    let mut center = points.centroid();
    center.y -= average;
    center.y += metrics.height as f32 / 2.0;
    points.recenter_on(center);

    points.scale_by(1.0 / metrics.width as f32);

    points
}

pub fn default_char_height() -> i32 {
    40
}

/// The format of the template file, used only for persistence. The cows allow us to save some
/// copying when writing the file; see `new` for the borrow.
#[derive(Serialize, Deserialize)]
pub struct TemplateFile<'a> {
    #[serde(default = "default_char_height")]
    template_height: i32,
    templates: BTreeMap<char, Vec<Cow<'a, str>>>,
    #[serde(default)]
    candidate_templates: Vec<TemplateFileEntry<'a>>,
}

#[derive(Serialize, Deserialize)]
pub struct TemplateFileEntry<'a> {
    char: char,
    ink: Cow<'a, str>,
}

impl<'a> Default for TemplateFile<'a> {
    fn default() -> Self {
        let bytes = include_bytes!("templates.json");
        serde_json::from_slice(bytes).expect("parsing known-correct template.json")
    }
}

impl<'a> TemplateFile<'a> {
    pub fn new(stuff: &'a TextStuff, template_height: i32) -> TemplateFile<'a> {
        let mut entries = BTreeMap::new();
        for ts in &stuff.templates {
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

        let candidate_templates: Vec<TemplateFileEntry<'a>> = stuff
            .candidate_templates
            .iter()
            .map(|(t, _, c)| TemplateFileEntry {
                char: *c,
                ink: Cow::Borrowed(&t.serialized),
            })
            .collect();

        TemplateFile {
            template_height,
            templates: entries,
            candidate_templates,
        }
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

const NUM_CANDIDATES: usize = 64;

pub struct TextStuff {
    pub templates: Vec<CharTemplates>,
    pub char_recognizer: CharRecognizer,
    pub big_recognizer: CharRecognizer,
    pub clipboard: Option<TextBuffer>,
    pub candidate_templates: VecDeque<(Template, Points, char)>,
}

impl TextStuff {
    pub fn new() -> TextStuff {
        TextStuff {
            templates: vec![],
            char_recognizer: CharRecognizer::new([]),
            big_recognizer: CharRecognizer::new([]),
            clipboard: None,
            candidate_templates: VecDeque::new(),
        }
    }

    pub fn load_from_file(
        &mut self,
        template_file: TemplateFile,
        metrics: &Metrics,
        config: &Config,
    ) {
        let TemplateFile {
            template_height,
            mut templates,
            candidate_templates,
        } = template_file;

        let scale = if template_height == metrics.height {
            None
        } else {
            // if metrics.height is larger than the height of the serialized templates, scale up!
            Some(metrics.height as f32 / template_height as f32)
        };

        let parse_template = |string: Cow<'_, str>| match scale {
            None => Template::from_string(string.into_owned()),
            Some(scale) => {
                let original = Ink::from_string(&string);
                let mut ink = Ink::new();
                for stroke in original.strokes() {
                    for p in stroke {
                        ink.push(p.x * scale, p.y * scale, p.z);
                    }
                    ink.pen_up();
                }
                Template::from_ink(ink)
            }
        };

        let char_data = |ch: char, strings: Vec<Cow<str>>| CharTemplates {
            char: ch,
            templates: strings.into_iter().map(parse_template).collect(),
        };

        let mut new_templates: Vec<CharTemplates> = vec![];

        for ch in PRINTABLE_ASCII.chars().chain(config.extra_chars()) {
            // TODO: avoid the quadratic behaviour here.
            if new_templates.iter().any(|t| t.char == ch) {
                continue;
            }
            new_templates.push(char_data(ch, templates.remove(&ch).unwrap_or_default()))
        }

        for (ch, strings) in templates {
            new_templates.push(char_data(ch, strings));
        }

        self.templates = new_templates;

        self.candidate_templates = candidate_templates
            .into_iter()
            .map(|ct| {
                let template = parse_template(ct.ink);
                let points = ink_to_points(&template.ink, metrics);
                (template, points, ct.char)
            })
            .collect();

        self.init_recognizer(metrics);
    }

    pub fn on_overwrite(&mut self, ink: Ink, points: Points, best: char) {
        if self.char_recognizer.templates.is_empty() {
            return;
        }

        let (index, old_score) = points.recognize(&self.char_recognizer.templates);
        let old_char = self.char_recognizer.chars[index];
        if old_char == best {
            // A bit surprising: we seem to predict this correctly now.
            // Maybe we've already added a better template?
            return;
        }

        // The candidate for the same char that most improves on the score, if any.
        let better_match = self
            .candidate_templates
            .iter()
            .enumerate()
            .map(|(i, (_, p, _))| (i, points.distance(p, old_score)))
            .filter(|(_, score)| *score < old_score)
            .min_by(|(_, l_score), (_, r_score)| {
                l_score.partial_cmp(r_score).unwrap_or(Ordering::Equal)
            });

        if let Some((index, score)) = better_match {
            let (template, _, candidate_char) = self
                .candidate_templates
                .remove(index)
                .expect("Removing just-found index.");
            if candidate_char == best {
                // Positive reinforcement! Promote to a template.
                dbg!("promote", best, old_score, score);
                if let Some(ct) = self.templates.iter_mut().find(|ct| ct.char == best) {
                    ct.templates.push(template);
                    // TODO: automatically reinit the recognizers?
                }
            } else {
                // Negative reinforcement! Get rid of the candidate.
                dbg!("demote", best, candidate_char, old_score, score);
            }
        } else {
            // This might be a good candidate for a template; track it.
            dbg!("consider", best, old_score);
            if let Some((_, _, rotated_out)) = rotate_queue(
                &mut self.candidate_templates,
                (Template::from_ink(ink), points, best),
                NUM_CANDIDATES,
            ) {
                eprintln!("Rotated out template for char `{rotated_out}`; never used.");
            }
        }
    }

    pub fn init_recognizer(&mut self, metrics: &Metrics) {
        // Discard trivial or invalid templates.
        for ct in &mut self.templates {
            ct.templates.retain(|t| t.ink.len() > 1);
        }
        self.char_recognizer = CharRecognizer::new(self.templates.iter().flat_map(|ct| {
            let c = ct.char;
            ct.templates
                .iter()
                .map(move |t| (ink_to_points(&t.ink, metrics), c))
        }));
        self.big_recognizer = CharRecognizer::new(
            self.templates
                .iter()
                .filter(|ct| ['X', 'C', 'V', 'S', '>', '<', 'Q', 'N', 'P'].contains(&ct.char))
                .flat_map(|ct| {
                    let c = ct.char;
                    ct.templates
                        .iter()
                        .filter(|t| t.ink.len() > 1)
                        .map(move |t| (Points::normalize(&t.ink), c))
                }),
        );
    }
}
