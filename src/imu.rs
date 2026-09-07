use defmt::{error, info, warn};
use embassy_rp::i2c::{Async, I2c};
use embassy_rp::peripherals::I2C1;
use embassy_time::{Delay, Instant, Timer};
use ph_qmi8658::{
    accel_lsb_per_g, AccelConfig, AccelOutputDataRate, AccelRange, Config, I2cConfig, Qmi8658Address,
    Qmi8658I2c,
};

use crate::SHAKE;

const ACCEL_RANGE: AccelRange = AccelRange::G16;
/// Poll rate: 50 Hz, ample for a ~4 Hz head shake (sensor ODR is 250 Hz).
const SAMPLE_PERIOD_MS: u64 = 20;

// Shake detection runs on *jerk* -- the sample-to-sample change in
// acceleration summed over the live axes. That ignores gravity (orientation
// doesn't matter) and a stuck axis reads a constant, so its jerk is zero and
// it drops out on its own. Thresholds tuned from logs of the real gesture:
// resting barely registers a hit; a shake lands 7+ per window.

/// Per-live-axis jerk (mg) for a sample to count as "energetic"; the real
/// threshold is this times the live-axis count.
const SHAKE_JERK_MG_PER_AXIS: u32 = 500;
/// Sliding window, in samples (15 * 20 ms = 300 ms).
const SHAKE_WINDOW: u32 = 15;
/// Energetic samples within the window needed to fire.
const SHAKE_MIN_HITS: u32 = 5;
/// Minimum gap between fires; short enough that a held shake keeps re-firing.
const SHAKE_COOLDOWN_MS: u64 = 1000;
/// A raw axis within this of i16 full-scale is saturated (usually a dead
/// axis) and excluded from the jerk sum.
const AXIS_SATURATED: i32 = 32000;

#[embassy_executor::task]
pub async fn imu_task(i2c: I2c<'static, I2C1, Async>) {
    let accel = AccelConfig::new(ACCEL_RANGE, AccelOutputDataRate::Hz250);
    let config = Config::new().with_accel_config(accel).without_gyro();
    // This QMI8658 outputs little-endian; the crate defaults to big-endian.
    let i2c_config = I2cConfig::new(Qmi8658Address::Secondary.addr()).with_big_endian(false);
    let mut imu: Qmi8658I2c<I2c<'static, I2C1, Async>> =
        Qmi8658I2c::with_i2c_config(i2c, None, None, config, i2c_config);

    let mut delay = Delay;
    // 0x6B on this board; try 0x6A too rather than fail silently.
    let addrs = [Qmi8658Address::Secondary.addr(), Qmi8658Address::Primary.addr()];
    match imu.init_with_addresses(&mut delay, &addrs).await {
        Ok(addr) => info!("QMI8658 @ {=u8:#x}", addr),
        Err(e) => {
            error!("QMI8658 init failed: {}", defmt::Debug2Format(&e));
            return;
        }
    }

    let lsb_per_g = accel_lsb_per_g(ACCEL_RANGE) as i64;
    let window_mask: u32 = (1 << SHAKE_WINDOW) - 1;
    let mg = |raw: i16| (raw as i64 * 1000 / lsb_per_g) as i32;

    let mut history: u32 = 0;
    let mut last_shake = Instant::from_ticks(0);
    let mut tick: u32 = 0;
    // Previous per-axis reading (mg) and per-axis saturation flags.
    let mut prev: Option<([i32; 3], [bool; 3])> = None;

    loop {
        tick = tick.wrapping_add(1);

        let sample = match imu.read_accel_raw().await {
            Ok(s) => s.data,
            Err(e) => {
                if tick.is_multiple_of(100) {
                    warn!("IMU read error: {}", defmt::Debug2Format(&e));
                }
                Timer::after_millis(SAMPLE_PERIOD_MS).await;
                continue;
            }
        };

        let cur = [mg(sample.x), mg(sample.y), mg(sample.z)];
        let sat = [
            (sample.x as i32).abs() >= AXIS_SATURATED,
            (sample.y as i32).abs() >= AXIS_SATURATED,
            (sample.z as i32).abs() >= AXIS_SATURATED,
        ];

        // Jerk over axes that are live in both this sample and the previous.
        let (jerk, live) = match prev {
            Some((pcur, psat)) => {
                let mut j = 0u32;
                let mut live = 0u32;
                for i in 0..3 {
                    if !sat[i] && !psat[i] {
                        j += (cur[i] - pcur[i]).unsigned_abs();
                        live += 1;
                    }
                }
                (j, live)
            }
            None => (0, 3),
        };
        prev = Some((cur, sat));

        if sat.contains(&true) && tick.is_multiple_of(250) {
            warn!(
                "IMU axis saturated (x={} y={} z={}) -- power-cycle?",
                sat[0], sat[1], sat[2]
            );
        }

        // Need >= 2 live axes to tell a shake from a linear bump.
        let energetic = live >= 2 && jerk > SHAKE_JERK_MG_PER_AXIS * live;
        history = ((history << 1) | energetic as u32) & window_mask;

        let now = Instant::now();
        if history.count_ones() >= SHAKE_MIN_HITS
            && now.duration_since(last_shake).as_millis() > SHAKE_COOLDOWN_MS
        {
            SHAKE.signal(());
            last_shake = now;
            history = 0;
        }

        Timer::after_millis(SAMPLE_PERIOD_MS).await;
    }
}
