//! Embassy entry point for ARC-DST-01. Select `x4` and exactly one revision.

#![no_std]
#![no_main]

#[cfg(not(feature = "x4"))]
compile_error!("select the supported district variant with --features x4");
#[cfg(not(any(feature = "rev1", feature = "rev2")))]
compile_error!("select exactly one hardware revision: rev1 or rev2");
#[cfg(all(feature = "rev1", feature = "rev2"))]
compile_error!("rev1 and rev2 are mutually exclusive");

use dst_01::network::{self, StatusCadence};
use embassy_executor::Spawner;
use embassy_stm32::can::{CanRx, CanTx, frame::FdFrame};
use embassy_time::{Duration, Instant, Timer};
use link::Presence;
use panic_halt as _;

mod hal;

#[embassy_executor::task(pool_size = 4)]
async fn district_task(hal: hal::District, control: &'static dst_01::tasks::Control) {
    dst_01::tasks::run(hal, control).await;
}

#[embassy_executor::task]
async fn can_rx_task(mut rx: CanRx<'static>, own_id: u32) {
    loop {
        let Ok(envelope) = rx.read_fd().await else {
            continue;
        };
        let frame = envelope.frame;
        if !frame.header().fdcan() || frame.header().rtr() {
            continue;
        }
        let embedded_can::Id::Extended(id) = frame.id() else {
            continue;
        };
        if let Some((index, config)) = network::decode_command(id.as_raw(), frame.data(), own_id) {
            let _ = dst_01::tasks::DISTRICTS[index].configure(config);
        }
    }
}

#[embassy_executor::task]
async fn can_tx_task(mut tx: CanTx<'static>, uid: [u8; 12]) {
    let presence = Presence::new(uid);
    let mut cadence = [StatusCadence::new(); 4];
    let mut next_presence_ms = 0u64;
    loop {
        let now_ms = Instant::now().as_millis();
        if now_ms >= next_presence_ms {
            let frame = FdFrame::new_extended(presence.can_id(), &presence.data())
                .expect("Presence is a valid CAN FD frame");
            let _ = tx.write_fd(&frame).await;
            next_presence_ms = now_ms.saturating_add(5_000);
        }
        for (index, control) in dst_01::tasks::DISTRICTS.iter().enumerate() {
            let Some(status) = control.status() else {
                continue;
            };
            if !cadence[index].due(now_ms, status) {
                continue;
            }
            let wire = network::status_frame(
                presence.network_id,
                index,
                status,
                None, // ADC currently provides only an overcurrent threshold.
                cadence[index].window_ms(now_ms),
            );
            let frame = FdFrame::new_extended(wire.can_id().unwrap(), &wire.data().unwrap())
                .expect("District status is a valid CAN FD frame");
            let _ = tx.write_fd(&frame).await;
            cadence[index].sent(now_ms, status);
        }
        Timer::after(Duration::from_millis(1)).await;
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    // The revision feature chooses the HAL's MCU and generated memory map.
    // Use the internal default clock until the board clock setup is implemented.
    let peripherals = embassy_stm32::init(Default::default());
    let uid = embassy_stm32::uid::uid();
    let network_id = Presence::new(uid).network_id;
    let (districts, can) = hal::init(peripherals);
    for (hal, control) in districts.into_iter().zip(&dst_01::tasks::DISTRICTS) {
        spawner.spawn(district_task(hal, control).unwrap());
    }
    let (tx, rx, _) = can.split();
    spawner.spawn(can_rx_task(rx, network_id).unwrap());
    spawner.spawn(can_tx_task(tx, uid).unwrap());
}
