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
use core::cell::RefCell;
#[cfg(feature = "rev1")]
use core::sync::atomic::{AtomicU8, Ordering};
use embassy_executor::Spawner;
use embassy_stm32::can::{CanRx, CanTx, frame::FdFrame};
use embassy_stm32::{flash::Flash, peripherals};
use embassy_sync::{blocking_mutex::{Mutex, raw::{CriticalSectionRawMutex, NoopRawMutex}}, channel::Channel};
use embassy_boot_stm32::{BlockingFirmwareUpdater, FirmwareUpdaterConfig};
#[cfg(feature = "rev1")]
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Instant, Timer};
use link::{Presence, update::{self, ImageKind, Phase, Status}};
use panic_halt as _;
#[cfg(feature = "rev1")]
mod usb_console;

mod hal;
#[path = "../../shared/update_receiver.rs"]
mod update_receiver;

static UPDATE_RESP: Channel<CriticalSectionRawMutex, Status, 4> = Channel::new();
#[cfg(feature = "rev1")]
static ERASE_REQUEST: Channel<CriticalSectionRawMutex, (), 1> = Channel::new();
#[cfg(feature = "rev1")]
static ERASE_RESPONSE: Channel<CriticalSectionRawMutex, EraseResult, 1> = Channel::new();
#[cfg(feature = "rev1")]
static LED_MODE: AtomicU8 = AtomicU8::new(dst_01::console::LedMode::Auto as u8);

#[cfg(feature = "rev1")]
#[derive(Clone, Copy)]
enum EraseResult {
    Erased,
    BootLocked,
    Failed,
}

#[embassy_executor::task(pool_size = 4)]
async fn district_task(hal: hal::District, control: &'static dst_01::tasks::Control) {
    dst_01::tasks::run(hal, control).await;
}

#[cfg(feature = "rev1")]
#[embassy_executor::task]
async fn rgb_blink_task(mut led: hal::RgbLed) {
    // Three primaries, their pairs, and white; both LEDs blink together.
    const COLORS: [(bool, bool, bool); 7] = [
        (true, false, false),
        (false, true, false),
        (false, false, true),
        (true, true, false),
        (false, true, true),
        (true, false, true),
        (true, true, true),
    ];
    let mut phase: usize = 0;
    loop {
        let mode = dst_01::console::LedMode::from_u8(LED_MODE.load(Ordering::Relaxed));
        let color = if let Some(color) = mode.color() {
            color
        } else {
            let color = if phase.is_multiple_of(2) {
                COLORS[phase / 2]
            } else {
                (false, false, false)
            };
            phase = (phase + 1) % (COLORS.len() * 2);
            color
        };
        led.show(color.0, color.1, color.2);
        Timer::after(Duration::from_millis(300)).await;
    }
}

