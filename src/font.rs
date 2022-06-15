use armrest::libremarkable::framebuffer::common::color;
use armrest::libremarkable::framebuffer::{FramebufferDraw, FramebufferIO};
use armrest::ui::{Canvas, Fragment, Text, TextFragment, View, Widget};
use once_cell::sync::Lazy;
use rusttype::Font;
use std::cell::{BorrowMutError, Cell, RefCell, RefMut};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use crate::{GridCell, Metrics, Vector2};

pub static FONT: Lazy<Font<'static>> = Lazy::new(|| {
    let font_bytes: &[u8] = include_bytes!("../fonts/Inconsolata-Regular.ttf");
    Font::from_bytes(font_bytes).unwrap()
});

pub fn text_literal(height: i32, text: &str) -> TextFragment {
    // NB: Inconsolata has zero line gap.
    Text::builder(height, &*FONT)
        .literal(text)
        .into_text()
        .to_fragment()
}

pub struct Cached<T> {
    fragment: T,
    cached_render: RefCell<(Vector2<i32>, Vec<u8>)>,
}

impl<T> Cached<T> {
    pub fn new(fragment: T) -> Cached<T> {
        Cached {
            fragment,
            cached_render: RefCell::new((Vector2::new(0, 0), vec![])),
        }
    }
}

impl<T: Hash> Hash for Cached<T> {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.fragment.hash(state)
    }
}

impl<T: Fragment> Fragment for Cached<T> {
    fn draw(&self, canvas: &mut Canvas) {
        let bounds = canvas.bounds();
        if let Ok(mut borrow) = self.cached_render.try_borrow_mut() {
            let (cached_size, cached_data) = &mut *borrow;

            if bounds.size() == *cached_size {
                // If our cached data is the right size, splat onto the framebuffer.
                let result = canvas
                    .framebuffer()
                    .restore_region(bounds.rect(), cached_data);
                if result.is_err() {
                    self.fragment.draw(canvas);
                }
            } else {
                // Otherwise, blank (to avoid caching any garbage), draw, and dump
                // for the next time.
                canvas.framebuffer().fill_rect(
                    bounds.top_left,
                    bounds.size().map(|c| c as u32),
                    color::WHITE,
                );
                self.fragment.draw(canvas);
                if let Ok(data) = canvas.framebuffer().dump_region(bounds.rect()) {
                    *cached_size = bounds.size();
                    *cached_data = data;
                }
            }
        } else {
            // Unlikely, since there should only be one draw happening at once!
            self.fragment.draw(canvas);
        }
    }
}

pub struct Atlas {
    opacity: f32,
    metrics: Metrics,
    cache: RefCell<HashMap<(Option<char>, bool), Rc<Cached<GridCell>>>>,
}

impl Atlas {
    pub fn new(opacity: f32, metrics: Metrics) -> Atlas {
        Atlas {
            opacity,
            metrics,
            cache: RefCell::new(Default::default()),
        }
    }

    fn fresh_cell(&self, char: Option<char>, selected: bool) -> Rc<Cached<GridCell>> {
        Rc::new(Cached::new(GridCell {
            baseline: self.metrics.baseline,
            char: char.map(|c| text_literal(self.metrics.height, &c.to_string())),
            insert_area: selected,
        }))
    }

    pub fn get_cell(&self, char: Option<char>, selected: bool) -> Rc<Cached<GridCell>> {
        if let Ok(mut cache) = self.cache.try_borrow_mut() {
            let value = cache
                .entry((char, selected))
                .or_insert(self.fresh_cell(char, selected));
            Rc::clone(value)
        } else {
            // Again, shouldn't be common, but it's good to be prepared!
            self.fresh_cell(char, selected)
        }
    }
}
