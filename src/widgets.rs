use crate::font::{DEFAULT_CHAR_HEIGHT, FONT, TEXT_WEIGHT};
use armrest::libremarkable::cgmath::{Vector2, Zero};
use armrest::libremarkable::framebuffer::common::color;
use armrest::ui::*;

#[derive(Hash)]
struct Underline(i32);
const UNDERLINE: i32 = 3;

impl Fragment for Underline {
    fn draw(&self, canvas: &mut Canvas) {
        let size = canvas.bounds().size();
        for y in 0..size.y.min(UNDERLINE) {
            for x in y..(size.x.min(self.0) - y) {
                canvas.write(x, y, color::GRAY(200));
            }
        }
    }
}

pub struct Button<T: Widget> {
    widget: T,
    on_tap: Option<T::Message>,
}

impl<T: Widget> Widget for Button<T>
where
    T::Message: Clone,
{
    type Message = T::Message;

    fn size(&self) -> Vector2<i32> {
        let mut size = self.widget.size();
        size.y += UNDERLINE;
        size
    }

    fn render(&self, mut view: View<Self::Message>) {
        if let Some(msg) = &self.on_tap {
            view.handlers().pad(10).on_tap(msg.clone());
        }
        self.widget.render_split(&mut view, Side::Top, 0.0);
        if self.on_tap.is_some() {
            view.split_off(Side::Top, UNDERLINE)
                .draw(&Underline(self.size().x))
        }
    }
}

impl<M: Clone> Button<Text<M>> {
    pub fn new(text: &str, msg: M, active: bool) -> Button<Text<M>> {
        let builder = Text::builder(DEFAULT_CHAR_HEIGHT, &*FONT);
        let builder = if active {
            builder.weight(TEXT_WEIGHT)
        } else {
            builder.weight(0.5)
        };
        let text = builder.literal(text).into_text();

        Button {
            widget: text,
            on_tap: if active { Some(msg) } else { None },
        }
    }
}

pub struct Spaced<'a, A>(pub i32, pub &'a [A]);

impl<'a, A: Widget> Widget for Spaced<'a, A> {
    type Message = A::Message;

    fn size(&self) -> Vector2<i32> {
        let mut size: Vector2<i32> = Vector2::zero();
        let Spaced(pad, widgets) = self;
        for (i, a) in widgets.iter().enumerate() {
            if i != 0 {
                size.x += *pad;
            }
            let a_size = a.size();
            size.x += a_size.x;
            size.y = size.y.max(a_size.y);
        }
        size
    }

    fn render(&self, mut view: View<Self::Message>) {
        let Spaced(pad, widgets) = self;
        for (i, a) in widgets.iter().enumerate() {
            if i != 0 {
                view.split_off(Side::Left, *pad);
            }
            a.render_split(&mut view, Side::Left, 0.0);
        }
    }
}
