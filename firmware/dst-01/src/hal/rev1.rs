use super::{ADC, Sense, configure_can};
use dst_01::district::{DistrictHal, Drive, Mode, Sample};
use embassy_stm32::{
    Peripherals,
    adc::{Adc, AdcChannel},
    bind_interrupts,
    can::{self, Can, CanConfigurator},
    gpio::{Level, Output, Speed},
    i2c::{self, I2c},
    mode::Blocking,
    peripherals,
    time::Hertz,
};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use embassy_time::Duration;
use static_cell::StaticCell;

struct Expanders {
    bus: I2c<'static, Blocking, i2c::Master>,
    shadow: [u8; 2],
    initialized: bool,
}

impl Expanders {
    fn fault(&mut self, index: usize) -> Result<bool, i2c::Error> {
        self.initialize()?;
        let mut input = [0];
        self.bus
            .blocking_write_read(0x20 + (index / 2) as u8, &[0], &mut input)?;
        Ok(input[0] & (1 << if index.is_multiple_of(2) { 2 } else { 5 }) == 0)
    }

    fn initialize(&mut self) -> Result<(), i2c::Error> {
        if !self.initialized {
            for address in [0x20, 0x21] {
                // Output latches first, then directions: no transient wake-up.
                self.bus.blocking_write(address, &[1, 0])?;
                self.bus.blocking_write(address, &[3, 0x24])?;
            }
            self.shadow = [0; 2];
            self.initialized = true;
        }
        Ok(())
    }
}

type SharedExpanders = Mutex<CriticalSectionRawMutex, Expanders>;
static EXPANDERS: StaticCell<SharedExpanders> = StaticCell::new();

bind_interrupts!(struct CanIrqs {
    TIM16_FDCAN_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    TIM17_FDCAN_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
});

pub struct District {
    gate: Output<'static>,
    sense: Sense,
    expanders: &'static SharedExpanders,
    index: usize,
    applied: Option<Drive>,
}

impl DistrictHal for District {
    type Error = i2c::Error;

    async fn apply(&mut self, drive: Drive) -> Result<(), Self::Error> {
        if self.applied == Some(drive) {
            return Ok(());
        }
        // The gate is direct MCU GPIO even on rev1. Never wait on I2C to inhibit.
        self.gate.set_low();
        self.applied = None;
        let mut shared = self.expanders.lock().await;
        shared.initialize()?;
        let which = self.index / 2;
        let (prog, sleep, mask) = if self.index.is_multiple_of(2) {
            (1, 3, 0x0b)
        } else {
            (6, 4, 0xd0)
        };
        // PROG is active high on rev1. Keep phase zero and preserve the peer's bits.
        let bits =
            ((drive.mode == Mode::Programming) as u8) << prog | (drive.enabled as u8) << sleep;
        let next = (shared.shadow[which] & !mask) | bits;
        shared.bus.blocking_write(0x20 + which as u8, &[1, next])?;
        shared.shadow[which] = next;
        if drive.enabled {
            self.gate.set_high();
        }
        self.applied = Some(drive);
        Ok(())
    }

    async fn sample(&mut self) -> Result<Sample, Self::Error> {
        let result = self.expanders.lock().await.fault(self.index);
        let fault = match result {
            Ok(fault) => fault,
            Err(error) => {
                self.inhibit();
                return Err(error);
            }
        };
        Ok(Sample {
            fault,
            overcurrent: self.sense.overcurrent().await,
        })
    }

    fn inhibit(&mut self) {
        self.gate.set_low();
        if self.applied.is_some_and(|drive| drive.enabled) {
            self.applied = None;
        }
    }
}

pub fn init(p: Peripherals) -> ([District; 4], Can<'static>) {
    let gates = [
        Output::new(p.PA6, Level::Low, Speed::Low),
        Output::new(p.PA7, Level::Low, Speed::Low),
        Output::new(p.PB0, Level::Low, Speed::Low),
        Output::new(p.PB1, Level::Low, Speed::Low),
    ];
    let mut config = i2c::Config::default();
    config.frequency = Hertz(400_000);
    config.timeout = Duration::from_millis(1);
    let expanders = EXPANDERS.init(Mutex::new(Expanders {
        bus: I2c::new_blocking(p.I2C3, p.PC0, p.PC1, config),
        shadow: [0; 2],
        initialized: false,
    }));
    let adc = ADC.init(Mutex::new(Adc::new(p.ADC1)));
    let pins = [
        p.PA0.degrade_adc(),
        p.PA1.degrade_adc(),
        p.PA2.degrade_adc(),
        p.PA3.degrade_adc(),
    ];
    let mut parts = gates.into_iter().zip(pins);
    let districts = core::array::from_fn(|index| {
        let (gate, pin) = parts.next().unwrap();
        District {
            gate,
            sense: Sense { adc, pin },
            expanders,
            index,
            applied: None,
        }
    });
    let can = configure_can(CanConfigurator::new(p.FDCAN1, p.PB8, p.PB9, CanIrqs));
    (districts, can)
}
