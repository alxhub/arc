use super::{ADC, Sense, configure_can};
use core::convert::Infallible;
use dst_01::district::{DistrictHal, Drive, Mode, Sample};
use embassy_stm32::{
    Peripherals,
    adc::{Adc, AdcChannel},
    bind_interrupts,
    can::{self, Can, CanConfigurator},
    gpio::{Input, Level, Output, Pull, Speed},
    peripherals,
};
use embassy_sync::mutex::Mutex;

bind_interrupts!(struct CanIrqs {
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
});

pub struct District {
    gate: Output<'static>,
    sleep: Output<'static>,
    nprog: Output<'static>,
    // Fixed normal phase for now; a future reversing policy will own its value.
    _phase: Output<'static>,
    fault: Input<'static>,
    sense: Sense,
    applied: Option<Drive>,
}

impl DistrictHal for District {
    type Error = Infallible;

    async fn apply(&mut self, drive: Drive) -> Result<(), Self::Error> {
        if self.applied == Some(drive) {
            return Ok(());
        }
        self.gate.set_low();
        self.sleep.set_low();
        self.nprog.set_level(if drive.mode == Mode::Programming {
            Level::Low
        } else {
            Level::High
        });
        if drive.enabled {
            self.sleep.set_high();
            self.gate.set_high();
        }
        self.applied = Some(drive);
        Ok(())
    }

    async fn sample(&mut self) -> Result<Sample, Self::Error> {
        Ok(Sample {
            fault: self.fault.is_low(),
            overcurrent: self.sense.overcurrent().await,
        })
    }

    fn inhibit(&mut self) {
        self.gate.set_low();
        self.sleep.set_low();
        if self.applied.is_some_and(|drive| drive.enabled) {
            self.applied = None;
        }
    }
}

pub fn init(p: Peripherals) -> ([District; 4], Can<'static>) {
    // Acquire every gate and nSLEEP low before configuring the ADC.
    let gates = [
        Output::new(p.PA0, Level::Low, Speed::Low),
        Output::new(p.PA2, Level::Low, Speed::Low),
        Output::new(p.PB14, Level::Low, Speed::Low),
        Output::new(p.PA11, Level::Low, Speed::Low),
    ];
    let sleeps = [
        Output::new(p.PA6, Level::Low, Speed::Low),
        Output::new(p.PB1, Level::Low, Speed::Low),
        Output::new(p.PB13, Level::Low, Speed::Low),
        Output::new(p.PC6, Level::Low, Speed::Low),
    ];
    let programs = [
        Output::new(p.PC15, Level::Low, Speed::Low),
        Output::new(p.PC13, Level::Low, Speed::Low),
        Output::new(p.PC14, Level::Low, Speed::Low),
        Output::new(p.PF3, Level::Low, Speed::Low),
    ];
    let phases = [
        Output::new(p.PA15, Level::Low, Speed::Low),
        Output::new(p.PD2, Level::Low, Speed::Low),
        Output::new(p.PB5, Level::Low, Speed::Low),
        Output::new(p.PD3, Level::Low, Speed::Low),
    ];
    let faults = [
        Input::new(p.PA4, Pull::None),
        Input::new(p.PA7, Pull::None),
        Input::new(p.PB12, Pull::None),
        Input::new(p.PA10, Pull::None),
    ];
    let adc = ADC.init(Mutex::new(Adc::new(
        p.ADC1,
        embassy_stm32::adc::Resolution::BITS12,
    )));
    let pins = [
        p.PA1.degrade_adc(),
        p.PA3.degrade_adc(),
        p.PB11.degrade_adc(),
        p.PA8.degrade_adc(),
    ];
    let mut parts = gates
        .into_iter()
        .zip(sleeps)
        .zip(programs)
        .zip(phases)
        .zip(faults)
        .zip(pins);
    let districts = core::array::from_fn(|_| {
        let (((((gate, sleep), nprog), phase), fault), pin) = parts.next().unwrap();
        District {
            gate,
            sleep,
            nprog,
            _phase: phase,
            fault,
            sense: Sense { adc, pin },
            applied: None,
        }
    });
    let can = configure_can(CanConfigurator::new(p.FDCAN1, p.PD0, p.PD1, CanIrqs));
    (districts, can)
}
