use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use armrest::libremarkable::cgmath::Vector2;
use armrest::libremarkable::framebuffer::common::color;
use armrest::libremarkable::framebuffer::FramebufferIO;
use armrest::ui::{Cached, Canvas, Fragment, Side, View};
use font_kit::canvas::{Format, RasterizationOptions};
use font_kit::font::Font;
use font_kit::handle::Handle;
use font_kit::hinting::HintingOptions;
use once_cell::sync::Lazy;
use pathfinder_geometry::transform2d::Transform2F;
use pathfinder_geometry::vector::{Vector2F, Vector2I};

use crate::font::*;

const GRID_LINE_COLOR: color = color::GRAY(0x7F);
const GUIDE_LINE_COLOR: color = color::GRAY(0x7F);

pub type Coord = (usize, usize);

// The width of the padding we put around a drawn grid. May or may not be coloured in.
pub const GRID_BORDER: i32 = 4;

pub static FONT_HANDLE: Lazy<Handle> = Lazy::new(|| {
    let bytes = include_bytes!("../fonts/Inconsolata-Medium.ttf");
    Handle::from_memory(Arc::new(bytes.to_vec()), 0)
});

fn fill(canvas: &mut Canvas, xs: Range<i32>, ys: Range<i32>) {
    for y in ys {
        for x in xs.clone() {
            canvas.write(x, y, GRID_LINE_COLOR);
        }
    }
}
fn line(canvas: &mut Canvas, xs: Range<i32>, ys: Range<i32>, width: i32) {
    // grid remnant
    for y in ys {
        for x in xs.clone().step_by(width as usize) {
            canvas.write(x, y, GRID_LINE_COLOR);
        }
    }
}

#[derive(Hash)]
pub struct GridBorder {
    pub side: Side,
    pub width: i32,
}

impl Fragment for GridBorder {
    fn draw(&self, canvas: &mut Canvas) {
        let size = canvas.bounds().size();

        match self.side {
            Side::Left => {
                fill(canvas, 0..size.x, 0..size.y);
            }
            Side::Right => {
                fill(canvas, 0..size.x, 0..size.y);
            }
            Side::Top => {
                fill(canvas, 0..size.x, 0..2);
                line(canvas, 0..size.x, 2..size.y, self.width);
            }
            Side::Bottom => {
                let y = size.y - 2;
                line(canvas, 0..size.x, 0..y, self.width);
                fill(canvas, 0..size.x, y..size.y);
            }
        }
    }
}

#[derive(Hash, Clone, Copy, Eq, PartialEq)]
pub struct CellDesc {
    pub metrics: Metrics,
    pub char: char,
    pub weight: u8,
    pub underline: bool,
    pub draw_guidelines: bool,
}

pub struct GridCell {
    desc: CellDesc,
    char: Option<font_kit::canvas::Canvas>,
}

impl Hash for GridCell {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.desc.hash(state);
    }
}

impl Fragment for GridCell {
    fn draw(&self, canvas: &mut Canvas) {
        let base_pixel = canvas.bounds().top_left;
        let size = canvas.bounds().size();

        let mut darken = move |x: i32, y: i32, color: color| {
            let pixel = base_pixel + Vector2::new(x, y);
            let read_pixel = pixel.map(|c| c as u32);
            let [r0, g0, b0] = canvas.framebuffer().read_pixel(read_pixel).to_rgb8();
            let [r1, g1, b1] = color.to_rgb8();
            let combined = color::RGB(r0.min(r1), g0.min(g1), b0.min(b1));
            canvas.write(x, y, combined);
        };

        if let Some(font_canvas) = &self.char {
            for (y, row) in font_canvas
                .pixels
                .chunks_exact(font_canvas.stride)
                .enumerate()
            {
                for (x, pixel) in row.iter().enumerate() {
                    let weight = ((*pixel as u32) * (self.desc.weight as u32)) / 255;
                    darken(x as i32, y as i32, color::GRAY(weight as u8));
                }
            }
        }

        let baseline = self.desc.metrics.baseline;
        let top_line = baseline - size.y * 3 / 4;
        let mid_line = baseline - size.y * 2 / 4;
        let bottom_line = baseline - size.y * 1 / 4;
        for y in 0..size.y {
            darken(0, y, GRID_LINE_COLOR);
        }
        for x in 1..size.x {
            if self.desc.draw_guidelines {
                darken(x, top_line, GUIDE_LINE_COLOR);
                darken(x, mid_line, GUIDE_LINE_COLOR);
                darken(x, bottom_line, GUIDE_LINE_COLOR);
            }
            darken(x, baseline, GRID_LINE_COLOR);
            darken(x, baseline + 1, GRID_LINE_COLOR);
            if self.desc.underline {
                darken(x, baseline + 2, GRID_LINE_COLOR);
                darken(x, baseline + 3, GRID_LINE_COLOR);
            }
        }
    }
}

