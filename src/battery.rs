use core::sync::atomic::Ordering;

use defmt::warn;
use embassy_rp::adc::{Adc, Blocking, Channel};
use embassy_time::{Duration, Timer};

use crate::BATTERY_PCT;

// VBAT is divided 100k/100k into GP29 / ADC3; the ADC is 12-bit at 3.3 V.
const DIVIDER_RATIO: u32 = 2;
const ADC_REF_MV: u32 = 3300;
const ADC_FULL_SCALE: u32 = 4095;

// Single-cell Li-ion voltage -> charge, linear. On USB the charger holds
// VBAT at ~4.20 V (a full cell), so that reads 100 %.
const EMPTY_MV: u32 = 3300;
const FULL_MV: u32 = 4200;
/// Below this the reading is a glitch or the RC filter hasn't settled.
const PLAUSIBLE_MIN_MV: u32 = 2500;

const SAMPLES: u32 = 16;
const PERIOD: Duration = Duration::from_secs(5);

#[embassy_executor::task]
pub async fn battery_task(mut adc: Adc<'static, Blocking>, mut ch: Channel<'static>) {
    // Let the RC filter settle, then discard the first conversions.
    Timer::after_millis(100).await;
    for _ in 0..8 {
        let _ = adc.blocking_read(&mut ch);
    }

    loop {
        let mut sum = 0u32;
        for _ in 0..SAMPLES {
            sum += adc.blocking_read(&mut ch).unwrap_or(0) as u32;
        }
        let mv = (sum / SAMPLES) * ADC_REF_MV * DIVIDER_RATIO / ADC_FULL_SCALE;

        if mv >= PLAUSIBLE_MIN_MV {
            let pct = (mv.clamp(EMPTY_MV, FULL_MV) - EMPTY_MV) * 100 / (FULL_MV - EMPTY_MV);
            BATTERY_PCT.store(pct as u8, Ordering::Relaxed);
        } else {
            warn!("battery: implausible read {=u32} mV", mv);
        }

        Timer::after(PERIOD).await;
    }
}
