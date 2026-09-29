//! Continuous packets, with table changes taking effect at packet boundaries.

use crate::{Packet, locos::Table};

pub struct Engine {
    pub table: Table,
    packet: Packet,
    bit_index: usize,
}

impl Engine {
    pub const fn new() -> Self {
        Self {
            table: Table::new(),
            packet: Packet::idle(),
            bit_index: 0,
        }
    }

    /// Restart with a full idle preamble, retaining the throttle table.
    pub fn restart(&mut self) {
        self.packet = Packet::idle();
        self.bit_index = 0;
    }

    pub fn next_bit(&mut self) -> bool {
        if self.bit_index == self.packet.len_bits() {
            self.packet = self.table.next_packet();
            self.bit_index = 0;
        }
        let bit = self.packet.bit(self.bit_index);
        self.bit_index += 1;
        bit
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updates_do_not_tear_packets_and_restart_has_full_preamble() {
        let mut engine = Engine::new();
        let command = link::ThrottleSet {
            target: 1,
            session: 2,
            sequence: 1,
            address_kind: 0,
            address: 3,
            direction: 1,
            speed: 20,
            stop_mode: 0,
        };
        let idle = Packet::idle();
        for i in 0..idle.len_bits() {
            if i == 20 {
                engine.table.apply(command, 1);
            }
            assert_eq!(engine.next_bit(), idle.bit(i));
        }
        let speed = Packet::speed(0, 3, 1, 20, false).unwrap();
        for i in 0..speed.len_bits() {
            assert_eq!(engine.next_bit(), speed.bit(i));
        }
        for i in 0..idle.len_bits() {
            assert_eq!(engine.next_bit(), idle.bit(i));
        }
        engine.next_bit();
        engine.restart();
        for i in 0..idle.len_bits() {
            assert_eq!(engine.next_bit(), idle.bit(i));
        }
    }
}
