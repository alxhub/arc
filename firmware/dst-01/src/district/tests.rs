use super::{Config, Controller, Mode, Recovery, Sample, State};

const GOOD: Sample = Sample {
    fault: false,
    overcurrent: false,
};
const SHORT: Sample = Sample {
    fault: false,
    overcurrent: true,
};

fn configured(clear_probes: u8) -> Controller {
    let mut c = Controller::new();
    c.configure(
        Config {
            mode: Mode::Synced,
            recovery: Recovery {
                clear_probes,
                ..Recovery::default()
            },
        },
        0,
    )
    .unwrap();
    c
}

#[test]
fn starts_disabled_and_requires_a_ready_source() {
    let mut c = Controller::new();
    assert!(!c.tick(0, true, GOOD).enabled);
    c.configure(
        Config {
            mode: Mode::Synced,
            ..Config::default()
        },
        1,
    )
    .unwrap();
    assert!(!c.tick(100, false, GOOD).enabled);
    assert_eq!(c.status().state, State::WaitingForSource);
    assert!(!c.tick(139, true, GOOD).enabled);
    assert!(c.tick(140, true, GOOD).enabled);
    assert_eq!(c.status().state, State::Probing);
}

#[test]
fn off_samples_do_not_count_as_cleared_and_probes_are_bounded() {
    let mut c = configured(2);
    for t in 0..40 {
        assert!(!c.tick(t, true, GOOD).enabled);
        assert_eq!(c.status().clear_probes, 0);
    }
    assert!(c.tick(40, true, SHORT).enabled); // preceding sample was powered off
    assert!(c.tick(41, true, GOOD).enabled);
    assert!(!c.tick(42, true, GOOD).enabled);
    assert_eq!(c.status().clear_probes, 1);
    assert!(!c.tick(81, true, GOOD).enabled);
    assert!(c.tick(82, true, GOOD).enabled);
    c.tick(83, true, GOOD);
    assert!(c.tick(84, true, GOOD).enabled);
    assert_eq!(c.status().state, State::Running);
}

#[test]
fn persistent_short_doubles_off_time_and_caps_it() {
    let mut c = configured(2);
    let mut now = 40;
    let mut off = 40;
    for _ in 0..14 {
        assert!(c.tick(now, true, GOOD).enabled);
        now += 1;
        assert!(!c.tick(now, true, SHORT).enabled);
        assert_eq!(c.status().state, State::CoolingDown);
        assert_eq!(c.status().clear_probes, 0);
        assert!(!c.tick(now + off - 1, true, GOOD).enabled);
        now += off;
        off = (off * 2).min(10_000);
        assert_eq!(u64::from(c.status().next_off_ms), off);
    }
}

#[test]
fn fault_pin_also_trips_and_stable_running_resets_backoff() {
    let mut c = configured(1);
    c.tick(40, true, GOOD);
    c.tick(41, true, GOOD);
    c.tick(42, true, GOOD);
    assert_eq!(c.status().state, State::Running);
    assert!(
        !c.tick(
            43,
            true,
            Sample {
                fault: true,
                overcurrent: false
            }
        )
        .enabled
    );
    assert_eq!(c.status().next_off_ms, 80);
    c.tick(83, true, GOOD);
    c.tick(84, true, GOOD);
    c.tick(85, true, GOOD);
    c.tick(5084, true, GOOD);
    assert_eq!(c.status().next_off_ms, 80);
    c.tick(5085, true, GOOD);
    assert_eq!(c.status().next_off_ms, 40);
}

