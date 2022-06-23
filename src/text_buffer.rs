use std::cmp::Ordering;

type Coord = (usize, usize);

fn add_coord(a: Coord, b: Coord) -> Coord {
    if b.0 == 0 {
        (a.0, a.1 + b.1)
    } else {
        (a.0 + b.0, b.1)
    }
}

fn diff_coord(a: Coord, b: Coord) -> Coord {
    let (a, b) = if a < b { (a, b) } else { (b, a) };
    if a.0 == b.0 {
        (0, b.1 - a.1)
    } else {
        (b.0 - a.0, b.0)
    }
}

#[derive(Clone)]
pub struct TextBuffer {
    pub contents: Vec<Vec<char>>,
}

impl Default for TextBuffer {
    fn default() -> Self {
        TextBuffer::empty()
    }
}

impl TextBuffer {
    pub fn empty() -> TextBuffer {
        TextBuffer {
            contents: vec![vec![]],
        }
    }

    pub fn from_string(str: &str) -> TextBuffer {
        let contents: Vec<_> = str.split('\n').map(|line| line.chars().collect()).collect();
        TextBuffer { contents }
    }

    pub fn padding(coord: Coord) -> TextBuffer {
        let mut contents = vec![vec![]; coord.0];
        contents.push(vec![' '; coord.1]);
        TextBuffer { contents }
    }

    /// Given a coordinate, find the nearest valid coordinate in the text.
    /// Cols past the end of a line clamp to the end of the line,
    /// and rows past the end clamp to the lasts valid coordinate.
    pub fn clamp(&self, coords: Coord) -> Coord {
        let (row, col) = coords;
        let rows = self.contents.len();
        if row >= rows {
            let row = rows - 1;
            (row, self.contents[row].len())
        } else {
            let cols = self.contents[row].len();
            (row, cols.min(col))
        }
    }

    /// Pad with newlines and spaces as necessary such that `contents[row][col..col]` is valid.
    pub fn pad(&mut self, row: usize, col: usize) {
        if self.contents.len() <= row {
            self.contents.resize(row + 1, vec![]);
        }
        let row = &mut self.contents[row];
        if row.len() < col {
            row.resize(col, ' ');
        }
    }

    /// Insert a specific char at specific coordinates, padding as necessary.
    pub fn write(&mut self, (row, col): Coord, c: char) {
        self.pad(row, col + 1);
        self.contents[row][col] = c;
    }

    pub fn split_off(&mut self, at: Coord) -> TextBuffer {
        let (row, col) = self.clamp(at);
        let insert_row = &mut self.contents[row];
        let trailer = insert_row.split_off(col);
        let mut result = Vec::with_capacity(self.contents.len() - row);
        result.push(trailer);
        result.extend(self.contents.drain((row + 1)..));
        TextBuffer { contents: result }
    }

    pub fn append(&mut self, buffer: TextBuffer) {
        let mut iter = buffer.contents.into_iter();
        self.contents
            .last_mut()
            .unwrap()
            .extend(iter.next().unwrap());
        self.contents.extend(iter);
    }

    pub fn replace(&mut self, replace: Replace) -> Replace {
        let clamped_from = self.clamp(replace.from);
        let replace = if replace.from > clamped_from {
            let mut content = TextBuffer::padding(diff_coord(clamped_from, replace.from));
            content.append(replace.content);
            Replace {
                from: clamped_from,
                until: replace.until,
                content,
            }
        } else {
            replace
        };

        let trailer = self.split_off(replace.until);
        let undo_content = self.split_off(replace.from);
        self.append(replace.content);
        let undo_until = self.end();
        self.append(trailer);
        Replace {
            from: replace.from,
            until: undo_until,
            content: undo_content,
        }
    }

    pub fn copy(&self, from: Coord, until: Coord) -> TextBuffer {
        assert!(from <= until);
        let (from_row, from_col) = self.clamp(from);
        let (until_row, until_col) = self.clamp(until);

        let contents = if from_row == until_row {
            vec![self.contents[from_row][from_col..until_col].to_vec()]
        } else {
            let mut contents = vec![self.contents[from_row][from_col..].to_vec()];
            contents.extend(self.contents[(from_row + 1)..=until_row].iter().cloned());
            contents.push(self.contents[until_row][..until_col].to_vec());
            contents
        };

        TextBuffer { contents }
    }

    pub fn splice(&mut self, at: Coord, mut buffer: TextBuffer) -> Replace {
        self.replace(Replace {
            from: at,
            until: at,
            content: buffer,
        })
    }

    /// Remove all the contents between the two provided coordinates.
    pub fn remove(&mut self, from: Coord, until: Coord) -> Replace {
        self.replace(Replace {
            from,
            until,
            content: TextBuffer::empty(),
        })
    }

    pub fn end(&self) -> Coord {
        let row = self.contents.len() - 1;
        (row, self.contents[row].len())
    }

    /// Render the contents of the buffer as a new String.
    pub fn content_string(&self) -> String {
        let mut result = String::new();
        for (i, line) in self.contents.iter().enumerate() {
            if i != 0 {
                result.push('\n');
            }
            result.extend(line);
        }
        result
    }
}

pub struct Replace {
    from: Coord,
    until: Coord,
    content: TextBuffer,
}

impl Replace {
    pub fn insert_at(coord: Coord, content: TextBuffer) -> Replace {
        Replace {
            from: coord,
            until: coord,
            content,
        }
    }
}
