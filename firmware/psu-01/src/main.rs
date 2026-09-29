//! ARC-PSU-01: link power, a bounded locomotive table, and repeating DCC packets.

#![no_std]
#![no_main]

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use embassy_boot_stm32::{BlockingFirmwareUpdater, FirmwareUpdaterConfig};
use critical_section::Mutex;
use dcc::stm32::Transmitter;
use embassy_executor::Spawner;
use embassy_stm32::{
    bind_interrupts,
    can::{self, CanConfigurator, CanRx, config::FrameTransmissionConfig, frame::FdFrame},
    gpio::{AfType, Flex, Input, Level, Output, OutputType, Pull, Speed},
    i2c::{self, I2c},
    interrupt,
    mode::Blocking,
    peripherals,
    rcc::{Hse, HseMode, Sysclk},
    time::Hertz,
    flash::Flash,
};
use embassy_sync::{blocking_mutex::{Mutex as BlockingMutex, raw::{CriticalSectionRawMutex, NoopRawMutex}}, channel::Channel, watch::Watch};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use link::{DccStatus, Presence, PsuStatus, ThrottleSet, update::{self, ImageKind, Phase, Status}};
use panic_halt as _;
use psu_01::{
    dcc,
    locos::CAPACITY,
    power::{Controller, Reading, State},
};

static DCC: Mutex<RefCell<Option<Transmitter<peripherals::TIM3>>>> = Mutex::new(RefCell::new(None));
static UPDATING: AtomicBool = AtomicBool::new(false);
static UPDATE_RESP: Channel<CriticalSectionRawMutex, Status, 4> = Channel::new();
#[path = "../../shared/update_receiver.rs"]
mod update_receiver;

struct DccInterrupt;
impl interrupt::typelevel::Handler<interrupt::typelevel::TIM3> for DccInterrupt {
    unsafe fn on_interrupt() {
        critical_section::with(|cs| {
            if let Some(dcc) = DCC.borrow(cs).borrow_mut().as_mut() {
                dcc.on_update();
            }
        });
    }
}