#[embassy_executor::task]
async fn can_rx_task(mut rx: CanRx<'static>, own_id: u32, flash: embassy_stm32::Peri<'static, peripherals::FLASH>) {
    let flash = Mutex::<NoopRawMutex, _>::new(RefCell::new(Flash::new_blocking(flash)));
    let config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
    let mut aligned = [0u8; 8];
    let updater = BlockingFirmwareUpdater::new(config, &mut aligned);
    #[cfg(feature = "rev1")]
    let kind = ImageKind::DstRev1;
    #[cfg(feature = "rev2")]
    let kind = ImageKind::DstRev2;
    let max_size = if cfg!(feature = "rev1") { 240 * 1024 } else { 54 * 1024 };
    let mut update = update_receiver::Receiver::new(updater, own_id, kind, max_size);
    update.confirm_boot();
    let mut updating = false;
    loop {
        #[cfg(feature = "rev1")]
        let envelope = match select(rx.read_fd(), ERASE_REQUEST.receive()).await {
            Either::First(Ok(envelope)) => envelope,
            Either::First(Err(_)) => continue,
            Either::Second(()) => {
                // BOOT_LOCK forces main-flash boot even when EMPTY is set.
                if unsafe { core::ptr::read_volatile(0x4002_2080 as *const u32) } & (1 << 16) != 0 {
                    ERASE_RESPONSE.send(EraseResult::BootLocked).await;
                    continue;
                }
                dst_01::sync::inhibit_for_update();
                for district in &dst_01::tasks::DISTRICTS {
                    let _ = district.configure(Default::default());
                }
                Timer::after(Duration::from_millis(10)).await;
                // STM32G0B1 page 0 is 2 KiB. Erasing its first word triggers
                // the hardware empty check on option-byte reload or power-on.
                let erased = flash.lock(|flash| flash.borrow_mut().blocking_erase(0, 2048)).is_ok()
                    && unsafe { core::ptr::read_volatile(0x0800_0000 as *const u32) }
                        == 0xffff_ffff;
                if erased {
                    // EMPTY survives a software reset, so update it here too.
                    let acr = 0x4002_2000 as *mut u32;
                    unsafe {
                        core::ptr::write_volatile(
                            acr,
                            core::ptr::read_volatile(acr) | (1 << 16),
                        );
                    }
                }
                ERASE_RESPONSE.send(if erased { EraseResult::Erased } else { EraseResult::Failed }).await;
                if erased {
                    Timer::after(Duration::from_millis(200)).await;
                    cortex_m::peripheral::SCB::sys_reset();
                }
                continue;
            }
        };
        #[cfg(feature = "rev2")]
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
        if let Some(begin) = update::Begin::decode(id.as_raw(), frame.data()) {
            if begin.target == own_id && begin.kind == kind && (8..=max_size).contains(&begin.size) {
                updating = true;
                #[cfg(feature = "rev1")]
                dst_01::sync::inhibit_for_update();
                for district in &dst_01::tasks::DISTRICTS {
                    let _ = district.configure(Default::default());
                }
                Timer::after(Duration::from_millis(10)).await;
            }
        }
        if let Some(status) = update.handle(id.as_raw(), frame.data()) {
            UPDATE_RESP.send(status).await;
            if status.phase == Phase::Rebooting {
                Timer::after(Duration::from_millis(200)).await;
                cortex_m::peripheral::SCB::sys_reset();
            }
            continue;
        }
        if updating { continue; }
        #[cfg(feature = "rev1")]
        {
            dst_01::sync::receive(id.as_raw(), frame.data(), own_id);
            if let Some(command) = link::ThrottleSet::decode(id.as_raw(), frame.data()) {
                dst_01::sync::apply(command, own_id);
            }
        }
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
    let mut last_dcc = None;
    let mut next_dcc_ms = 0;
    loop {
        let now_ms = Instant::now().as_millis();
        while let Ok(status) = UPDATE_RESP.try_receive() {
            let frame = FdFrame::new_extended(update::status_id(presence.network_id).unwrap(), &status.data()).unwrap();
            let _ = tx.write_fd(&frame).await;
        }
        #[cfg(feature = "rev1")]
        let dcc_status = dst_01::sync::status(presence.network_id);
        #[cfg(feature = "rev2")]
        let dcc_status = link::DccStatus {
            network_id: presence.network_id,
            kind: 0,
            permitted: false,
            transmitting: false,
        };
        if last_dcc != Some(dcc_status) || now_ms >= next_dcc_ms {
            let frame =
                FdFrame::new_extended(dcc_status.can_id().unwrap(), &dcc_status.data().unwrap())
                    .unwrap();
            let _ = tx.write_fd(&frame).await;
            last_dcc = Some(dcc_status);
            next_dcc_ms = now_ms + 1_000;
        }
        #[cfg(feature = "rev1")]
        for index in 0..dcc::locos::CAPACITY {
            if let Some(status) = dst_01::sync::throttle_status(index, now_ms, presence.network_id)
            {
                let frame =
                    FdFrame::new_extended(status.can_id().unwrap(), &status.data().unwrap())
                        .unwrap();
                let _ = tx.write_fd(&frame).await;
                dst_01::sync::throttle_sent(index, status.sequence, now_ms);
            }
        }
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
    let config = embassy_stm32::Config::default();
    #[cfg(feature = "rev1")]
    let config = {
        use embassy_stm32::{
            rcc::{Hse, HseMode, Sysclk, mux::Fdcansel},
            time::Hertz,
        };
        let mut config = config;
        // Rev1's preserved hardware/clock source specifies a 40 MHz oscillator.
        config.rcc.hse = Some(Hse {
            freq: Hertz(40_000_000),
            mode: HseMode::Bypass,
        });
        config.rcc.sys = Sysclk::HSE;
        config.rcc.mux.fdcansel = Fdcansel::HSE;
        if let Some(hsi48) = &mut config.rcc.hsi48 {
            hsi48.sync_from_usb = true;
        }
        config
    };
    let peripherals = embassy_stm32::init(config);
    let uid = embassy_stm32::uid::uid();
    let network_id = Presence::new(uid).network_id;
    #[cfg(feature = "rev1")]
    let (districts, can, usb, flash, rgb) = hal::init(peripherals);
    #[cfg(feature = "rev2")]
    let (districts, can, flash) = hal::init(peripherals);
    #[cfg(feature = "rev1")]
    spawner.spawn(rgb_blink_task(rgb).unwrap());
    for (hal, control) in districts.into_iter().zip(&dst_01::tasks::DISTRICTS) {
        spawner.spawn(district_task(hal, control).unwrap());
    }
    let (tx, rx, _) = can.split();
    spawner.spawn(can_rx_task(rx, network_id, flash).unwrap());
    spawner.spawn(can_tx_task(tx, uid).unwrap());
    #[cfg(feature = "rev1")]
    spawner.spawn(usb_console::run(usb, network_id).unwrap());
}
