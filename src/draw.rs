use core::sync::atomic::Ordering;

use embassy_time::{Duration, Instant, Timer};
use embedded_graphics::{
    image::Image,
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, PrimitiveStyleBuilder, Rectangle, RoundedRectangle},
};
use tinyrand::{Probability, Rand, RandRange, StdRand};

use super::sheet::{self, AnimKind, Sheet};
use super::{BATTERY_PCT, SHAKE};

const DISPLAY: Size = Size::new(240, 240);
const FRAME: u32 = sheet::FRAME;
/// Top-left position that centres a sprite.
const HOME: i32 = (DISPLAY.width as i32 - FRAME as i32) / 2;
/// Largest in-bounds sprite X.
const MAX_X: i32 = DISPLAY.width as i32 - FRAME as i32;

const BLACK_FILL: PrimitiveStyle<Rgb565> = PrimitiveStyle::with_fill(Rgb565::BLACK);
/// Chance an idle animation repeats instead of switching.
const LINGER_CHANCE: f64 = 0.7;

// --- Battery bar: a 100 px pill near the bottom of the round face. ---

const BAR_W: u32 = 100;
const BAR_H: u32 = 6;
const BAR_X: i32 = (DISPLAY.width as i32 - BAR_W as i32) / 2;
const BAR_Y: i32 = DISPLAY.height as i32 - BAR_H as i32 - 20;
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

// --- Tumble: on a shake the sprite is flung between random points on the
// rim, snapping to a new one each leg -- edge to edge, not a timid bounce. ---

/// Sprite top-left points on a radius-110 circle about the centre (a few px
/// hangs off the rim at the extremes, which is the point).
const TUMBLE_TARGETS: [(i32, i32); 16] = [
    (205, 95),
    (197, 137),
    (173, 173),
    (137, 197),
    (95, 205),
    (53, 197),
    (17, 173),
    (-7, 137),
    (-15, 95),
    (-7, 53),
    (17, 17),
    (53, -7),
    (95, -15),
    (137, -7),
    (173, 17),
    (197, 53),
];
/// Interpolated frames per leg -- small, so it nearly teleports rim to rim.
const TUMBLE_LEG_STEPS: u32 = 3;
/// Physics/redraw tick while tumbling (~25 fps).
const TUMBLE_STEP: Duration = Duration::from_millis(40);
/// A tumble lasts this long; each shake pushes the deadline out again.
const TUMBLE_DURATION: Duration = Duration::from_millis(2500);

const fn sub_frame(index: u32) -> Rectangle {
    Rectangle::new(Point::new((index * FRAME) as i32, 0), Size::new(FRAME, FRAME))
}

fn pick_random_kind(rand: &mut StdRand) -> AnimKind {
    AnimKind::ALL[rand.next_range(0..AnimKind::ALL.len() as u32) as usize]
}

/// A fresh tumble leg from `from` to a random rim point: `(start, target, step)`.
fn new_leg(rand: &mut StdRand, from: (i32, i32)) -> ((i32, i32), (i32, i32), u32) {
    let idx = rand.next_range(0..TUMBLE_TARGETS.len() as u32) as usize;
    (from, TUMBLE_TARGETS[idx], 0)
}

