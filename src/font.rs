use armrest::ui::{Text, TextFragment};
use once_cell::sync::Lazy;
use rusttype::{Font, Scale};

use std::hash::Hash;

pub static FONT: Lazy<Font<'static>> = Lazy::new(|| {
    let font_bytes: &[u8] = include_bytes!("../fonts/Inconsolata-Regular.ttf");
    Font::from_bytes(font_bytes).unwrap()
});

pub(crate) const DEFAULT_CHAR_HEIGHT: i32 = 40;
pub(crate) const TEXT_WEIGHT: f32 = 0.9;

pub fn text_literal(height: i32, text: &str) -> TextFragment {
    // NB: Inconsolata has zero line gap.
    // TODO: proper centering instead of this manual hack.
    Text::builder(height, &*FONT)
        .scale(1.5)
        .space()
        .scale(height as f32)
        .literal(text)
        .into_text()
        .to_fragment()
}

#[derive(Hash, Clone, Copy)]
pub struct Metrics {
    pub height: i32,
    pub width: i32,
    pub baseline: i32,
}

impl Metrics {
    pub fn new(height: i32) -> Metrics {
        let scale = Scale::uniform(height as f32);
        let v_metrics = FONT.v_metrics(scale);
        let h_metrics = FONT.glyph(' ').scaled(scale).h_metrics();
        let width = h_metrics.advance_width.ceil() as i32;

        Metrics {
            height,
            width,
            baseline: v_metrics.ascent.ceil() as i32,
        }
    }
}
