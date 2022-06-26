use crate::grid_ui::Coord;
use crate::{Metrics, Vector2};
use armrest::ink::Ink;
use std::collections::HashMap;

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
#[derive(Debug)]
pub enum InkType {
    // A horizontal strike through the current line: typically, delete.
    Strikethrough { start: Coord, end: Coord },
    // A scratch-out of a single cell: typically, replace with whitespace.
    Scratch { at: Coord },
    // Something that appears to be one or more characters.
    Glyphs { tokens: Vec<(Coord, Ink)> },
    // A line between characters; typically represents an insertion point.
    Carat { at: Coord, ink: Ink },
}

impl InkType {
    pub fn tokenize(metrics: &Metrics, ink: &Ink) -> HashMap<usize, Ink> {
        // Idea: if the center of a stroke is ~this close to the margin, it's ambiguous,
        // and we decide which cell it belongs to by looking at where the neigbouring unambiguous
        // strokes end up.
        const LIMINAL_SPACE: f32 = 0.2;

        let strokes: Vec<_> = ink
            .strokes()
            .map(|s| {
                let mut i = Ink::new();
                for p in s {
                    i.push(p.x, p.y, p.z);
                }
                i.pen_up();
                i
            })
            .collect();

        let mut index_to_time_range = HashMap::new();
        for stroke in &strokes {
            let center = (stroke.centroid().x / metrics.width as f32).max(0.0);
            if (center - center.round()).abs() > LIMINAL_SPACE {
                let index = center as usize;
                let (min, max) = index_to_time_range
                    .entry(index)
                    .or_insert((f32::INFINITY, f32::NEG_INFINITY));
                *min = min.min(stroke.t_range.min);
                *max = max.max(stroke.t_range.max);
            }
        }

        let mut index_to_ink: HashMap<usize, Ink> = HashMap::new();
        for stroke in strokes {
            let center = (stroke.centroid().x / metrics.width as f32).max(0.0);
            let index = if (center - center.round()).abs() > LIMINAL_SPACE {
                center as usize
            } else {
                let right = center.round() as usize;
                if right == 0 {
                    0
                } else {
                    let left = right - 1;
                    match (
                        index_to_time_range.get(&left),
                        index_to_time_range.get(&right),
                    ) {
                        (None, None) => center as usize,
                        (Some(_), None) => left,
                        (None, Some(_)) => right,
                        (Some((_, left_max)), Some((right_min, _))) => {
                            let left_d = stroke.t_range.min - left_max;
                            let right_d = right_min - stroke.t_range.max;
                            if left_d < right_d {
                                left
                            } else {
                                right
                            }
                        }
                    }
                }
            };
            index_to_ink.entry(index).or_default().append(
                stroke.translate(-Vector2::new(index as f32 * metrics.width as f32, 0.0)),
                f32::MAX,
            );
        }

        index_to_ink
    }

    pub fn classify(metrics: &Metrics, ink: Ink) -> Option<InkType> {
        if ink.len() == 0 {
            return None;
        }

        let row = (ink.centroid().y / metrics.height as f32).max(0.0) as usize;
        let ink = ink.translate(-Vector2::new(0.0, row as f32 * metrics.height as f32));

        let min_x = ink.x_range.min / metrics.width as f32;
        let max_x = ink.x_range.max / metrics.width as f32;
        let min_y = ink.y_range.min / metrics.height as f32;
        let max_y = ink.y_range.max / metrics.height as f32;

        // Roughly: a strikethrough should be a single stroke that's mostly horizontal.
        if (max_x - min_x) > 1.5 && ink.strokes().count() == 1 {
            if ink.ink_len() / (ink.x_range.max - ink.x_range.min) < 1.2 {
                let start = min_x.round().max(0.0) as usize;
                let end = max_x.round().max(0.0) as usize;
                return Some(InkType::Strikethrough {
                    start: (row, start),
                    end: (row, end),
                });
            } else {
                // TODO: could just be a single char!
                // Maybe fall through and handle this case as part of char splitting?
                return None;
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
            return Some(InkType::Carat {
                at: (row, center.round() as usize),
                ink: ink.translate(-Vector2::new(center.round() * metrics.width as f32, 0.0)),
            });
        }

        if center < 0.0 {
            // Out of bounds!
            return None;
        }

        if is_erase(&ink) {
            let col = center as usize;
            return Some(InkType::Scratch { at: (row, col) });
        }

        let mut tokens: Vec<_> = Self::tokenize(metrics, &ink)
            .into_iter()
            .map(|(c, v)| ((row, c), v))
            .collect();
        tokens.sort_by_key(|(k, _)| *k);
        Some(InkType::Glyphs { tokens })
    }
}
