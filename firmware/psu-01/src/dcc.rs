//! NMRA DCC idle and 128-step speed packets shared by board and host.

pub const ONE_HALF_US: u16 = 58;
pub const ZERO_HALF_US: u16 = 100;
pub const MAX_BYTES: usize = 5;
pub const MAX_BITS: usize = 15 + 9 * MAX_BYTES;

/// Validate packet framing and checksum without interpreting the instruction.
pub fn valid_packet_bits(bits: &[u8]) -> bool {
    if !matches!(bits.len(), 42 | 51 | 60)
        || bits[..14].iter().any(|bit| *bit != b'1')
        || bits[bits.len() - 1] != b'1'
    {
        return false;
    }
    let mut checksum = 0u8;
    for byte_start in (14..bits.len() - 1).step_by(9) {
        if bits[byte_start] != b'0' {
            return false;
        }
        let mut byte = 0u8;
        for bit in &bits[byte_start + 1..byte_start + 9] {
            if *bit != b'0' && *bit != b'1' {
                return false;
            }
            byte = (byte << 1) | u8::from(*bit == b'1');
        }
        checksum ^= byte;
    }
    checksum == 0
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Packet {
    bytes: [u8; MAX_BYTES],
    len: u8,
}

impl Packet {
    pub const fn idle() -> Self {
        Self {
            bytes: [0xff, 0x00, 0xff, 0, 0],
            len: 3,
        }
    }

    /// 128-speed-step DCC instruction. Zero is stop; 1..126 become DCC codes 2..127.
    pub fn speed(
        address_kind: u8,
        address: u16,
        direction: u8,
        speed: u8,
        emergency: bool,
    ) -> Option<Self> {
        if !link::valid_loco_address(address_kind, address)
            || direction > 1
            || speed > 126
            || (emergency && speed != 0)
        {
            return None;
        }
        let mut bytes = [0u8; MAX_BYTES];
        let mut len = if address_kind == 0 {
            bytes[0] = address as u8;
            1
        } else {
            bytes[0] = 0xc0 | ((address >> 8) as u8 & 0x3f);
            bytes[1] = address as u8;
            2
        };
        bytes[len] = 0x3f;
        len += 1;
        bytes[len] = (direction << 7)
            | if emergency {
                1
            } else if speed == 0 {
                0
            } else {
                speed + 1
            };
        len += 1;
        bytes[len] = bytes[..len].iter().fold(0, |sum, byte| sum ^ byte);
        len += 1;
        Some(Self {
            bytes,
            len: len as u8,
        })
    }

    pub const fn len_bits(&self) -> usize {
        15 + 9 * self.len as usize
    }

    pub fn bit(&self, index: usize) -> bool {
        assert!(index < self.len_bits());
        if index < 14 {
            return true;
        }
        let index = index - 14;
        if index == 9 * self.len as usize {
            return true;
        }
        let byte = index / 9;
        let offset = index % 9;
        offset != 0 && (self.bytes[byte] & (0x80 >> (offset - 1))) != 0
    }

    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..self.len as usize]
    }

    pub fn duration_us(&self) -> u64 {
        (0..self.len_bits())
            .map(|i| {
                u64::from(if self.bit(i) {
                    ONE_HALF_US
                } else {
                    ZERO_HALF_US
                }) * 2
            })
            .sum()
    }
}

pub const fn bit_ticks(bit: bool, timer_hz: u32) -> u16 {
    let half_us = if bit { ONE_HALF_US } else { ZERO_HALF_US };
    ((timer_hz / 1_000_000) * (half_us as u32) * 2) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_packet_preserves_existing_bits_and_timing() {
        let packet = Packet::idle();
        assert_eq!(packet.bytes(), &[0xff, 0x00, 0xff]);
        assert_eq!(packet.len_bits(), 42);
        assert!((0..14).all(|i| packet.bit(i)));
        assert!(!packet.bit(14));
        assert!(packet.bit(41));
        assert_eq!(bit_ticks(true, 40_000_000), 4_640);
        assert_eq!(bit_ticks(false, 40_000_000), 8_000);
    }

    #[test]
    fn speed_packet_encodes_short_long_stop_and_emergency() {
        let short = Packet::speed(0, 3, 1, 20, false).unwrap();
        assert_eq!(short.bytes(), &[3, 0x3f, 0x95, 0xa9]);
        assert_eq!(short.len_bits(), 51);
        let long = Packet::speed(1, 1234, 0, 0, false).unwrap();
        assert_eq!(long.bytes(), &[0xc4, 0xd2, 0x3f, 0, 0x29]);
        assert_eq!(long.len_bits(), 60);
        assert_eq!(
            Packet::speed(0, 3, 1, 0, true).unwrap().bytes(),
            &[3, 0x3f, 0x81, 0xbd]
        );
        let mut bits = [0u8; MAX_BITS];
        for (index, bit) in bits[..short.len_bits()].iter_mut().enumerate() {
            *bit = if short.bit(index) { b'1' } else { b'0' };
        }
        assert!(valid_packet_bits(&bits[..short.len_bits()]));
    }
}
