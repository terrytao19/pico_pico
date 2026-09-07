use embedded_graphics::pixelcolor::Rgb565;
use tinybmp::Bmp;

/// Every sprite sheet is a horizontal strip of `FRAME`x`FRAME` frames.
pub const FRAME: u32 = 50;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AnimKind {
    Sit,
    SleepLeft,
    SleepRight,
    WalkLeft,
    WalkRight,
    StretchLeft,
    StretchRight,
    Tumble,
}

impl AnimKind {
    /// Idle animations the random picker chooses from. Tumble is excluded --
    /// it's only entered on a shake.
    pub const ALL: [AnimKind; 7] = [
        AnimKind::Sit,
        AnimKind::SleepLeft,
        AnimKind::SleepRight,
        AnimKind::WalkLeft,
        AnimKind::WalkRight,
        AnimKind::StretchLeft,
        AnimKind::StretchRight,
    ];

    pub const fn should_linger(self) -> bool {
        matches!(self, AnimKind::Sit | AnimKind::SleepLeft | AnimKind::SleepRight)
    }

    /// Horizontal drift per frame, for the walk animations.
    pub const fn dx_per_frame(self) -> i32 {
        match self {
            AnimKind::WalkLeft => -2,
            AnimKind::WalkRight => 2,
            _ => 0,
        }
    }
}

pub struct Sheet {
    pub data: &'static [u8],
    pub num_frames: u32,
}

impl Sheet {
    pub fn for_kind(kind: AnimKind) -> &'static Sheet {
        match kind {
            AnimKind::Sit => &SIT,
            AnimKind::SleepLeft => &SLEEP_LEFT,
            AnimKind::SleepRight => &SLEEP_RIGHT,
            AnimKind::WalkLeft => &WALK_LEFT,
            AnimKind::WalkRight => &WALK_RIGHT,
            AnimKind::StretchLeft => &STRETCH_LEFT,
            AnimKind::StretchRight => &STRETCH_RIGHT,
            AnimKind::Tumble => &TUMBLE,
        }
    }

    /// Parse the embedded BMP header. Cheap -- fine to call whenever the
    /// active sheet changes.
    pub fn bmp(&self) -> Bmp<'static, Rgb565> {
        Bmp::from_slice(self.data).unwrap()
    }
}

macro_rules! sheet {
    ($name:ident, $file:literal, $frames:expr) => {
        static $name: Sheet = Sheet {
            data: include_bytes!(concat!(env!("OUT_DIR"), "/sheets/", $file)),
            num_frames: $frames,
        };
    };
}

sheet!(SIT, "sit.bmp", 63);
sheet!(SLEEP_LEFT, "sleep_left.bmp", 24);
sheet!(SLEEP_RIGHT, "sleep_right.bmp", 24);
sheet!(WALK_LEFT, "walk_left.bmp", 16);
sheet!(WALK_RIGHT, "walk_right.bmp", 16);
sheet!(STRETCH_LEFT, "stretch_left.bmp", 36);
sheet!(STRETCH_RIGHT, "stretch_right.bmp", 36);
sheet!(TUMBLE, "tumble.bmp", 6);
