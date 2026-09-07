//! Animation "brain" -- runs on core 0. Owns the RNG, the idle/tumble state
//! machine and all the timing, and publishes the frame core 1 should draw via
//! [`SCENE`]. Core 1 (`draw::lcd_task`) is then a pure renderer.

use core::sync::atomic::Ordering;

use embassy_futures::select::{select, Either};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer};
use tinyrand::{Probability, Rand, RandRange, StdRand};

use crate::sheet::{AnimKind, Sheet, FRAME};
use crate::{BATTERY_PCT, SCREEN, SHAKE};

/// The sprite frame core 1 should be showing right now.
#[derive(Copy, Clone, PartialEq, Eq)]
pub struct Scene {
    pub kind: AnimKind,
    pub index: u32,
    pub x: i16,
    pub y: i16,
    pub battery: u8,
}

/// Published by [`anim_task`] each tick, consumed by `draw::lcd_task`. A
/// `Signal` (not a channel) so a slow renderer just gets the latest state.
pub static SCENE: Signal<CriticalSectionRawMutex, Scene> = Signal::new();

/// Top-left position that centres a sprite.
const HOME: i32 = (SCREEN - FRAME as i32) / 2;
/// Largest in-bounds sprite X.
const MAX_X: i32 = SCREEN - FRAME as i32;
/// Chance an idle animation repeats instead of switching.
const LINGER_CHANCE: f64 = 0.7;

const IDLE_STEP: Duration = Duration::from_millis(300);

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
const TUMBLE_STEP: Duration = Duration::from_millis(40);
/// A tumble lasts this long; each shake pushes the deadline out again.
const TUMBLE_DURATION: Duration = Duration::from_millis(2500);

fn pick_kind(rand: &mut StdRand) -> AnimKind {
    AnimKind::ALL[rand.next_range(0..AnimKind::ALL.len() as u32) as usize]
}

/// A fresh tumble leg from `from` to a random rim point: `(start, target, step)`.
fn new_leg(rand: &mut StdRand, from: (i32, i32)) -> ((i32, i32), (i32, i32), u32) {
    let idx = rand.next_range(0..TUMBLE_TARGETS.len() as u32) as usize;
    (from, TUMBLE_TARGETS[idx], 0)
}

#[embassy_executor::task]
pub async fn anim_task() {
    let mut rand = StdRand::default();
    let linger = Probability::new(LINGER_CHANCE);

    let mut kind = pick_kind(&mut rand);
    let mut sheet = Sheet::for_kind(kind);
    let mut index = 0u32;
    let (mut x, mut y) = (HOME, HOME);

    // Tumble leg state: interpolate from leg_start to leg_target over
    // TUMBLE_LEG_STEPS ticks, then pick a new rim point.
    let mut leg_start = (x, y);
    let mut leg_target = (x, y);
    let mut leg_step = 0u32;
    let mut tumble_until = Instant::now();

    loop {
        publish(kind, index, x, y);

        let step = if kind == AnimKind::Tumble {
            TUMBLE_STEP
        } else {
            IDLE_STEP
        };
        let shaken = matches!(
            select(Timer::after(step), SHAKE.wait()).await,
            Either::Second(())
        );

        // A shake starts a tumble; a shake during one re-flings it, so a
        // sustained head-shake keeps it flying.
        if shaken {
            if kind != AnimKind::Tumble {
                kind = AnimKind::Tumble;
                sheet = Sheet::for_kind(kind);
                index = 0;
            }
            (leg_start, leg_target, leg_step) = new_leg(&mut rand, (x, y));
            tumble_until = Instant::now() + TUMBLE_DURATION;
            continue;
        }

        if kind == AnimKind::Tumble {
            leg_step += 1;
            if leg_step >= TUMBLE_LEG_STEPS {
                (x, y) = leg_target;
                (leg_start, leg_target, leg_step) = new_leg(&mut rand, leg_target);
            } else {
                let (t, n) = (leg_step as i32, TUMBLE_LEG_STEPS as i32);
                x = leg_start.0 + (leg_target.0 - leg_start.0) * t / n;
                y = leg_start.1 + (leg_target.1 - leg_start.1) * t / n;
            }
            index = (index + 1) % sheet.num_frames;

            if Instant::now() >= tumble_until {
                kind = pick_kind(&mut rand);
                sheet = Sheet::for_kind(kind);
                index = 0;
                (x, y) = (HOME, HOME);
            }
        } else {
            x = (x + kind.dx_per_frame()).clamp(0, MAX_X);
            index += 1;
            if index >= sheet.num_frames {
                index = 0;
                let next = if kind.should_linger() && rand.next_bool(linger) {
                    kind
                } else {
                    pick_kind(&mut rand)
                };
                if next != kind {
                    kind = next;
                    sheet = Sheet::for_kind(kind);
                    // Keep X so the character stays where a walk left it.
                    y = HOME;
                }
            }
        }
    }
}

fn publish(kind: AnimKind, index: u32, x: i32, y: i32) {
    SCENE.signal(Scene {
        kind,
        index,
        x: x as i16,
        y: y as i16,
        battery: BATTERY_PCT.load(Ordering::Relaxed).min(100),
    });
}
