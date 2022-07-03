use crate::ink_type::InkMode;
use crate::util::rotate_queue;
use crate::*;
use armrest::dollar::Points;
use armrest::ink::Ink;
use armrest::ui::{View, Widget};
use std::cmp::Ordering;
use std::collections::{BTreeSet, VecDeque};
use std::mem;
use std::rc::Rc;
use textwrap;
use textwrap::Options;

const NUM_RECENT_RECOGNITIONS: usize = 10;
const NUM_UNDOS: usize = 64;

pub enum TextMessage {
    Write(Ink),
    Erase(Ink),
}

#[derive(Clone, Debug)]
pub struct Recognition {
    coord: Coord,
    ink: Ink,
    recognized_as: char,
    overwrites: Vec<Ink>,
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
    pub redos: Vec<Replace>,
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
            redos: vec![],
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

    pub fn scroll_into_view(&mut self, coord: Coord) {
        fn clamp_relative(value: usize, reference: usize, dimension: usize) -> usize {
            value.clamp(reference.saturating_sub(dimension - 1), reference)
        }
        let row = clamp_relative(self.origin.0, coord.0, self.dimensions.0);
        let col = clamp_relative(self.origin.1, coord.1, self.dimensions.1);
        dbg!(self.origin, self.dimensions, row, col, coord);
        self.origin = (row, col);
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

    pub fn do_replace(&mut self, mut replace: Replace) -> Replace {
        // Avoid editing the frozen section of the buffer.
        if self.frozen_until > replace.until {
            replace.from = self.frozen_until;
            replace.until = self.frozen_until;
            replace.content = TextBuffer::empty();
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
        undo
    }

    pub fn replace(&mut self, replace: Replace) {
        // Avoid editing the frozen section of the buffer.
        let undo = self.do_replace(replace);
        rotate_queue(&mut self.undos, undo, NUM_UNDOS);
        self.redos.clear(); // No longer valid!
    }

    pub fn undo(&mut self) {
        if let Some(undo) = self.undos.pop_back() {
            self.scroll_into_view(undo.from);
            let redo = self.do_replace(undo);
            self.redos.push(redo);
        }
    }

    pub fn redo(&mut self) {
        if let Some(redo) = self.redos.pop() {
            self.scroll_into_view(redo.from);
            let undo = self.do_replace(redo);
            rotate_queue(&mut self.undos, undo, NUM_UNDOS);
        }
    }

    fn relative(&self, coord: Coord) -> Coord {
        (self.origin.0 + coord.0, self.origin.1 + coord.1)
    }

    pub fn erase(&mut self, ink: Ink) {
        let width = self.grid_metrics.width as f32;
        let height = self.grid_metrics.height as f32;
        let ink = ink.resample(width / 2.0);

        let mut to_erase = BTreeSet::new();
        for stroke in ink.strokes() {
            for point in stroke {
                let row = (point.y / height).max(0.0);
                let col = (point.x / width).max(0.0);
                to_erase.insert(self.relative((row as usize, col as usize)));
            }
        }

        let mut iter = to_erase.into_iter();
        if let Some((row, col)) = iter.next() {
            let mut start = (row, col);
            let mut end = (row, col + 1);

            while let Some(coord) = iter.next() {
                if coord == end {
                    // we can expand the current run
                    end = (end.0, end.1 + 1);
                } else {
                    // replace and begin anew
                    start = self.buffer.clamp(start);
                    end = self.buffer.clamp(end);
                    self.replace(Replace {
                        from: start,
                        until: end,
                        content: TextBuffer::padding(diff_coord(start, end)),
                    });
                    start = coord;
                    end = (coord.0, coord.1 + 1);
                }
            }
            start = self.buffer.clamp(start);
            end = self.buffer.clamp(end);
            self.replace(Replace {
                from: start,
                until: end,
                content: TextBuffer::padding(diff_coord(start, end)),
            });
        }
    }

    fn find_token(&mut self, start: Coord, end: Coord, forward: bool) {
        let query = self.buffer.copy(start, end);
        let line: &[char] = &query.contents[0];

        fn find_in(
            contents: &Vec<Vec<char>>,
            query: &[char],
            rows: impl Iterator<Item = usize>,
        ) -> Option<(usize, usize)> {
            rows.filter_map(|row| {
                let row_data = &contents[row];
                row_data
                    .windows(query.len())
                    .enumerate()
                    .find(|(_, w)| *w == query)
                    .map(|(col, _)| (row, col))
            })
            .next()
        }

        let result = if forward {
            find_in(
                &self.buffer.contents,
                line,
                (start.0 + 1)..self.buffer.contents.len(),
            )
        } else {
            find_in(&self.buffer.contents, line, (0..start.0).rev())
        };

        if let Some(new_start) = result {
            let new_end = add_coord(new_start, diff_coord(start, end));
            if let Selection::Range { start, end } = &mut self.selection {
                start.coord = new_start;
                end.coord = new_end;
                // NB: try and get both start/end onscreen where possible.
                self.scroll_into_view(new_end);
                self.scroll_into_view(new_start);
            }
        }
    }

    pub fn mode(&self) -> InkMode {
        match &self.selection {
            Selection::Normal => InkMode::Normal,
            _ => InkMode::Special,
        }
    }

    pub fn ink_row(&mut self, ink_type: InkType, text_stuff: &mut TextStuff) {
        match ink_type {
            InkType::Scratch { at } => {
                let coord = self.relative(at);
                self.replace(Replace::write(coord, ' '));
            }
            InkType::Glyphs { tokens } => {
                // TODO: a little coalescing perhaps?
                for (col, ink) in tokens {
                    // So, this is a slightly awkward little dance. The key observation is that
                    // if the system mispredicts a character, the user will almost always try
                    // and overwrite the bad guess again to "fix up" the text; and when that
                    // happens, the original ink is a likely candidate for a new template.
                    // If we can automatically add that template to the list, the burden of
                    // gardening templates is significantly reduced.

                    // However, a user might overwrite a character for other reasons; changing
                    // their mind about what they want to say, for example, so we need some way
                    // to validate our guess.

                    // Every window keeps a little state tracking recent recognitions. We track
                    // overwrites; if an overwrite is not itself overwritten, we add it to our
                    // shared list of candidates.

                    // When we score new characters, we also score them against our candidates,
                    // and track how well the candidates do. Candidates that are reliable get
                    // promoted to the main template list. We presumably will still get this
                    // wrong, but at least users can prune bad ones from there if needed.
                    let coord = self.relative(col);
                    if let Some(c) = text_stuff
                        .char_recognizer
                        .best_match(&ink_to_points(&ink, &self.grid_metrics), f32::MAX)
                    {
                        let overwrites = if let Some(index) = self
                            .tentative_recognitions
                            .iter()
                            .position(|r| r.coord == coord)
                        {
                            let mut prev = self
                                .tentative_recognitions
                                .remove(index)
                                .expect("removing just-discovered match");
                            prev.overwrites.push(prev.ink);
                            prev.overwrites
                        } else {
                            vec![]
                        };

                        let recon = Recognition {
                            coord,
                            ink,
                            recognized_as: c,
                            overwrites,
                        };

                        self.replace(Replace::write(coord, c));

                        if let Some(r) = rotate_queue(
                            &mut self.tentative_recognitions,
                            recon,
                            NUM_RECENT_RECOGNITIONS,
                        ) {
                            dbg!(r.recognized_as, r.overwrites.len());
                            for ink in r.overwrites {
                                let points = ink_to_points(&ink, &self.grid_metrics);
                                text_stuff.on_overwrite(ink, points, r.recognized_as);
                            }
                        }
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
            InkType::BigGlyph { token } => {
                let ink = token;
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
                    Some('Q') => {
                        let line_start = (start.0, 0);
                        let end = if end == start {
                            self.buffer.clamp((end.0, usize::MAX))
                        } else {
                            end
                        };
                        let remaining_width = self.dimensions.1 - start.1;
                        let prefix = self.buffer.copy(line_start, start).content_string();
                        let remainder = self.buffer.copy(start, end).content_string();
                        let remainder = remainder.replace(&format!("\n{}", prefix), " ");
                        let wrapped = textwrap::fill(
                            &remainder,
                            Options::new(remaining_width).subsequent_indent(&prefix),
                        );
                        self.replace(Replace {
                            from: start,
                            until: end,
                            content: TextBuffer::from_string(&wrapped),
                        });
                        self.selection = Selection::Normal;
                    }
                    Some('N') if start != end && start.0 == end.0 => {
                        self.find_token(start, end, true);
                    }
                    Some('P') if start != end && start.0 == end.0 => {
                        self.find_token(start, end, false);
                    }
                    _ => {}
                }
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
                view.handlers().on_erase(TextMessage::Erase);
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
