use crate::*;
use armrest::dollar::Points;
use armrest::ink::Ink;
use armrest::ui::{View, Widget};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::mem;
use std::rc::Rc;

const NUM_RECENT_RECOGNITIONS: usize = 16;

pub enum TextMessage {
    Write(Ink),
}

#[derive(Clone)]
pub struct TextWindow {
    pub buffer: TextBuffer,
    atlas: Rc<Atlas>,
    pub grid_metrics: Metrics,
    selection: Selection,
    pub dimensions: Coord,
    pub origin: Coord,
    pub frozen_until: Coord,
    pub undos: VecDeque<Replace>,
    tentative_recognitions: VecDeque<Recognition>,
}

impl TextWindow {
    pub fn new(
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
            frozen_until: (0, 0),
            undos: VecDeque::new(),
            tentative_recognitions: VecDeque::new(),
        }
    }

    pub fn page_relative(&mut self, (row_d, col_d): (isize, isize)) {
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

    pub fn replace(&mut self, mut replace: Replace) {
        // Avoid editing the frozen section of the buffer.
        if self.frozen_until > replace.until {
            return;
        } else if self.frozen_until >= replace.from {
            let after = replace
                .content
                .split_off(diff_coord(self.frozen_until, replace.from));
            replace.from = self.frozen_until;
            replace.content = after;
        }

        let from = replace.from;
        let old_until = replace.until;
        let undo = self.buffer.replace(replace);
        let new_until = undo.until;
        self.undos.push_front(undo);
        self.tentative_recognitions.retain_mut(|r| {
            if r.coord < from {
                true
            } else if r.coord < old_until {
                false
            } else {
                let diff = diff_coord(old_until, r.coord);
                r.coord = add_coord(new_until, diff);
                true
            }
        });
    }

    pub fn undo(&mut self) {
        if let Some(undo) = self.undos.pop_front() {
            self.replace(undo);
            self.undos.pop_front(); // TODO: shift to a redo stack.
        }
    }

    fn relative(&self, coord: Coord) -> Coord {
        (self.origin.0 + coord.0, self.origin.1 + coord.1)
    }

    pub fn ink_row(&mut self, ink_type: InkType, text_stuff: &mut TextStuff) {
        match ink_type {
            InkType::Scratch { at } => {
                let coord = self.relative(at);
                self.replace(Replace::write(coord, ' '));
            }
            InkType::Glyphs { tokens } => {
                if matches!(self.selection, Selection::Normal) {
                    // TODO: a little coalescing perhaps?
                    for (col, ink) in tokens {
                        let coord = self.relative(col);
                        if let Some(c) = text_stuff
                            .char_recognizer
                            .best_match(&ink_to_points(&ink, &self.grid_metrics), f32::MAX)
                        {
                            self.replace(Replace::write(coord, c));
                            if let Some(r) = self.record_recognition(coord, ink, c) {
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
                    let best_match = text_stuff
                        .big_recognizer
                        .best_match(&Points::normalize(&ink), f32::MAX);
                    let (start, end) = match &self.selection {
                        Selection::Normal => unreachable!("checked in matches! above."),
                        Selection::Single { carat } => (carat.coord, carat.coord),
                        Selection::Range { start, end } => (start.coord, end.coord),
                    };
                    match best_match {
                        Some('X') if start != end => {
                            text_stuff.clipboard = Some(self.buffer.copy(start, end));
                            self.replace(Replace::remove(start, end));
                            self.selection = Selection::Normal;
                        }
                        Some('C') if start != end => {
                            text_stuff.clipboard = Some(self.buffer.copy(start, end));
                            self.selection = Selection::Normal;
                        }
                        Some('V') => {
                            if let Some(buffer) = &text_stuff.clipboard {
                                self.replace(Replace {
                                    from: start,
                                    until: end,
                                    content: buffer.clone(),
                                });
                            }
                            self.selection = Selection::Normal;
                        }
                        Some('S') | Some('>') => {
                            self.replace(Replace::splice(
                                start,
                                TextBuffer::padding(diff_coord(start, end)),
                            ));
                            self.selection = Selection::Normal;
                        }
                        Some('<') => {
                            self.replace(Replace::remove(start, end));
                            self.selection = Selection::Normal;
                        }
                        _ => {}
                    }
                }
            }
            InkType::Strikethrough { start, end } => {
                self.replace(Replace::remove(self.relative(start), self.relative(end)));
            }
            InkType::Carat { at, ink } => {
                let coord = self.relative(at);
                self.carat(Carat { coord, ink });
            }
        };
    }
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
            |view| {
                view.handlers().pad(8).on_ink(TextMessage::Write);
            },
            |row_offset, col_offset, mut view| {
                let row = row_origin + row_offset;
                let col = col_origin + col_offset;
                let coord = (row, col);

                let (underline, draw_guidelines) = match &self.selection {
                    Selection::Normal => (false, (row, col) >= self.frozen_until),
                    Selection::Single { carat } => {
                        if coord == carat.coord {
                            view.annotate(&carat.ink);
                        }
                        (false, false)
                    }
                    Selection::Range { start, end } => {
                        if coord == start.coord {
                            view.annotate(&start.ink);
                        }
                        if coord == end.coord {
                            view.annotate(&end.ink);
                        }
                        let in_selection = coord >= start.coord && coord < end.coord;
                        (in_selection, false)
                    }
                };

                let line = self.buffer.contents.get(row);
                let char = line
                    .map(|l| match col.cmp(&l.len()) {
                        Ordering::Less => Some((l[col], 230)),
                        Ordering::Equal => {
                            let char = if row + 1 == self.buffer.contents.len() {
                                '⌧'
                            } else {
                                '⏎'
                            };
                            Some((char, 80))
                        }
                        _ => None,
                    })
                    .unwrap_or(None);

                let fragment = self.atlas.get_cell(GridCell::new(
                    &self.grid_metrics,
                    char,
                    underline,
                    draw_guidelines,
                ));
                view.draw(&*fragment);
            },
        );
    }
}
