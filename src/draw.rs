//! Renderer -- runs on core 1. Blits whatever [`anim::SCENE`] says onto the
//! LCD and keeps the battery bar painted. No animation logic lives here.

use embedded_graphics::{
    image::Image,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle},
};

use super::anim::{Scene, SCENE};
use super::sheet::{AnimKind, Sheet, FRAME};
use super::{BATTERY_PCT, SCREEN};

const BLACK_FILL: PrimitiveStyle<Rgb565> = PrimitiveStyle::with_fill(Rgb565::BLACK);

// --- Battery bar: a 100 px pill near the bottom of the round face. ---

const BAR_W: u32 = 100;
const BAR_H: u32 = 6;
const BAR_X: i32 = (SCREEN - BAR_W as i32) / 2;
const BAR_Y: i32 = SCREEN - BAR_H as i32 - 20;
const BAR_RADIUS: u32 = BAR_H / 2;

const BAR_TRACK_BG: Rgb565 = Rgb565::new(4, 8, 4);
const BAR_TRACK_BORDER: Rgb565 = Rgb565::new(14, 28, 14);
const BAR_GREEN: Rgb565 = Rgb565::new(7, 50, 12);
const BAR_AMBER: Rgb565 = Rgb565::new(31, 44, 0);
const BAR_RED: Rgb565 = Rgb565::new(28, 8, 6);

fn battery_bar_bbox() -> Rectangle {
    Rectangle::new(Point::new(BAR_X, BAR_Y), Size::new(BAR_W, BAR_H))
}

fn rects_overlap(a: Rectangle, b: Rectangle) -> bool {
    a.top_left.x < b.top_left.x + b.size.width as i32
        && a.top_left.x + a.size.width as i32 > b.top_left.x
        && a.top_left.y < b.top_left.y + b.size.height as i32
        && a.top_left.y + a.size.height as i32 > b.top_left.y
}

fn sub_frame(index: u32) -> Rectangle {
    Rectangle::new(Point::new((index * FRAME) as i32, 0), Size::new(FRAME, FRAME))
}

fn draw_battery_bar(lcd: &mut super::BufferedDriver, pct: u8) {
    let pct = pct.min(100);

    RoundedRectangle::with_equal_corners(battery_bar_bbox(), Size::new(BAR_RADIUS, BAR_RADIUS))
        .into_styled(
            PrimitiveStyleBuilder::new()
                .fill_color(BAR_TRACK_BG)
                .stroke_color(BAR_TRACK_BORDER)
                .stroke_width(1)
                .build(),
        )
        .draw(lcd)
        .unwrap();

    let inner_w = BAR_W - 2;
    let fill_w = inner_w * pct as u32 / 100;
    if fill_w >= BAR_RADIUS {
        let color = if pct > 50 {
            BAR_GREEN
        } else if pct > 20 {
            BAR_AMBER
        } else {
            BAR_RED
        };
        let fill = Rectangle::new(Point::new(BAR_X + 1, BAR_Y + 1), Size::new(fill_w, BAR_H - 2));
        RoundedRectangle::with_equal_corners(fill, Size::new(BAR_RADIUS - 1, BAR_RADIUS - 1))
            .into_styled(PrimitiveStyle::with_fill(color))
            .draw(lcd)
            .unwrap();
    }
}

#[embassy_executor::task]
pub async fn lcd_task(lcd: &'static mut super::BufferedDriver) {
    lcd.clear();
    let mut shown_battery = BATTERY_PCT.load(core::sync::atomic::Ordering::Relaxed).min(100);
    draw_battery_bar(lcd, shown_battery);
    lcd.flush().unwrap();

    let mut cur_kind: Option<AnimKind> = None;
    let mut bmp = Sheet::for_kind(AnimKind::Sit).bmp();
    let mut prev_rect: Option<Rectangle> = None;
    let mut last = Scene {
        kind: AnimKind::Tumble,
        index: u32::MAX,
        x: 0,
        y: 0,
        battery: 0,
    };

    loop {
        let scene = SCENE.wait().await;
        if scene == last {
            continue;
        }
        last = scene;

        if cur_kind != Some(scene.kind) {
            bmp = Sheet::for_kind(scene.kind).bmp();
            cur_kind = Some(scene.kind);
        }

        if let Some(r) = prev_rect {
            r.into_styled(BLACK_FILL).draw(lcd).unwrap();
        }

        let pos = Point::new(scene.x as i32, scene.y as i32);
        Image::new(&bmp.sub_image(&sub_frame(scene.index)), pos)
            .draw(lcd)
            .unwrap();
        let rect = Rectangle::new(pos, Size::new(FRAME, FRAME));

        // Repaint the bar if a sprite scribbled over it or the level moved.
        let bar = battery_bar_bbox();
        let touched = rects_overlap(rect, bar)
            || prev_rect.is_some_and(|r| rects_overlap(r, bar));
        if scene.battery != shown_battery || touched {
            draw_battery_bar(lcd, scene.battery);
            shown_battery = scene.battery;
        }

        prev_rect = Some(rect);
        lcd.flush().unwrap();
    }
}