#[embassy_executor::task]
async fn can_rx_task(mut rx: CanRx<'static>, own_id: u32, flash: embassy_stm32::Peri<'static, peripherals::FLASH>) {
    let flash = BlockingMutex::<NoopRawMutex, _>::new(RefCell::new(Flash::new_blocking(flash)));
    let config = FirmwareUpdaterConfig::from_linkerfile_blocking(&flash, &flash);
    let mut aligned = [0u8; 8];
    let updater = BlockingFirmwareUpdater::new(config, &mut aligned);
    let mut update = update_receiver::Receiver::new(updater, own_id, ImageKind::Psu, 54 * 1024);
    let mut boot_confirmed = false;
    loop {
        if !boot_confirmed && matches!(TELEMETRY.try_get(), Some(Telemetry { state: State::Online, .. })) {
            update.confirm_boot();
            boot_confirmed = true;
        }
        let Ok(Ok(envelope)) = with_timeout(Duration::from_millis(100), rx.read_fd()).await else {
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
            if begin.target == own_id && begin.kind == ImageKind::Psu && (8..=54 * 1024).contains(&begin.size) {
                UPDATING.store(true, Ordering::Relaxed);
                critical_section::with(|cs| {
                    if let Some(dcc) = DCC.borrow(cs).borrow_mut().as_mut() {
                        dcc.update_power(false);
                    }
                });
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
        if UPDATING.load(Ordering::Relaxed) { continue; }
        critical_section::with(|cs| {
            DCC.borrow(cs)
                .borrow_mut()
                .as_mut()
                .unwrap()
                .permission
                .receive(id.as_raw(), frame.data(), own_id);
        });
        if matches!(
            TELEMETRY.try_get(),
            Some(Telemetry {
                state: State::Online,
                ..
            })
        ) && let Some(command) = ThrottleSet::decode(id.as_raw(), frame.data())
        {
            critical_section::with(|cs| {
                let _ = DCC
                    .borrow(cs)
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .engine
                    .table
                    .apply(command, own_id);
            });
        }
    }
}

bind_interrupts!(struct Irqs {
    TIM3 => DccInterrupt;
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
});

#[derive(Clone, Copy)]
struct Telemetry {
    state: State,
    reading: Option<Reading>,
}

static TELEMETRY: Watch<CriticalSectionRawMutex, Telemetry, 1> = Watch::new_with(Telemetry {
    state: State::Off,
    reading: None,
});

const MONITOR_ADDRESS: u8 = 0x44; // INA226 A1=VS, A0=GND.

fn read_register(
    bus: &mut I2c<'static, Blocking, i2c::Master>,
    register: u8,
) -> Result<u16, i2c::Error> {
    let mut bytes = [0; 2];
    bus.blocking_write_read(MONITOR_ADDRESS, &[register], &mut bytes)?;
    Ok(u16::from_be_bytes(bytes))
}

fn measure(bus: &mut I2c<'static, Blocking, i2c::Master>) -> Option<Reading> {
    let shunt = read_register(bus, 0x01).ok()? as i16;
    let voltage = read_register(bus, 0x02).ok()?;
    // INA226: 2.5 µV/shunt count and 1.25 mV/bus count. At 20 mΩ,
    // one shunt count represents 0.125 mA.
    Some(Reading {
        millivolts: ((u32::from(voltage) * 5) / 4).min(u32::from(u16::MAX)) as u16,
        milliamps: shunt / 8,
    })
}

#[embassy_executor::task]
async fn power_task(
    mut link: Output<'static>,
    mut can_stb: Output<'static>,
    mut bus: I2c<'static, Blocking, i2c::Master>,
    _alert: Input<'static>,
) {
    let mut control = Controller::new();
    // The monitor is powered before the link switch. If it cannot answer, keep
    // the backbone and the DCC line driver off for this boot.
    if read_register(&mut bus, 0x00).is_ok() {
        control.start(Instant::now().as_millis());
        link.set_high();
    } else {
        control.fail();
    }
    loop {
        let reading = if control.link_enabled() {
            measure(&mut bus)
        } else {
            None
        };
        control.sample(Instant::now().as_millis(), reading);
        critical_section::with(|cs| {
            DCC.borrow(cs)
                .borrow_mut()
                .as_mut()
                .unwrap()
                .update_power(control.dcc_enabled() && !UPDATING.load(Ordering::Relaxed));
        });
        if control.dcc_enabled() {
            link.set_high();
            can_stb.set_low();
        } else {
            can_stb.set_high();
            if !control.link_enabled() {
                link.set_low();
            }
        }
        TELEMETRY.sender().send(Telemetry {
            state: control.state(),
            reading,
        });
        Timer::after(Duration::from_millis(10)).await;
    }
}

#[embassy_executor::task]
async fn can_task(mut tx: can::CanTx<'static>, uid: [u8; 12]) {
    let presence = Presence::new(uid);
    let mut last_state = State::Off;
    let mut next_status_ms = 1_000u64;
    let mut last_dcc = None;
    let mut next_dcc_ms = 0;
    let mut next_presence_ms = 0u64;
    loop {
        let now_ms = Instant::now().as_millis();
        while let Ok(status) = UPDATE_RESP.try_receive() {
            let frame = FdFrame::new_extended(update::status_id(presence.network_id).unwrap(), &status.data()).unwrap();
            let _ = tx.write_fd(&frame).await;
        }
        let telemetry = TELEMETRY.try_get();
        if !matches!(
            telemetry,
            Some(Telemetry {
                state: State::Online,
                ..
            })
        ) {
            Timer::after(Duration::from_millis(10)).await;
            continue;
        }
        let dcc_status = critical_section::with(|cs| {
            let slot = DCC.borrow(cs).borrow();
            let dcc = slot.as_ref().unwrap();
            DccStatus {
                network_id: presence.network_id,
                kind: 1,
                permitted: dcc.permission.granted(),
                transmitting: dcc.enabled(),
            }
        });
        if last_dcc != Some(dcc_status) || now_ms >= next_dcc_ms {
            let frame =
                FdFrame::new_extended(dcc_status.can_id().unwrap(), &dcc_status.data().unwrap())
                    .unwrap();
            let _ = tx.write_fd(&frame).await;
            last_dcc = Some(dcc_status);
            next_dcc_ms = now_ms + 1_000;
        }
        if now_ms >= next_presence_ms {
            let frame = FdFrame::new_extended(presence.can_id(), &presence.data()).unwrap();
            let _ = tx.write_fd(&frame).await;
            next_presence_ms = now_ms.saturating_add(5_000);
        }
        if let Some(telemetry) = telemetry {
            if telemetry.state != last_state || now_ms >= next_status_ms {
                let reading = telemetry.reading;
                let status = PsuStatus {
                    network_id: presence.network_id,
                    state: match telemetry.state {
                        State::Off => 0,
                        State::Starting => 1,
                        State::Online => 2,
                        State::Fault => 3,
                    },
                    dcc_enabled: dcc_status.transmitting,
                    monitor_valid: reading.is_some(),
                    link_mv: reading.map_or(0, |r| r.millivolts),
                    link_ma: reading.map_or(0, |r| r.milliamps.max(0) as u16),
                    uptime_ms: now_ms.min(u64::from(u32::MAX)) as u32,
                };
                let frame =
                    FdFrame::new_extended(status.can_id().unwrap(), &status.data().unwrap())
                        .unwrap();
                let _ = tx.write_fd(&frame).await;
                last_state = telemetry.state;
                if now_ms >= next_status_ms {
                    next_status_ms = now_ms.saturating_add(1_000);
                }
            }
        }
        for index in 0..CAPACITY {
            let status = critical_section::with(|cs| {
                DCC.borrow(cs)
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .engine
                    .table
                    .status_due(index, now_ms, presence.network_id, dcc_status.transmitting)
            });
            if let Some(status) = status {
                let frame =
                    FdFrame::new_extended(status.can_id().unwrap(), &status.data().unwrap())
                        .unwrap();
                let _ = tx.write_fd(&frame).await;
                critical_section::with(|cs| {
                    DCC.borrow(cs)
                        .borrow_mut()
                        .as_mut()
                        .unwrap()
                        .engine
                        .table
                        .status_sent(index, status.sequence, now_ms);
                });
            }
        }
        Timer::after(Duration::from_millis(10)).await;
    }
}

fn start_dcc(
    tim: embassy_stm32::Peri<'static, peripherals::TIM3>,
    pin: embassy_stm32::Peri<'static, peripherals::PB4>,
    enable: Output<'static>,
) {
    let mut pin = Flex::new(pin);
    pin.set_as_af_unchecked(1, AfType::output(OutputType::PushPull, Speed::VeryHigh));
    critical_section::with(|cs| {
        *DCC.borrow(cs).borrow_mut() = Some(Transmitter::new(tim, pin, enable));
    });
    unsafe {
        <interrupt::typelevel::TIM3 as interrupt::typelevel::Interrupt>::set_priority(
            interrupt::Priority::P0,
        );
        <interrupt::typelevel::TIM3 as interrupt::typelevel::Interrupt>::unpend();
        <interrupt::typelevel::TIM3 as interrupt::typelevel::Interrupt>::enable();
    }
}

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let mut config = embassy_stm32::Config::default();
    config.rcc.hse = Some(Hse {
        freq: Hertz(40_000_000),
        mode: HseMode::Bypass,
    });
    config.rcc.sys = Sysclk::HSE;
    let p = embassy_stm32::init(config);

    let link = Output::new(p.PD3, Level::Low, Speed::Low);
    let dcc_de = Output::new(p.PB5, Level::Low, Speed::Low);
    let can_stb = Output::new(p.PD2, Level::High, Speed::Low);
    let _can_shdn = Output::new(p.PA15, Level::Low, Speed::Low);
    let alert = Input::new(p.PB6, Pull::Up);
    let mut i2c_config = i2c::Config::default();
    i2c_config.frequency = Hertz(400_000);
    i2c_config.timeout = Duration::from_millis(1);
    let bus = I2c::new_blocking(p.I2C1, p.PB8, p.PB9, i2c_config);
    start_dcc(p.TIM3, p.PB4, dcc_de);

    let mut can = CanConfigurator::new(p.FDCAN1, p.PD0, p.PD1, Irqs);
    can.set_bitrate(500_000);
    can.set_config(
        can.config()
            .set_frame_transmit(FrameTransmissionConfig::AllowFdCan),
    );
    let (tx, rx, _) = can.into_normal_mode().split();
    let uid = embassy_stm32::uid::uid();
    spawner.spawn(power_task(link, can_stb, bus, alert).unwrap());
    spawner.spawn(can_task(tx, uid).unwrap());
    spawner.spawn(can_rx_task(rx, Presence::new(uid).network_id, p.FLASH).unwrap());
}
