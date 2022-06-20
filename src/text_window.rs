use crate::{
    draw_grid, ink_to_points, Atlas, Carat, Coord, InkType, Metrics, Recognition, Selection,
    Template, TextBuffer, TextMessage, TextStuff, Vector2, GRID_BORDER, NUM_RECENT_RECOGNITIONS,
};
use armrest::dollar::Points;
use armrest::ink::Ink;
use armrest::ui::{View, Widget};
use std::cmp::Ordering;
use std::collections::VecDeque;
use std::mem;
use std::rc::Rc;

#[derive(Clone)]
pub struct TextWindow {
    pub buffer: TextBuffer,
    atlas: Rc<Atlas>,
    pub grid_metrics: Metrics,
    selection: Selection,
    pub dimensions: Coord,
    pub origin: Coord,
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

    pub fn ink_row(&mut self, ink: Ink, row: usize, text_stuff: &mut TextStuff) {
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
