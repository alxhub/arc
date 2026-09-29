//! Rev1 backbone transmitter. Disabled at boot; source arbitration must enable it.
//! Generating TX does not prove receiver readiness and never enables a district.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};
use critical_section::Mutex;
use dcc::{locos::Apply, stm32::Transmitter};
use embassy_stm32::{
    Peri, bind_interrupts,
    gpio::{AfType, Flex, Input, Level, Output, OutputType, Pull, Speed},
    interrupt::{self, typelevel::Interrupt},
    peripherals::{PC2, PC12, PC13, TIM14},
};

static POWER: Mutex<RefCell<Option<Input<'static>>>> = Mutex::new(RefCell::new(None));
static UPDATING: AtomicBool = AtomicBool::new(false);

static DCC: Mutex<RefCell<Option<Transmitter<TIM14>>>> = Mutex::new(RefCell::new(None));

struct DccInterrupt;
impl interrupt::typelevel::Handler<interrupt::typelevel::TIM14> for DccInterrupt {
    unsafe fn on_interrupt() {
        critical_section::with(|cs| {
            if let Some(dcc) = DCC.borrow(cs).borrow_mut().as_mut() {
                dcc.on_update();
            }
        });
    }
}

bind_interrupts!(struct Irqs { TIM14 => DccInterrupt; });

pub fn init(
    tim: Peri<'static, TIM14>,
    tx: Peri<'static, PC12>,
    de: Peri<'static, PC13>,
    power: Peri<'static, PC2>,
) {
    let enable = Output::new(de, Level::Low, Speed::Low);
    let mut tx = Flex::new(tx);
    // Preserved rev1 pin map: TIM14_CH1 on PC12, AF2.
    tx.set_as_af_unchecked(2, AfType::output(OutputType::PushPull, Speed::VeryHigh));
    critical_section::with(|cs| {
        *POWER.borrow(cs).borrow_mut() = Some(Input::new(power, Pull::Up));
        *DCC.borrow(cs).borrow_mut() = Some(Transmitter::new(tim, tx, enable));
    });
    interrupt::typelevel::TIM14::set_priority(interrupt::Priority::P0);
    interrupt::typelevel::TIM14::unpend();
    // SAFETY: Irqs binds TIM14 above; the transmitter is initialized.
    unsafe {
        interrupt::typelevel::TIM14::enable();
    }
}

/// Called periodically; power qualification gates output without clearing permission.
pub fn status(own_id: u32) -> link::DccStatus {
    critical_section::with(|cs| {
        let mut slot = DCC.borrow(cs).borrow_mut();
        let dcc = slot.as_mut().unwrap();
        let power_ready = POWER.borrow(cs).borrow().as_ref().unwrap().is_low() && !UPDATING.load(Ordering::Relaxed);
        dcc.update_power(power_ready);
        link::DccStatus {
            network_id: own_id,
            kind: 2,
            permitted: dcc.permission.granted(),
            transmitting: dcc.enabled(),
        }
    })
}

pub fn inhibit_for_update() {
    UPDATING.store(true, Ordering::Relaxed);
    critical_section::with(|cs| {
        if let Some(dcc) = DCC.borrow(cs).borrow_mut().as_mut() {
            dcc.update_power(false);
        }
    });
}

pub fn receive(id: u32, data: &[u8], own_id: u32) {
    critical_section::with(|cs| {
        if let Some(dcc) = DCC.borrow(cs).borrow_mut().as_mut() {
            dcc.permission.receive(id, data, own_id);
        }
    });
}

/// Local bench command. The same power interlock still gates the line driver.
pub fn grant_local() {
    critical_section::with(|cs| {
        if let Some(dcc) = DCC.borrow(cs).borrow_mut().as_mut() {
            let grant = link::DccGrant { target: 0 };
            let id = link::DccGrant::can_id(0).unwrap();
            dcc.permission.receive(id, &grant.data().unwrap(), 0);
        }
    });
}

pub fn throttle_status(index: usize, now_ms: u64, own_id: u32) -> Option<link::ThrottleStatus> {
    critical_section::with(|cs| {
        let slot = DCC.borrow(cs).borrow();
        let dcc = slot.as_ref()?;
        dcc.engine
            .table
            .status_due(index, now_ms, own_id, dcc.enabled())
    })
}

pub fn throttle_sent(index: usize, sequence: u32, now_ms: u64) {
    critical_section::with(|cs| {
        if let Some(dcc) = DCC.borrow(cs).borrow_mut().as_mut() {
            dcc.engine.table.status_sent(index, sequence, now_ms);
        }
    });
}

/// Preparing a throttle entry does not activate the line driver.
pub fn apply(command: link::ThrottleSet, own_id: u32) -> Apply {
    critical_section::with(|cs| {
        let mut slot = DCC.borrow(cs).borrow_mut();
        slot.as_mut().map_or(Apply::Rejected, |dcc| {
            dcc.engine.table.apply(command, own_id)
        })
    })
}