pub struct Atlas {
    font: Font,
    cache: RefCell<HashMap<CellDesc, Rc<Cached<GridCell>>>>,
}

impl Atlas {
    pub fn new() -> Atlas {
        Atlas {
            font: FONT_HANDLE.load().expect("known-good font"),
            cache: RefCell::new(Default::default()),
        }
    }

    fn mint_cell(&self, desc: CellDesc) -> Rc<Cached<GridCell>> {
        let image = self.font.glyph_for_char(desc.char).map(|glyph_id| {
            let metrics = desc.metrics;
            let size = Vector2I::new(metrics.width, metrics.height);
            let mut font_canvas = font_kit::canvas::Canvas::new(size, Format::A8);
            let point_size = (metrics.height - 1) as f32;
            self.font
                .rasterize_glyph(
                    &mut font_canvas,
                    glyph_id,
                    point_size,
                    Transform2F::from_translation(Vector2F::new(0.5, metrics.baseline as f32)),
                    HintingOptions::Full(point_size),
                    RasterizationOptions::Bilevel,
                )
                .expect("rasterizing a char");
            font_canvas
        });
        Rc::new(Cached::new(GridCell { desc, char: image }))
    }

    pub fn get_cell(&self, desc: CellDesc) -> Rc<Cached<GridCell>> {
        if let Ok(mut cache) = self.cache.try_borrow_mut() {
            let value = cache
                .entry(desc.clone())
                .or_insert_with(|| self.mint_cell(desc));
            Rc::clone(value)
        } else {
            // Again, shouldn't be common, but it's good to be prepared!
            self.mint_cell(desc)
        }
    }
}

// TODO: consider making this a widget?
pub fn draw_grid<T>(
    mut view: View<T>,
    metrics: &Metrics,
    dimensions: Coord,
    mut on_grid: impl FnMut(&mut View<T>),
    mut draw_cell: impl FnMut(usize, usize, View<T>),
) {
    let (rows, cols) = dimensions;

    // TODO: put this in armrest
    let section_height = metrics.height as f32 / 4.0;
    let baseline_grid_offset = metrics.baseline as f32 % section_height;

    let top_height = (section_height - baseline_grid_offset).ceil() as i32 + 2;
    let bottom_height = baseline_grid_offset.floor() as i32 + 2;
    let left_width = 1; // NB: has a pixel of line in the cell already
    let right_width = 2;

    let height = rows as i32 * metrics.height + top_height + bottom_height;
    let width = cols as i32 * metrics.width + left_width + right_width;
    let remaining = view.size();
    view.split_off(Side::Right, (remaining.x - width).max(0));
    view.split_off(Side::Bottom, (remaining.y - height).max(0));

    // let view = view.split_off(Side::Left, cols as usize * metrics.width + GRID_BORDER * 2);

    view.split_off(Side::Left, left_width).draw(&GridBorder {
        width: metrics.width,
        side: Side::Left,
    });
    view.split_off(Side::Right, right_width).draw(&GridBorder {
        width: metrics.width,
        side: Side::Right,
    });
    view.split_off(
        Side::Top,
        (section_height - baseline_grid_offset).ceil() as i32 + 2,
    )
    .draw(&GridBorder {
        width: metrics.width,
        side: Side::Top,
    });
    view.split_off(Side::Bottom, bottom_height)
        .draw(&GridBorder {
            width: metrics.width,
            side: Side::Bottom,
        });
    on_grid(&mut view);
    for row in 0..rows {
        let mut line_view = view.split_off(Side::Top, metrics.height);
        for col in 0..cols {
            let char_view = line_view.split_off(Side::Left, metrics.width);
            draw_cell(row, col, char_view);
        }
    }
}