#[test]
fn disable_mode_changes_and_repeated_commands_cannot_bypass_cooldown() {
    let mut c = configured(1);
    c.tick(40, true, GOOD);
    c.tick(41, true, SHORT);
    let synced = c.status().config;
    c.configure(synced, 42).unwrap();
    assert!(!c.tick(80, true, GOOD).enabled);
    c.configure(
        Config {
            mode: Mode::Disabled,
            ..synced
        },
        80,
    )
    .unwrap();
    assert!(!c.drive().enabled);
    c.configure(
        Config {
            mode: Mode::Programming,
            ..synced
        },
        81,
    )
    .unwrap();
    assert!(!c.tick(160, true, GOOD).enabled);
    let drive = c.tick(161, true, GOOD);
    assert!(drive.enabled);
    assert_eq!(drive.mode, Mode::Programming);
}

#[test]
fn source_loss_cuts_power_and_requires_new_probes() {
    let mut c = configured(1);
    c.tick(40, true, GOOD);
    c.tick(42, true, GOOD);
    assert!(!c.tick(43, false, GOOD).enabled);
    assert!(!c.tick(44, true, GOOD).enabled);
    assert!(c.tick(83, true, GOOD).enabled);
    assert_eq!(c.status().state, State::Probing);
}

#[test]
fn invalid_config_is_rejected_atomically() {
    let mut c = configured(1);
    let before = c.status();
    for recovery in [
        Recovery {
            probe_ms: 0,
            ..Recovery::default()
        },
        Recovery {
            initial_off_ms: 1,
            ..Recovery::default()
        },
        Recovery {
            max_off_ms: 1,
            ..Recovery::default()
        },
        Recovery {
            clear_probes: 0,
            ..Recovery::default()
        },
        Recovery {
            reset_after_ms: 0,
            ..Recovery::default()
        },
    ] {
        assert!(
            c.configure(
                Config {
                    mode: Mode::Programming,
                    recovery
                },
                10
            )
            .is_err()
        );
        assert_eq!(c.status(), before);
    }
}

#[test]
fn hardware_fault_latches_until_explicit_disable() {
    let mut c = configured(1);
    c.tick(40, true, GOOD);
    c.hardware_fault();
    assert!(!c.tick(100, true, GOOD).enabled);
    c.configure(
        Config {
            mode: Mode::Programming,
            ..Config::default()
        },
        100,
    )
    .unwrap();
    assert_eq!(c.status().state, State::HardwareFault);
    c.configure(Config::default(), 101).unwrap();
    assert_eq!(c.status().state, State::Disabled);
    c.configure(
        Config {
            mode: Mode::Synced,
            ..Config::default()
        },
        102,
    )
    .unwrap();
    assert!(!c.tick(103, true, GOOD).enabled);
}

#[test]
fn one_districts_short_does_not_change_anothers_state() {
    let mut a = configured(1);
    let mut b = configured(1);
    for c in [&mut a, &mut b] {
        c.tick(40, true, GOOD);
        c.tick(42, true, GOOD);
    }
    assert!(!a.tick(43, true, SHORT).enabled);
    assert!(b.tick(43, true, GOOD).enabled);
    assert_eq!(b.status().next_off_ms, 40);
}

#[test]
fn a_short_after_clear_probes_restarts_clearance_count() {
    let mut c = configured(3);
    c.tick(40, true, GOOD);
    c.tick(42, true, GOOD);
    assert_eq!(c.status().clear_probes, 1);
    c.tick(82, true, GOOD);
    assert!(!c.tick(83, true, SHORT).enabled);
    assert_eq!(c.status().clear_probes, 0);
    assert_eq!(c.status().state, State::CoolingDown);
}

#[test]
fn persistent_short_never_returns_to_continuous_drive() {
    let mut c = configured(1);
    let mut powered_ms = 0;
    for now in 0..60_000 {
        // A short is measurable only while powered. Off measurements look good.
        let sample = if c.drive().enabled { SHORT } else { GOOD };
        powered_ms += u32::from(c.tick(now, true, sample).enabled);
        assert_ne!(c.status().state, State::Running);
    }
    assert!(powered_ms > 0);
    assert!(powered_ms < 60); // <0.1% over one minute after exponential backoff
    assert_eq!(c.status().next_off_ms, 10_000);
}
