//! ARC-PSU-01: link power, a bounded locomotive table, and repeating DCC packets.

#![no_std]
#![no_main]

use core::cell::RefCell;
use critical_section::Mutex;
use embassy_executor::Spawner;
use embassy_stm32::{
    bind_interrupts,
    can::{self, CanConfigurator, CanRx, config::FrameTransmissionConfig, frame::FdFrame},
    gpio::{Input, Level, Output, OutputType, Pull, Speed},
    i2c::{self, I2c},
    interrupt,
    mode::Blocking,
    peripherals,
    rcc::{Hse, HseMode, Sysclk},
    time::Hertz,
    timer::{
        Channel,
        low_level::CountingMode,
        simple_pwm::{PwmPin, SimplePwm},
    },
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Duration, Instant, Timer};
use link::{Presence, PsuStatus, ThrottleSet};
use panic_halt as _;
use psu_01::{
    dcc,
    locos::{CAPACITY, Table},
    power::{Controller, Reading, State},
};
use static_cell::StaticCell;
use stm32_metapac as pac;

static DCC_PWM: StaticCell<SimplePwm<'static, peripherals::TIM3>> = StaticCell::new();

struct DccEngine {
    table: Table,
    packet: dcc::Packet,
    bit_index: usize,
}

impl DccEngine {
    const fn new() -> Self {
        Self {
            table: Table::new(),
            packet: dcc::Packet::idle(),
            bit_index: 1,
        }
    }

    fn next_bit(&mut self) -> bool {
        let bit = self.packet.bit(self.bit_index);
        self.bit_index += 1;
        if self.bit_index == self.packet.len_bits() {
            self.packet = self.table.next_packet();
            self.bit_index = 0;
        }
        bit
    }
}

static ENGINE: Mutex<RefCell<DccEngine>> = Mutex::new(RefCell::new(DccEngine::new()));

struct DccInterrupt;
impl interrupt::typelevel::Handler<interrupt::typelevel::TIM3> for DccInterrupt {
    unsafe fn on_interrupt() {
        let regs = pac::TIM3;
        if !regs.sr().read().uif() {
            return;
        }
        regs.sr().modify(|w| w.set_uif(false));
        let bit = critical_section::with(|cs| ENGINE.borrow(cs).borrow_mut().next_bit());
        let ticks = dcc::bit_ticks(bit, 40_000_000);
        regs.arr().write(|w| w.set_arr(ticks - 1));
        regs.ccr(0).write(|w| w.set_ccr(ticks / 2));
    }
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
        if matches!(
            TELEMETRY.try_get(),
            Some(Telemetry {
                state: State::Online,
                ..
            })
        ) && let Some(command) = ThrottleSet::decode(id.as_raw(), frame.data())
        {
            critical_section::with(|cs| {
                let _ = ENGINE.borrow(cs).borrow_mut().table.apply(command, own_id);
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
    mut dcc_de: Output<'static>,
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
        if control.dcc_enabled() {
            link.set_high();
            dcc_de.set_high();
            can_stb.set_low();
        } else {
            dcc_de.set_low();
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
    let mut next_presence_ms = 0u64;
    loop {
        let now_ms = Instant::now().as_millis();
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
                    dcc_enabled: telemetry.state == State::Online,
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
                ENGINE.borrow(cs).borrow().table.status_due(
                    index,
                    now_ms,
                    presence.network_id,
                    true,
                )
            });
            if let Some(status) = status {
                let frame =
                    FdFrame::new_extended(status.can_id().unwrap(), &status.data().unwrap())
                        .unwrap();
                let _ = tx.write_fd(&frame).await;
                critical_section::with(|cs| {
                    ENGINE.borrow(cs).borrow_mut().table.status_sent(
                        index,
                        status.sequence,
                        now_ms,
                    );
                });
            }
        }
        Timer::after(Duration::from_millis(10)).await;
    }
}

fn start_dcc(
    tim: embassy_stm32::Peri<'static, peripherals::TIM3>,
    pin: embassy_stm32::Peri<'static, peripherals::PB4>,
) {
    let pwm = DCC_PWM.init(SimplePwm::new(
        tim,
        Some(PwmPin::new(pin, OutputType::PushPull)),
        None,
        None,
        None,
        Hertz(8_620),
        CountingMode::EdgeAlignedUp,
    ));
    let regs = pac::TIM3;
    let ticks = dcc::bit_ticks(dcc::Packet::idle().bit(0), 40_000_000);
    regs.psc().write_value(0);
    regs.arr().write(|w| w.set_arr(ticks - 1));
    regs.ccr(0).write(|w| w.set_ccr(ticks / 2));
    regs.egr().write(|w| w.set_ug(true));
    regs.sr().modify(|w| w.set_uif(false));
    pwm.channel(Channel::Ch1).enable();
    regs.dier().modify(|w| w.set_uie(true));
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
    start_dcc(p.TIM3, p.PB4);

    let mut can = CanConfigurator::new(p.FDCAN1, p.PD0, p.PD1, Irqs);
    can.set_bitrate(500_000);
    can.set_config(
        can.config()
            .set_frame_transmit(FrameTransmissionConfig::AllowFdCan),
    );
    let (tx, rx, _) = can.into_normal_mode().split();
    let uid = embassy_stm32::uid::uid();
    spawner.spawn(power_task(link, dcc_de, can_stb, bus, alert).unwrap());
    spawner.spawn(can_task(tx, uid).unwrap());
    spawner.spawn(can_rx_task(rx, Presence::new(uid).network_id).unwrap());
}
