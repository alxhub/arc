//! Board-independent ARC-Link handling for one DST-01x4 node.

use crate::district::{Config, Mode, State, Status};
use link::{DistrictConfig, DistrictStatus};

pub fn decode_command(can_id: u32, data: &[u8], own_id: u32) -> Option<(usize, Config)> {
    let command = DistrictConfig::decode(can_id, data)?;
    if command.target != own_id {
        return None;
    }
    let mode = match command.mode {
        0 => Mode::Disabled,
        1 => Mode::Synced,
        2 => Mode::Programming,
        _ => return None,
    };
    Some((
        usize::from(command.district),
        Config {
            mode,
            ..Config::default()
        },
    ))
}

fn mode_code(mode: Mode) -> u8 {
    match mode {
        Mode::Disabled => 0,
        Mode::Synced => 1,
        Mode::Programming => 2,
    }
}

fn state_code(state: State) -> u8 {
    match state {
        State::Disabled => 0,
        State::WaitingForSource => 1,
        State::Probing => 2,
        State::CoolingDown => 3,
        State::Running => 4,
        State::HardwareFault => 5,
    }
}

/// Produces a wire status from the controller's actual state. Current values
/// remain invalid until a calibrated current measurement is supplied.
pub fn status_frame(
    network_id: u32,
    district: usize,
    status: Status,
    current: Option<(u16, u16)>,
    window_ms: u32,
) -> DistrictStatus {
    let (average_ma, peak_ma, current_valid) = match current {
        Some((average, peak)) => (average, peak, true),
        None => (0, 0, false),
    };
    DistrictStatus {
        network_id,
        district: district as u8,
        mode: mode_code(status.config.mode),
        state: state_code(status.state),
        tripped: status.tripped,
        current_valid,
        average_ma,
        peak_ma,
        window_ms,
    }
}

#[derive(Clone, Copy)]
pub struct StatusCadence {
    last: Option<(Mode, State, bool)>,
    last_sent_ms: u64,
    next_periodic_ms: u64,
}

impl StatusCadence {
    pub const fn new() -> Self {
        Self {
            last: None,
            last_sent_ms: 0,
            next_periodic_ms: 1_000,
        }
    }

    pub fn due(&self, now_ms: u64, status: Status) -> bool {
        now_ms > self.last_sent_ms
            && (self.last != Some((status.config.mode, status.state, status.tripped))
                || now_ms >= self.next_periodic_ms)
    }

    pub fn window_ms(&self, now_ms: u64) -> u32 {
        now_ms
            .saturating_sub(self.last_sent_ms)
            .min(u64::from(u32::MAX)) as u32
    }

    pub fn sent(&mut self, now_ms: u64, status: Status) {
        self.last = Some((status.config.mode, status.state, status.tripped));
        self.last_sent_ms = now_ms;
        if now_ms >= self.next_periodic_ms {
            self.next_periodic_ms = now_ms.saturating_add(1_000);
        }
    }
}

impl Default for StatusCadence {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_targets_one_board_and_uses_local_recovery_policy() {
        let command = DistrictConfig {
            target: 0xfa5138,
            district: 2,
            mode: 1,
        };
        let id = DistrictConfig::can_id(1).unwrap();
        let data = command.data().unwrap();
        assert!(decode_command(id, &data, 2).is_none());
        let (index, config) = decode_command(id, &data, command.target).unwrap();
        assert_eq!(index, 2);
        assert_eq!(config.mode, Mode::Synced);
        assert_eq!(config.recovery, Config::default().recovery);
    }

    #[test]
    fn state_change_and_periodic_report_have_independent_deadlines() {
        let mut cadence = StatusCadence::new();
        let mut controller = crate::district::Controller::new();
        let disabled = controller.status();
        assert!(!cadence.due(0, disabled));
        assert!(cadence.due(1, disabled));
        cadence.sent(1, disabled);
        assert!(!cadence.due(2, disabled));
        controller
            .configure(
                Config {
                    mode: Mode::Synced,
                    ..Config::default()
                },
                200,
            )
            .unwrap();
        let changed = controller.status();
        assert!(cadence.due(200, changed));
        cadence.sent(200, changed);
        assert!(!cadence.due(999, changed));
        assert!(cadence.due(1_000, changed));
    }

    #[test]
    fn unavailable_hardware_current_is_explicit_in_status() {
        let status = crate::district::Controller::new().status();
        let frame = status_frame(0xfa5138, 0, status, None, 1_000);
        assert!(!frame.current_valid);
        assert_eq!((frame.average_ma, frame.peak_ma), (0, 0));
        assert_eq!(
            link::DistrictStatus::decode(frame.can_id().unwrap(), &frame.data().unwrap()),
            Some(frame)
        );
    }
}
