use crate::{text_literal, Metrics};
use armrest::libremarkable::framebuffer::common::color;
use armrest::ui::{Cached, Canvas, Fragment, Side, TextFragment, View};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

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
    pub insert_area: bool,
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
            if self.insert_area {
                canvas.write(x, self.baseline + 1, color::GRAY(120));
                canvas.write(x, self.baseline + 2, color::GRAY(120));
            }
        }
        if let Some(c) = &self.char {
            c.draw(canvas);
        }
    }
}

pub struct Atlas {
    metrics: Metrics,
    cache: RefCell<HashMap<(Option<char>, bool, bool), Rc<Cached<GridCell>>>>,
}

impl Atlas {
    pub fn new(metrics: Metrics) -> Atlas {
        Atlas {
            metrics,
            cache: RefCell::new(Default::default()),
        }
    }

    fn fresh_cell(
        &self,
        char: Option<char>,
        selected: bool,
        background: bool,
    ) -> Rc<Cached<GridCell>> {
        let weight = if background { 0.3 } else { 0.9 };
        Rc::new(Cached::new(GridCell {
            baseline: self.metrics.baseline,
            char: char
                .map(|c| text_literal(self.metrics.height, &c.to_string()).with_weight(weight)),
            insert_area: selected,
        }))
    }

    pub fn get_cell(
        &self,
        char: Option<char>,
        selected: bool,
        background: bool,
    ) -> Rc<Cached<GridCell>> {
        if let Ok(mut cache) = self.cache.try_borrow_mut() {
            let value = cache
                .entry((char, selected, background))
                .or_insert(self.fresh_cell(char, selected, background));
            Rc::clone(value)
        } else {
            // Again, shouldn't be common, but it's good to be prepared!
            self.fresh_cell(char, selected, background)
        }
    }
}

pub type Coord = (usize, usize);

// The width of the padding we put around a drawn grid. May or may not be coloured in.
pub const GRID_BORDER: i32 = 4;

// TODO: consider making this a widget?
pub fn draw_grid<T>(
    mut view: View<T>,
    metrics: &Metrics,
    dimensions: Coord,
    mut on_row: impl FnMut(usize, &mut View<T>),
    mut draw_cell: impl FnMut(usize, usize, View<T>),
) {
    let (rows, cols) = dimensions;
    // TODO: fit to space provided?
    const LEFT_MARGIN_BORDER: i32 = 4;
    const MARGIN_BORDER: i32 = 2;

    // TODO: put this in armrest
    let height = rows as i32 * metrics.height + GRID_BORDER * 2;
    let width = cols as i32 * metrics.width + GRID_BORDER * 2;
    let remaining = view.size();
    view.split_off(Side::Right, (remaining.x - width).max(0));
    view.split_off(Side::Bottom, (remaining.y - height).max(0));

    // let view = view.split_off(Side::Left, cols as usize * metrics.width + GRID_BORDER * 2);
    view.split_off(Side::Top, GRID_BORDER).draw(&Border {
        side: Side::Bottom,
        width: MARGIN_BORDER,
        color: 100,
        start_offset: GRID_BORDER - LEFT_MARGIN_BORDER,
        end_offset: GRID_BORDER - MARGIN_BORDER,
    });
    view.split_off(Side::Bottom, GRID_BORDER).draw(&Border {
        side: Side::Top,
        width: MARGIN_BORDER,
        color: 100,
        start_offset: GRID_BORDER - LEFT_MARGIN_BORDER,
        end_offset: GRID_BORDER - MARGIN_BORDER,
    });
    view.split_off(Side::Left, GRID_BORDER).draw(&Border {
        side: Side::Right,
        width: LEFT_MARGIN_BORDER,
        color: 100,
        start_offset: 0,
        end_offset: 0,
    });
    view.split_off(Side::Right, GRID_BORDER).draw(&Border {
        side: Side::Left,
        width: MARGIN_BORDER,
        color: 100,
        start_offset: 0,
        end_offset: 0,
    });
    for row in 0..rows {
        let mut line_view = view.split_off(Side::Top, metrics.height);
        on_row(row, &mut line_view);
        for col in 0..cols {
            let char_view = line_view.split_off(Side::Left, metrics.width);
            draw_cell(row, col, char_view);
        }
    }
}
