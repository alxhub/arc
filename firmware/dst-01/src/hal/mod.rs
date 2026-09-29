//! Board wiring belongs here, never in the district task or recovery policy.

use embassy_stm32::adc::{Adc, AnyAdcChannel, SampleTime};
use embassy_stm32::can::{Can, CanConfigurator, config::FrameTransmissionConfig};
use embassy_stm32::peripherals::ADC1;
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, mutex::Mutex};
use static_cell::StaticCell;
#[cfg(feature = "rev1")]
use core::sync::atomic::{AtomicU16, Ordering};

#[cfg(feature = "rev1")]
static ADC_COUNTS: [AtomicU16; 4] = [const { AtomicU16::new(0) }; 4];

#[cfg(feature = "rev1")]
pub fn adc_counts() -> [u16; 4] {
    core::array::from_fn(|index| ADC_COUNTS[index].load(Ordering::Relaxed))
}

#[cfg(feature = "rev1")]
mod rev1;
#[cfg(feature = "rev1")]
pub use rev1::{District, RgbLed, init};
#[cfg(feature = "rev2")]
mod rev2;
#[cfg(feature = "rev2")]
pub use rev2::{District, init};

type SharedAdc = Mutex<CriticalSectionRawMutex, Adc<'static, ADC1>>;
static ADC: StaticCell<SharedAdc> = StaticCell::new();

fn configure_can(mut can: CanConfigurator<'static>) -> Can<'static> {
    can.set_bitrate(500_000);
    let config = can
        .config()
        .set_frame_transmit(FrameTransmissionConfig::AllowFdCan);
    can.set_config(config);
    can.into_normal_mode()
}

pub struct Sense {
    adc: &'static SharedAdc,
    pin: AnyAdcChannel<'static, ADC1>,
    #[cfg(feature = "rev1")]
    index: usize,
}

impl Sense {
    async fn overcurrent(&mut self) -> bool {
        #[cfg(feature = "rev1")]
        let sample_time = SampleTime::CYCLES160_5;
        #[cfg(feature = "rev2")]
        let sample_time = SampleTime::CYCLES247_5;
        let raw = self
            .adc
            .lock()
            .await
            .blocking_read(&mut self.pin, sample_time);
        #[cfg(feature = "rev1")]
        ADC_COUNTS[self.index].store(raw, Ordering::Relaxed);
        // 90% of the hardware trip-reference divider, relative to the same
        // 3V3 ADC reference. Both current ranges trip at this voltage.
        // Rev1: 3.16k/30k; rev2: 9.76k/30k. Bench calibration is still needed.
        #[cfg(feature = "rev1")]
        const TRIP: u16 = 3334;
        #[cfg(feature = "rev2")]
        const TRIP: u16 = 2780;
        raw >= TRIP
    }
}
