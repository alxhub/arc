//! One independently configured task per district; no revision-specific logic.

use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, watch::Watch};
use embassy_time::{Duration, Instant, Timer};

use crate::district::{Config, Controller, DistrictHal, InvalidConfig, Mode, Status};

#[derive(Clone, Copy, Default)]
pub struct Sources {
    pub synced: bool,
    pub programming: bool,
}

/// The DCC receiver/generator will publish readiness here. Neither source is
/// available at boot; selecting a mode alone cannot energize stationary rails.
pub static SOURCES: Watch<CriticalSectionRawMutex, Sources, 4> = Watch::new_with(Sources {
    synced: false,
    programming: false,
});

pub struct Control {
    config: Watch<CriticalSectionRawMutex, Config, 1>,
    status: Watch<CriticalSectionRawMutex, Status, 1>,
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

impl Control {
    pub const fn new() -> Self {
        Self {
            config: Watch::new(),
            status: Watch::new(),
        }
    }

    /// Latest configuration wins; the task publishes when it becomes active.
    pub fn configure(&self, config: Config) -> Result<(), InvalidConfig> {
        self.config.sender().send(config.validate()?);
        Ok(())
    }

    pub fn status(&self) -> Option<Status> {
        self.status.try_get()
    }
}

pub static DISTRICTS: [Control; 4] = [const { Control::new() }; 4];

pub async fn run<H: DistrictHal>(mut hal: H, control: &'static Control) -> ! {
    let mut controller = Controller::new();
    let mut configs = control.config.receiver().expect("one task per district");
    hal.inhibit();
    loop {
        let now = Instant::now().as_millis();
        if let Some(config) = configs.try_changed() {
            // Validation also occurs in configure(), so invalid policy never
            // becomes active. Apply a changed mode off before taking its sample.
            let before = controller.drive();
            let _ = controller.configure(config, now);
            if before != controller.drive() && hal.apply(controller.drive()).await.is_err() {
                hal.inhibit();
                controller.hardware_fault();
            }
        }
        let sources = SOURCES.try_get().unwrap_or_default();
        let ready = match controller.status().config.mode {
            Mode::Disabled => false,
            Mode::Synced => sources.synced,
            Mode::Programming => sources.programming,
        };
        // Remove drive immediately on source loss, before any shared-bus read.
        if !ready {
            hal.inhibit();
        }
        match hal.sample().await {
            Ok(sample) => {
                let drive = controller.tick(Instant::now().as_millis(), ready, sample);
                if hal.apply(drive).await.is_err() {
                    hal.inhibit();
                    controller.hardware_fault();
                }
            }
            Err(_) => {
                hal.inhibit();
                controller.hardware_fault();
            }
        }
        control.status.sender().send(controller.status());
        // No catch-up bursts after a slow bus transaction. Hardware current
        // limiting is still required; this loop is supervisory protection.
        Timer::after(Duration::from_millis(1)).await;
    }
}