#[embassy_executor::task]
pub async fn lcd_task(lcd: &'static mut super::BufferedDriver) {
    Timer::after_millis(100).await;

    let mut rand = StdRand::default();
    let linger_prob = Probability::new(LINGER_CHANCE);

    let mut current_kind = pick_random_kind(&mut rand);
    let mut current_sheet = Sheet::for_kind(current_kind);
    let mut current_bmp = current_sheet.bmp();

    let (mut sprite_x, mut sprite_y) = (HOME, HOME);

    // Tumble leg state: interpolate the sprite from leg_start to leg_target
    // over TUMBLE_LEG_STEPS ticks, then pick a new rim point.
    let mut leg_start = (sprite_x, sprite_y);
    let mut leg_target = (sprite_x, sprite_y);
    let mut leg_step: u32 = 0;
    let mut tumble_deadline = Instant::now();

    let mut anim_frame: u32 = 0;
    let mut prev_sprite_rect: Option<Rectangle> = None;

    lcd.clear();
    let mut shown_battery = BATTERY_PCT.load(Ordering::Relaxed).min(100);
    draw_battery_bar(lcd, shown_battery);
    lcd.flush().unwrap();

    let anim_period = Duration::from_millis(300);
    let mut next_anim_tick = Instant::now() + anim_period;
    let mut next_tumble_step = Instant::now();

    loop {
        // A shake starts a tumble; a shake during one re-flings it, so a
        // sustained head-shake keeps it flying.
        if SHAKE.try_take().is_some() {
            let now = Instant::now();
            if current_kind != AnimKind::Tumble {
                current_kind = AnimKind::Tumble;
                current_sheet = Sheet::for_kind(current_kind);
                current_bmp = current_sheet.bmp();
                anim_frame = 0;
                next_tumble_step = now;
            }
            (leg_start, leg_target, leg_step) = new_leg(&mut rand, (sprite_x, sprite_y));
            tumble_deadline = now + TUMBLE_DURATION;
        }

        let mut sprite_dirty = false;
        let now = Instant::now();

        if current_kind == AnimKind::Tumble {
            if now >= next_tumble_step {
                next_tumble_step = now + TUMBLE_STEP;

                leg_step += 1;
                if leg_step >= TUMBLE_LEG_STEPS {
                    (sprite_x, sprite_y) = leg_target;
                    (leg_start, leg_target, leg_step) = new_leg(&mut rand, leg_target);
                } else {
                    let (t, n) = (leg_step as i32, TUMBLE_LEG_STEPS as i32);
                    sprite_x = leg_start.0 + (leg_target.0 - leg_start.0) * t / n;
                    sprite_y = leg_start.1 + (leg_target.1 - leg_start.1) * t / n;
                }

                anim_frame = (anim_frame + 1) % current_sheet.num_frames;
                sprite_dirty = true;

                if now >= tumble_deadline {
                    current_kind = pick_random_kind(&mut rand);
                    current_sheet = Sheet::for_kind(current_kind);
                    current_bmp = current_sheet.bmp();
                    anim_frame = 0;
                    (sprite_x, sprite_y) = (HOME, HOME);
                    next_anim_tick = now + anim_period;
                }
            }
        } else if now >= next_anim_tick {
            next_anim_tick = now + anim_period;

            sprite_x = (sprite_x + current_kind.dx_per_frame()).clamp(0, MAX_X);

            anim_frame += 1;
            if anim_frame >= current_sheet.num_frames {
                anim_frame = 0;

                let next_kind = if current_kind.should_linger() && rand.next_bool(linger_prob) {
                    current_kind
                } else {
                    pick_random_kind(&mut rand)
                };
                if next_kind != current_kind {
                    current_kind = next_kind;
                    current_sheet = Sheet::for_kind(current_kind);
                    current_bmp = current_sheet.bmp();
                    // Keep X so the character stays where a walk left it.
                    sprite_y = HOME;
                }
            }

            sprite_dirty = true;
        }

        if sprite_dirty {
            if let Some(prev_rect) = prev_sprite_rect {
                prev_rect.into_styled(BLACK_FILL).draw(lcd).unwrap();
            }

            let sub_image = current_bmp.sub_image(&sub_frame(anim_frame));
            let sprite_pos = Point::new(sprite_x, sprite_y);
            Image::new(&sub_image, sprite_pos).draw(lcd).unwrap();

            let sprite_rect = Rectangle::new(sprite_pos, Size::new(FRAME, FRAME));

            // Repaint the bar if a sprite scribbled over it or the level moved.
            let battery = BATTERY_PCT.load(Ordering::Relaxed).min(100);
            let bar = battery_bar_bbox();
            let touched = rects_overlap(sprite_rect, bar)
                || prev_sprite_rect.is_some_and(|r| rects_overlap(r, bar));
            if battery != shown_battery || touched {
                draw_battery_bar(lcd, battery);
                shown_battery = battery;
            }

            prev_sprite_rect = Some(sprite_rect);
            lcd.flush().unwrap();
        }

        Timer::after_millis(10).await;
    }
}
