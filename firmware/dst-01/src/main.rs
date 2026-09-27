//! Embassy entry point for ARC-DST-01. Select `x4` and exactly one revision.

#![no_std]
#![no_main]

#[cfg(not(feature = "x4"))]
compile_error!("select the supported district variant with --features x4");
#[cfg(not(any(feature = "rev1", feature = "rev2")))]
compile_error!("select exactly one hardware revision: rev1 or rev2");
#[cfg(all(feature = "rev1", feature = "rev2"))]
compile_error!("rev1 and rev2 are mutually exclusive");

use embassy_executor::Spawner;
use panic_halt as _;

mod hal;

#[embassy_executor::task(pool_size = 4)]
async fn district_task(hal: hal::District, control: &'static dst_01::tasks::Control) {
    dst_01::tasks::run(hal, control).await;
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // The revision feature chooses the HAL's MCU and generated memory map.
    // Use the internal default clock until the board clock setup is implemented.
    let peripherals = embassy_stm32::init(Default::default());
    for (hal, control) in hal::init(peripherals)
        .into_iter()
        .zip(&dst_01::tasks::DISTRICTS)
    {
        spawner.spawn(district_task(hal, control).unwrap());
    }
}
