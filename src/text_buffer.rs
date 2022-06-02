use std::cmp::Ordering;

type Coord = (usize, usize);

#[derive(Clone)]
pub struct TextBuffer {
    pub contents: Vec<Vec<char>>,
}

impl TextBuffer {
    pub fn empty() -> TextBuffer {
        TextBuffer {
            contents: vec![vec![]],
        }
    }

    pub fn from_string(str: &str) -> TextBuffer {
        let contents = str.lines().map(|line| line.chars().collect()).collect();
        TextBuffer { contents }
    }

    /// Clamp a pair of coordinates to the nearest valid pair.
    /// (row, col) is valid if `contents[row][col..col]` wouldn't panic.
    pub fn clamp(&self, coords: Coord) -> Coord {
        let (row, col) = coords;
        let rows = self.contents.len();
        match (row + 1).cmp(&rows) {
            Ordering::Less => {
                let cols = self.contents[row].len();
                if col > cols {
                    (row + 1, 0)
                } else {
                    (row, col)
                }
            }
            Ordering::Equal => (row, self.contents[row].len().min(col)),
            Ordering::Greater => {
                let row = rows - 1;
                (row, self.contents[row].len())
            }
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

    /// Remove all the contents between the two provided coordinates.
    pub fn remove(&mut self, from: Coord, to: Coord) {
        assert!(from < to, "start must be less than end");
        let (from_row, from_col) = self.clamp(from);
        let (to_row, to_col) = self.clamp(to);
        if from_row == to_row {
            self.contents[from_row].drain(from_col..to_col);
        } else {
            self.contents[from_row].truncate(from_col);
            let trailer = self
                .contents
                .drain((from_row + 1)..=to_row)
                .last()
                .expect("removing at least one row");
            self.contents[from_row].extend(trailer.into_iter().skip(to_col))
        }
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
