//! Channel-one PWM transmitter for STM32 general-purpose timers, including TIM14.
//! The board owns the interrupt binding and configures the output pin's AF.

use crate::{Engine, bit_ticks};
use embassy_stm32::{
    Peri,
    gpio::{Flex, Output},
    timer::{GeneralInstance1Channel, low_level::Timer},
};

pub struct Transmitter<T: GeneralInstance1Channel> {
    timer: Timer<'static, T>,
    _pin: Flex<'static>,
    enable: Output<'static>,
    timer_hz: u32,
    enabled: bool,
    pub engine: Engine,
    pub permission: crate::Permission,
}

impl<T: GeneralInstance1Channel> Transmitter<T> {
    /// Starts disabled. The caller must bind and enable T's update interrupt
    /// before enabling transmission, and serialize access with that interrupt.
    pub fn new(tim: Peri<'static, T>, pin: Flex<'static>, mut enable: Output<'static>) -> Self {
        enable.set_low();
        let timer = Timer::new(tim);
        timer.stop();
        let timer_hz = timer.get_clock_frequency().0;
        // Both half-bit durations must be exact and fit a 16-bit full period.
        assert!(timer_hz.is_multiple_of(1_000_000));
        assert!((1_000_000..=327_000_000).contains(&timer_hz));
        let regs = timer.regs_1ch();
        regs.psc().write_value(0);
        regs.cr1().modify(|w| w.set_arpe(true));
        regs.ccmr_output(0).modify(|w| {
            // STM32 OC1M = 0b110: PWM mode 1 (also supported by TIM14).
            w.set_ocm(0, 6u8.into());
            w.set_ocpe(0, true);
        });
        regs.ccer().modify(|w| w.set_cce(0, true));
        Self {
            timer,
            _pin: pin,
            enable,
            timer_hz,
            enabled: false,
            engine: Engine::new(),
            permission: crate::Permission::new(),
        }
    }

    fn preload_next(&mut self) {
        let ticks = bit_ticks(self.engine.next_bit(), self.timer_hz);
        let regs = self.timer.regs_1ch();
        regs.arr().write(|w| w.set_arr(ticks - 1));
        regs.ccr(0).write(|w| w.set_ccr(ticks / 2));
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// Enabling always starts a fresh packet; repeated requests are idempotent.
    /// The CAN grant is latched separately; power qualification belongs to the caller.
    pub fn update_power(&mut self, power_ready: bool) {
        let enabled = self.permission.transmitting(power_ready);
        if self.enabled == enabled {
            return;
        }
        self.enable.set_low();
        self.timer.stop();
        self.timer.enable_update_interrupt(false);
        self.enabled = enabled;
        if enabled {
            self.engine.restart();
            self.preload_next();
            self.timer.generate_update_event();
            self.timer.reset();
            // ARR and CCR are buffered: stage bit 1 before starting bit 0.
            self.preload_next();
            self.timer.regs_1ch().sr().modify(|w| w.set_uif(false));
            self.timer.enable_update_interrupt(true);
            self.enable.set_high();
            self.timer.start();
        }
    }

    /// Call from the board's timer update handler while holding its mutex.
    pub fn on_update(&mut self) {
        let regs = self.timer.regs_1ch();
        if !regs.sr().read().uif() {
            return;
        }
        regs.sr().modify(|w| w.set_uif(false));
        if self.enabled {
            self.preload_next();
        }
    }
}
