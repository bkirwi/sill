use armrest::ui::{Canvas, Fragment, Side, TextFragment};
use libremarkable::framebuffer::common::color;

#[derive(Hash)]
pub struct Border {
    pub side: Side,
    pub width: i32,
    pub start_offset: i32,
    pub end_offset: i32,
    pub color: u8,
}

impl Fragment for Border {
    fn draw(&self, canvas: &mut Canvas) {
        let size = canvas.bounds().size();
        let (xrange, yrange) = match self.side {
            Side::Left => (0..self.width, self.start_offset..(size.y - self.end_offset)),
            Side::Right => (
                (size.x - self.width)..size.x,
                self.start_offset..(size.y - self.end_offset),
            ),
            Side::Top => (self.start_offset..(size.x - self.end_offset), 0..self.width),
            Side::Bottom => (
                self.start_offset..(size.x - self.end_offset),
                (size.y - self.width)..size.y,
            ),
        };

        for x in xrange {
            for y in yrange.clone() {
                canvas.write(x, y, color::GRAY(self.color));
            }
        }
    }
}

#[derive(Hash)]
pub struct GridCell {
    pub baseline: i32,
    pub char: Option<TextFragment>,
}

impl Fragment for GridCell {
    fn draw(&self, canvas: &mut Canvas) {
        let size = canvas.bounds().size();
        let top_line = self.baseline - size.y * 3 / 4;
        let mid_line = self.baseline - size.y * 2 / 4;
        let bottom_line = self.baseline - size.y * 1 / 4;
        for y in 0..size.y {
            canvas.write(0, y, color::GRAY(120));
        }
        for x in 1..size.x {
            canvas.write(x, top_line, color::GRAY(40));
            canvas.write(x, mid_line, color::GRAY(40));
            canvas.write(x, bottom_line, color::GRAY(40));
            canvas.write(x, self.baseline, color::GRAY(120));
        }
        if let Some(c) = &self.char {
            c.draw(canvas);
        }
    }
}
