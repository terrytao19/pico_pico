//! Firmware for a Waveshare RP2040-LCD-1.28 worn on a head-bow: an idle
//! sprite animation on the round LCD that tumbles when you shake your head,
//! plus a battery bar. Core 1 drives the display; core 0 runs the IMU and
//! battery tasks.

#![no_std]
#![no_main]

mod battery;
mod draw;
mod imu;
mod sheet;

use defmt::unwrap;
use embassy_executor::Executor;
use embassy_rp::adc::{self, Adc};
use embassy_rp::bind_interrupts;
use embassy_rp::gpio::{Level, Output, Pull};
use embassy_rp::i2c::{self, I2c};
use embassy_rp::multicore::{spawn_core1, Stack};
use embassy_rp::peripherals::I2C1;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Delay;
use gc9a01::prelude::*;
use static_cell::StaticCell;
use {defmt_rtt as _, panic_probe as _};

static mut CORE1_STACK: Stack<4096> = Stack::new();
static EXECUTOR0: StaticCell<Executor> = StaticCell::new();
static EXECUTOR1: StaticCell<Executor> = StaticCell::new();

/// Raised by `imu_task` on a shake, consumed by `draw::lcd_task`.
pub static SHAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Battery charge 0..=100, written by `battery::battery_task`, drawn by
/// `draw::lcd_task`. Starts at 100 so the bar isn't alarming before the
/// first reading.
pub static BATTERY_PCT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(100);

bind_interrupts!(struct Irqs {
    I2C1_IRQ => i2c::InterruptHandler<I2C1>;
});

type Spi1Mutex = embassy_sync::blocking_mutex::CriticalSectionMutex<
    core::cell::RefCell<
        embassy_rp::spi::Spi<'static, embassy_rp::peripherals::SPI1, embassy_rp::spi::Async>,
    >,
>;
pub type BufferedDriver = gc9a01::Gc9a01<
    gc9a01::prelude::SPIInterface<
        embassy_embedded_hal::shared_bus::blocking::spi::SpiDevice<
            'static,
            embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
            embassy_rp::spi::Spi<'static, embassy_rp::peripherals::SPI1, embassy_rp::spi::Async>,
            embassy_rp::gpio::Output<'static>,
        >,
        embassy_rp::gpio::Output<'static>,
    >,
    gc9a01::prelude::DisplayResolution240x240,
    gc9a01::mode::BufferedGraphics<gc9a01::prelude::DisplayResolution240x240>,
>;
static SPI1_MUTEX: StaticCell<Spi1Mutex> = StaticCell::new();
static LCD: StaticCell<BufferedDriver> = StaticCell::new();

#[cortex_m_rt::entry]
fn main() -> ! {
    let p = embassy_rp::init(Default::default());

    let mut p_lcd_backlight = Output::new(p.PIN_25, Level::Low);
    let p_lcd_dc_select = Output::new(p.PIN_8, Level::Low);
    let p_lcd_cs = Output::new(p.PIN_9, Level::Low);
    let mut p_lcd_reset = Output::new(p.PIN_12, Level::Low);

    let mut spi_config = embassy_rp::spi::Config::default();
    spi_config.frequency = 40_000_000;
    spi_config.phase = embassy_rp::spi::Phase::CaptureOnFirstTransition;
    spi_config.polarity = embassy_rp::spi::Polarity::IdleLow;
    let spi =
        embassy_rp::spi::Spi::new_txonly(p.SPI1, p.PIN_10, p.PIN_11, p.DMA_CH0, spi_config);
    let spi_driver = embassy_embedded_hal::shared_bus::blocking::spi::SpiDevice::new(
        SPI1_MUTEX.init(embassy_sync::blocking_mutex::CriticalSectionMutex::new(
            core::cell::RefCell::new(spi),
        )),
        p_lcd_cs,
    );
    let spi_device = gc9a01::SPIDisplayInterface::new(spi_driver, p_lcd_dc_select);

    // Rotate180 for the head-bow mounting -- handled in the panel (MADCTL).
    let mut lcd = gc9a01::Gc9a01::new(spi_device, DisplayResolution240x240, DisplayRotation::Rotate180);
    lcd.reset(&mut p_lcd_reset, &mut Delay).unwrap();
    lcd.init_with_addr_mode(&mut Delay).unwrap();

    // init_with, not init: the ~113 KiB framebuffer goes straight into the
    // static instead of being built on the stack and copied (that transient
    // overflows the stack).
    let fb = LCD.init_with(|| lcd.into_buffered_graphics());
    p_lcd_backlight.set_high();

    // IMU (QMI8658) on I2C1: GP6 = SDA, GP7 = SCL.
    let mut i2c_config = embassy_rp::i2c::Config::default();
    i2c_config.frequency = 400_000;
    let i2c = I2c::new_async(p.I2C1, p.PIN_7, p.PIN_6, Irqs, i2c_config);

    // Battery: VBAT / 2 on GP29 (ADC3).
    let bat_adc = Adc::new_blocking(p.ADC, adc::Config::default());
    let bat_ch = adc::Channel::new_pin(p.PIN_29, Pull::None);

    spawn_core1(
        p.CORE1,
        unsafe { &mut *core::ptr::addr_of_mut!(CORE1_STACK) },
        move || {
            let executor1 = EXECUTOR1.init(Executor::new());
            executor1.run(|spawner| unwrap!(spawner.spawn(draw::lcd_task(fb))));
        },
    );

    let executor0 = EXECUTOR0.init(Executor::new());
    executor0.run(|spawner| {
        unwrap!(spawner.spawn(imu::imu_task(i2c)));
        unwrap!(spawner.spawn(battery::battery_task(bat_adc, bat_ch)));
    });
}
