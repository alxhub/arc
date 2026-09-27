//! Hardware-independent district policy. Time is monotonic milliseconds.

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    #[default]
    Disabled,
    Synced,
    Programming,
}

/// Short recovery uses full-amplitude, bounded on windows separated by off time.
/// This is burst duty limiting, not high-frequency PWM of the DCC waveform.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Recovery {
    pub probe_ms: u32,
    pub initial_off_ms: u32,
    pub max_off_ms: u32,
    pub clear_probes: u8,
    pub reset_after_ms: u32,
}

impl Default for Recovery {
    fn default() -> Self {
        Self {
            probe_ms: 2,
            initial_off_ms: 40,
            max_off_ms: 10_000,
            clear_probes: 10,
            reset_after_ms: 5_000,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    pub mode: Mode,
    pub recovery: Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidConfig;

impl Config {
    pub fn validate(self) -> Result<Self, InvalidConfig> {
        let r = self.recovery;
        // At least one settling tick and one measurement tick. Keep nominal
        // retry duty at or below 10%, including when changing the policy live.
        if r.probe_ms < 2
            || r.probe_ms > 20
            || r.initial_off_ms < r.probe_ms * 9
            || r.max_off_ms < r.initial_off_ms
            || r.clear_probes == 0
            || r.reset_after_ms < r.probe_ms
        {
            Err(InvalidConfig)
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    pub fault: bool,
    pub overcurrent: bool,
}

impl Sample {
    fn bad(self) -> bool {
        self.fault || self.overcurrent
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Drive {
    pub mode: Mode,
    pub enabled: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Disabled,
    WaitingForSource,
    Probing,
    CoolingDown,
    Running,
    /// Hardware errors latch off until an explicit Disabled configuration.
    HardwareFault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub config: Config,
    pub state: State,
    pub drive: Drive,
    pub next_off_ms: u32,
    pub clear_probes: u8,
}

/// Each task owns one controller. It has no CAN, STM32, or revision knowledge.
pub struct Controller {
    config: Config,
    state: State,
    since: u64,
    deadline: u64,
    next_off_ms: u32,
    clear_probes: u8,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    pub fn new() -> Self {
        let config = Config::default();
        Self {
            config,
            state: State::Disabled,
            since: 0,
            deadline: 0,
            next_off_ms: config.recovery.initial_off_ms,
            clear_probes: 0,
        }
    }

    /// Apply the whole configuration atomically. A change first turns output off.
    /// Repeated identical commands cannot restart a retry or erase its backoff.
    pub fn configure(&mut self, config: Config, now: u64) -> Result<(), InvalidConfig> {
        let config = config.validate()?;
        if config == self.config
            && !(config.mode == Mode::Disabled && self.state == State::HardwareFault)
        {
            return Ok(());
        }
        let latched = self.state == State::HardwareFault;
        self.config = config;
        self.clear_probes = 0;
        self.next_off_ms = self.next_off_ms.max(config.recovery.initial_off_ms);
        if config.mode == Mode::Disabled {
            self.state = State::Disabled;
            // Preserve short backoff across mode changes and disable/enable.
        } else if !latched {
            // Configuration changes cannot bypass an existing cooldown.
            self.deadline = self.deadline.max(now + u64::from(self.next_off_ms));
            self.state = State::CoolingDown;
        }
        Ok(())
    }

    pub fn hardware_fault(&mut self) {
        self.state = State::HardwareFault;
        self.clear_probes = 0;
    }

    /// Sample must describe the output commanded on the preceding tick. Never
    /// infer clearance from current or fault readings while the bridge is off.
    pub fn tick(&mut self, now: u64, source_ready: bool, sample: Sample) -> Drive {
        if self.state == State::HardwareFault {
            return self.drive();
        }
        if self.config.mode == Mode::Disabled {
            self.state = State::Disabled;
        } else if !source_ready {
            if self.state != State::WaitingForSource {
                self.deadline = self.deadline.max(now + u64::from(self.next_off_ms));
            }
            self.state = State::WaitingForSource;
            self.clear_probes = 0;
        } else {
            match self.state {
                State::WaitingForSource | State::CoolingDown if now >= self.deadline => {
                    self.state = State::Probing;
                    self.since = now;
                }
                State::Probing => {
                    // Allow the driver's wake-up and the IPROPI filter one tick.
                    let elapsed = now.saturating_sub(self.since);
                    if elapsed >= 1 && sample.bad() {
                        self.trip(now);
                    } else if elapsed >= u64::from(self.config.recovery.probe_ms) {
                        self.clear_probes += 1;
                        if self.clear_probes >= self.config.recovery.clear_probes {
                            self.state = State::Running;
                            self.since = now;
                        } else {
                            self.cooldown(now);
                        }
                    }
                }
                State::Running => {
                    if sample.bad() {
                        self.trip(now);
                    } else if now.saturating_sub(self.since)
                        >= u64::from(self.config.recovery.reset_after_ms)
                    {
                        self.next_off_ms = self.config.recovery.initial_off_ms;
                    }
                }
                _ => {}
            }
        }
        self.drive()
    }

    fn cooldown(&mut self, now: u64) {
        self.state = State::CoolingDown;
        self.deadline = now + u64::from(self.next_off_ms);
    }

    fn trip(&mut self, now: u64) {
        self.clear_probes = 0;
        self.cooldown(now);
        self.next_off_ms = self
            .next_off_ms
            .saturating_mul(2)
            .min(self.config.recovery.max_off_ms);
    }

    pub fn drive(&self) -> Drive {
        Drive {
            mode: self.config.mode,
            enabled: matches!(self.state, State::Running | State::Probing),
        }
    }

    pub fn status(&self) -> Status {
        Status {
            config: self.config,
            state: self.state,
            drive: self.drive(),
            next_off_ms: self.next_off_ms,
            clear_probes: self.clear_probes,
        }
    }
}

/// A task owns the district's gate. Implementations serialize access to shared
/// buses internally. `apply` must gate off before changing source/range, and
/// any I/O error must leave the local gate off even if a shared bus is broken.
#[allow(async_fn_in_trait)]
pub trait DistrictHal {
    type Error;
    async fn apply(&mut self, drive: Drive) -> Result<(), Self::Error>;
    async fn sample(&mut self) -> Result<Sample, Self::Error>;
    /// Infallible, immediate local inhibit, independent of any shared bus.
    fn inhibit(&mut self);
}
