//! Shared ARC-Link backbone CAN protocol.
//!
//! ARC-Link CAN identities and Presence frames from the repository's `PROTOCOL.md`.

#![no_std]

pub const DISCOVERY_CLASS: u32 = 0x1f;
pub const PRESENCE_TYPE: u8 = 0x01;
pub const MAX_NETWORK_ID: u32 = 0x00ff_ffff;

/// Low 24 bits of FNV-1a over `stm32` and UID bytes in register-address order.
pub fn stm32_network_id(uid: &[u8; 12]) -> u32 {
    let mut hash = 0x811c_9dc5u32;
    for byte in b"stm32".iter().chain(uid.iter()) {
        hash = (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193);
    }
    hash & MAX_NETWORK_ID
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presence {
    pub network_id: u32,
    pub uid: [u8; 12],
}

impl Presence {
    pub fn new(uid: [u8; 12]) -> Self {
        Self {
            network_id: stm32_network_id(&uid),
            uid,
        }
    }

    /// Extended 29-bit CAN identifier for discovery class `0x1f`.
    pub fn can_id(&self) -> u32 {
        (DISCOVERY_CLASS << 24) | self.network_id
    }

    /// Exactly 16 data bytes, corresponding to CAN FD DLC 10.
    pub fn data(&self) -> [u8; 16] {
        let mut data = [1, PRESENCE_TYPE, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        data[4..].copy_from_slice(&self.uid);
        data
    }

    pub fn decode(can_id: u32, data: &[u8]) -> Option<Self> {
        if can_id >> 24 != DISCOVERY_CLASS
            || data.len() != 16
            || data[..4] != [1, PRESENCE_TYPE, 1, 0]
        {
            return None;
        }
        let mut uid = [0; 12];
        uid.copy_from_slice(&data[4..]);
        Some(Self {
            network_id: can_id & MAX_NETWORK_ID,
            uid,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_matches_protocol_vector() {
        let uid = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
        let presence = Presence::new(uid);
        assert_eq!(presence.network_id, 0xfa5138);
        assert_eq!(presence.can_id(), 0x1ffa5138);
        assert_eq!(
            presence.data(),
            [1, 1, 1, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11]
        );
        assert_eq!(
            Presence::decode(presence.can_id(), &presence.data()),
            Some(presence)
        );
    }
}
