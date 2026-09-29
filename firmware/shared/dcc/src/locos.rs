//! Bounded throttle table and round-robin DCC packet selection.

use crate::Packet;
use link::{ThrottleSet, ThrottleStatus};

pub const CAPACITY: usize = 16;

#[derive(Clone, Copy)]
struct Entry {
    command: ThrottleSet,
    dirty: bool,
    last_report_ms: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Apply {
    Changed,
    Repeated,
    Rejected,
    Full,
}

pub struct Table {
    entries: [Option<Entry>; CAPACITY],
    cursor: usize,
}

impl Table {
    pub const fn new() -> Self {
        Self {
            entries: [None; CAPACITY],
            cursor: 0,
        }
    }

    pub fn apply(&mut self, command: ThrottleSet, own_id: u32) -> Apply {
        if command.data().is_none() || command.target != own_id {
            return Apply::Rejected;
        }
        if let Some(entry) = self.entries.iter_mut().flatten().find(|entry| {
            entry.command.address_kind == command.address_kind
                && entry.command.address == command.address
        }) {
            // Sessions are opaque until the authority exchange is designed.
            // Sequence ordering applies within one session only.
            if command.session == entry.command.session && command.sequence < entry.command.sequence
            {
                return Apply::Rejected;
            }
            if command.session == entry.command.session
                && command.sequence == entry.command.sequence
            {
                return if command == entry.command {
                    entry.dirty = true;
                    Apply::Repeated
                } else {
                    Apply::Rejected
                };
            }
            entry.command = command;
            entry.dirty = true;
            return Apply::Changed;
        }
        if let Some(slot) = self.entries.iter_mut().find(|entry| entry.is_none()) {
            *slot = Some(Entry {
                command,
                dirty: true,
                last_report_ms: 0,
            });
            Apply::Changed
        } else {
            Apply::Full
        }
    }

    pub fn next_packet(&mut self) -> Packet {
        for index in self.cursor..CAPACITY {
            if let Some(entry) = self.entries[index] {
                self.cursor = index + 1;
                let c = entry.command;
                return Packet::speed(
                    c.address_kind,
                    c.address,
                    c.direction,
                    c.speed,
                    c.stop_mode == 1,
                )
                .expect("validated throttle command");
            }
        }
        self.cursor = 0;
        Packet::idle()
    }

    pub fn status_due(
        &self,
        index: usize,
        now_ms: u64,
        network_id: u32,
        eligible: bool,
    ) -> Option<ThrottleStatus> {
        let entry = self.entries.get(index)?.as_ref()?;
        if !entry.dirty && now_ms < entry.last_report_ms.saturating_add(1_000) {
            return None;
        }
        let c = entry.command;
        Some(ThrottleStatus {
            network_id,
            session: c.session,
            sequence: c.sequence,
            address_kind: c.address_kind,
            address: c.address,
            direction: c.direction,
            speed: c.speed,
            stop_mode: c.stop_mode,
            state: u8::from(eligible),
        })
    }

    pub fn status_sent(&mut self, index: usize, sequence: u32, now_ms: u64) {
        if let Some(Some(entry)) = self.entries.get_mut(index)
            && entry.command.sequence == sequence
        {
            entry.dirty = false;
            entry.last_report_ms = now_ms;
        }
    }

    pub fn len(&self) -> usize {
        self.entries.iter().flatten().count()
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(address: u16, sequence: u32, speed: u8) -> ThrottleSet {
        ThrottleSet {
            target: 0x123456,
            session: 7,
            sequence,
            address_kind: 0,
            address,
            direction: 1,
            speed,
            stop_mode: 0,
        }
    }

    #[test]
    fn accepts_commands_without_authority_and_orders_within_a_session() {
        let mut table = Table::new();
        assert_eq!(table.apply(command(3, 1, 20), 0xffffff), Apply::Rejected);
        assert_eq!(table.apply(command(3, 1, 20), 0x123456), Apply::Changed);
        assert_eq!(table.apply(command(3, 1, 20), 0x123456), Apply::Repeated);
        assert_eq!(table.apply(command(3, 1, 21), 0x123456), Apply::Rejected);
        assert_eq!(table.apply(command(3, 0, 20), 0x123456), Apply::Rejected);
        assert_eq!(table.apply(command(3, 2, 21), 0x123456), Apply::Changed);
        assert_eq!(table.status_due(0, 1, 0x123456, true).unwrap().speed, 21);
        table.status_sent(0, 2, 1);
        assert!(table.status_due(0, 500, 0x123456, true).is_none());
        assert!(table.status_due(0, 1_001, 0x123456, true).is_some());
        let mut next_session = command(3, 1, 0);
        next_session.session = 8;
        assert_eq!(table.apply(next_session, 0x123456), Apply::Changed);
        assert_eq!(
            table.status_due(0, 1_002, 0x123456, true).unwrap().session,
            8
        );
    }

    #[test]
    fn cycles_all_locomotives_with_idle_between_rounds() {
        let mut table = Table::new();
        assert_eq!(table.next_packet(), Packet::idle());
        table.apply(command(3, 1, 20), 0x123456);
        table.apply(command(4, 1, 10), 0x123456);
        assert_eq!(table.next_packet().bytes()[0], 3);
        assert_eq!(table.next_packet().bytes()[0], 4);
        assert_eq!(table.next_packet(), Packet::idle());
        assert_eq!(table.next_packet().bytes()[0], 3);
    }
}
