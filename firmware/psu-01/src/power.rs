//! Link power qualification. The hardware cannot measure VPWR before closing
//! its switch, so startup is a supervised trial followed by a downstream check.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    Starting,
    Online,
    Fault,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Reading {
    pub millivolts: u16,
    pub milliamps: i16,
}

pub struct Controller {
    state: State,
    started_ms: u64,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    pub const fn new() -> Self {
        Self {
            state: State::Off,
            started_ms: 0,
        }
    }

    /// Start only after the monitor has answered over I2C.
    pub fn start(&mut self, now_ms: u64) {
        if self.state == State::Off {
            self.state = State::Starting;
            self.started_ms = now_ms;
        }
    }

    pub fn fail(&mut self) {
        self.state = State::Fault;
    }

    /// A missing monitor reading is a fault, including during startup.
    pub fn sample(&mut self, now_ms: u64, reading: Option<Reading>) {
        if !matches!(self.state, State::Starting | State::Online) {
            return;
        }
        let Some(reading) = reading else {
            self.state = State::Fault;
            return;
        };
        if reading.milliamps < -100 || reading.milliamps > 2_000 {
            self.state = State::Fault;
        } else if self.state == State::Starting {
            if now_ms.saturating_sub(self.started_ms) >= 20 {
                self.state = if (12_000..=18_000).contains(&reading.millivolts) {
                    State::Online
                } else {
                    State::Fault
                };
            }
        } else if !(12_000..=18_000).contains(&reading.millivolts) {
            self.state = State::Fault;
        }
    }

    pub const fn state(&self) -> State {
        self.state
    }
    pub const fn link_enabled(&self) -> bool {
        matches!(self.state, State::Starting | State::Online)
    }
    pub const fn dcc_enabled(&self) -> bool {
        matches!(self.state, State::Online)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const GOOD: Reading = Reading {
        millivolts: 15_000,
        milliamps: 500,
    };

    #[test]
    fn safe_start_and_fault_latch() {
        let mut c = Controller::new();
        assert!(!c.link_enabled());
        c.start(0);
        assert!(c.link_enabled());
        assert!(!c.dcc_enabled());
        c.sample(19, Some(GOOD));
        assert_eq!(c.state(), State::Starting);
        c.sample(20, Some(GOOD));
        assert_eq!(c.state(), State::Online);
        assert!(c.dcc_enabled());
        c.sample(
            30,
            Some(Reading {
                milliamps: 2_100,
                ..GOOD
            }),
        );
        assert_eq!(c.state(), State::Fault);
        assert!(!c.link_enabled());
        c.start(40);
        assert_eq!(c.state(), State::Fault);
    }

    #[test]
    fn failed_readback_never_becomes_online() {
        let mut c = Controller::new();
        c.start(0);
        c.sample(20, None);
        assert_eq!(c.state(), State::Fault);
    }
}
