use armrest::ui::{Text, TextFragment};
use once_cell::sync::Lazy;
use rusttype::Font;

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

pub const DEFAULT_CHAR_HEIGHT: i32 = 40;
