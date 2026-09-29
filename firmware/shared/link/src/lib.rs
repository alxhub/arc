//! Shared ARC-Link backbone CAN protocol.
//!
//! ARC-Link CAN identities and Presence frames from the repository's `PROTOCOL.md`.

#![no_std]

pub mod update;

pub const DISCOVERY_CLASS: u32 = 0x1f;
pub const THROTTLE_CONTROL_CLASS: u32 = 0x08;
pub const DISTRICT_CONTROL_CLASS: u32 = 0x10;
pub const DISTRICT_STATUS_CLASS: u32 = 0x11;
pub const PSU_STATUS_CLASS: u32 = 0x12;
pub const THROTTLE_STATUS_CLASS: u32 = 0x13;
pub const PRESENCE_TYPE: u8 = 0x01;
pub const DISTRICT_CONFIG_TYPE: u8 = 0x01;
pub const DISTRICT_STATUS_TYPE: u8 = 0x01;
pub const PSU_STATUS_TYPE: u8 = 0x01;
pub const THROTTLE_SET_TYPE: u8 = 0x01;
pub const THROTTLE_STATUS_TYPE: u8 = 0x01;
pub const MAX_NETWORK_ID: u32 = 0x00ff_ffff;

pub const DCC_CONTROL_CLASS: u32 = 0x14;
pub const DCC_STATUS_CLASS: u32 = 0x15;

/// One-way grant, latched by the addressed source until its MCU restarts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DccGrant {
    pub target: u32,
}
impl DccGrant {
    pub fn can_id(sender: u32) -> Option<u32> {
        (sender <= MAX_NETWORK_ID).then_some((DCC_CONTROL_CLASS << 24) | sender)
    }
    pub fn data(&self) -> Option<[u8; 5]> {
        if self.target > MAX_NETWORK_ID {
            return None;
        }
        let b = self.target.to_be_bytes();
        Some([1, 1, b[1], b[2], b[3]])
    }
    pub fn decode(id: u32, data: &[u8]) -> Option<Self> {
        if id >> 24 != DCC_CONTROL_CLASS || data.len() != 5 || data[..2] != [1, 1] {
            return None;
        }
        Some(Self {
            target: u32::from_be_bytes([0, data[2], data[3], data[4]]),
        })
    }
}

/// Kind: 0 = no transmitter, 1 = PSU, 2 = district transmitter.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DccStatus {
    pub network_id: u32,
    pub kind: u8,
    pub permitted: bool,
    pub transmitting: bool,
}
impl DccStatus {
    pub fn can_id(&self) -> Option<u32> {
        (self.network_id <= MAX_NETWORK_ID).then_some((DCC_STATUS_CLASS << 24) | self.network_id)
    }
    pub fn data(&self) -> Option<[u8; 4]> {
        if self.network_id > MAX_NETWORK_ID
            || self.kind > 2
            || (self.kind == 0 && self.permitted)
            || (self.transmitting && !self.permitted)
        {
            return None;
        }
        Some([
            1,
            1,
            self.kind,
            u8::from(self.permitted) | (u8::from(self.transmitting) << 1),
        ])
    }
    pub fn decode(id: u32, data: &[u8]) -> Option<Self> {
        if id >> 24 != DCC_STATUS_CLASS
            || data.len() != 4
            || data[..2] != [1, 1]
            || data[3] & !3 != 0
        {
            return None;
        }
        let status = Self {
            network_id: id & MAX_NETWORK_ID,
            kind: data[2],
            permitted: data[3] & 1 != 0,
            transmitting: data[3] & 2 != 0,
        };
        status.data()?;
        Some(status)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThrottleSet {
    pub target: u32,
    pub session: u32,
    pub sequence: u32,
    pub address_kind: u8,
    pub address: u16,
    pub direction: u8,
    pub speed: u8,
    pub stop_mode: u8,
}

pub fn valid_loco_address(kind: u8, address: u16) -> bool {
    matches!((kind, address), (0, 1..=127) | (1, 1..=10239))
}

fn valid_throttle(kind: u8, address: u16, direction: u8, speed: u8, stop_mode: u8) -> bool {
    valid_loco_address(kind, address)
        && direction <= 1
        && speed <= 126
        && stop_mode <= 1
        && (stop_mode == 0 || speed == 0)
}

impl ThrottleSet {
    pub fn can_id(sender: u32) -> Option<u32> {
        (sender <= MAX_NETWORK_ID).then_some((THROTTLE_CONTROL_CLASS << 24) | sender)
    }

    pub fn data(&self) -> Option<[u8; 20]> {
        if self.target > MAX_NETWORK_ID
            || !valid_throttle(
                self.address_kind,
                self.address,
                self.direction,
                self.speed,
                self.stop_mode,
            )
        {
            return None;
        }
        let mut data = [0u8; 20];
        data[..2].copy_from_slice(&[1, THROTTLE_SET_TYPE]);
        data[2..5].copy_from_slice(&self.target.to_be_bytes()[1..]);
        data[5..9].copy_from_slice(&self.session.to_be_bytes());
        data[9..13].copy_from_slice(&self.sequence.to_be_bytes());
        data[13] = self.address_kind;
        data[14..16].copy_from_slice(&self.address.to_be_bytes());
        data[16..19].copy_from_slice(&[self.direction, self.speed, self.stop_mode]);
        Some(data)
    }

    pub fn decode(can_id: u32, data: &[u8]) -> Option<Self> {
        if can_id >> 24 != THROTTLE_CONTROL_CLASS
            || data.len() != 20
            || data[..2] != [1, THROTTLE_SET_TYPE]
            || data[19] != 0
        {
            return None;
        }
        let result = Self {
            target: u32::from_be_bytes([0, data[2], data[3], data[4]]),
            session: u32::from_be_bytes(data[5..9].try_into().ok()?),
            sequence: u32::from_be_bytes(data[9..13].try_into().ok()?),
            address_kind: data[13],
            address: u16::from_be_bytes([data[14], data[15]]),
            direction: data[16],
            speed: data[17],
            stop_mode: data[18],
        };
        valid_throttle(
            result.address_kind,
            result.address,
            result.direction,
            result.speed,
            result.stop_mode,
        )
        .then_some(result)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ThrottleStatus {
    pub network_id: u32,
    pub session: u32,
    pub sequence: u32,
    pub address_kind: u8,
    pub address: u16,
    pub direction: u8,
    pub speed: u8,
    pub stop_mode: u8,
    pub state: u8,
}

impl ThrottleStatus {
    pub fn can_id(&self) -> Option<u32> {
        (self.network_id <= MAX_NETWORK_ID)
            .then_some((THROTTLE_STATUS_CLASS << 24) | self.network_id)
    }

    pub fn data(&self) -> Option<[u8; 20]> {
        if !valid_throttle(
            self.address_kind,
            self.address,
            self.direction,
            self.speed,
            self.stop_mode,
        ) || self.state > 1
            || self.network_id > MAX_NETWORK_ID
        {
            return None;
        }
        let mut data = [0u8; 20];
        data[..2].copy_from_slice(&[1, THROTTLE_STATUS_TYPE]);
        data[2..6].copy_from_slice(&self.session.to_be_bytes());
        data[6..10].copy_from_slice(&self.sequence.to_be_bytes());
        data[10] = self.address_kind;
        data[11..13].copy_from_slice(&self.address.to_be_bytes());
        data[13..17].copy_from_slice(&[self.direction, self.speed, self.stop_mode, self.state]);
        Some(data)
    }

    pub fn decode(can_id: u32, data: &[u8]) -> Option<Self> {
        if can_id >> 24 != THROTTLE_STATUS_CLASS
            || data.len() != 20
            || data[..2] != [1, THROTTLE_STATUS_TYPE]
            || data[17..20] != [0, 0, 0]
        {
            return None;
        }
        let result = Self {
            network_id: can_id & MAX_NETWORK_ID,
            session: u32::from_be_bytes(data[2..6].try_into().ok()?),
            sequence: u32::from_be_bytes(data[6..10].try_into().ok()?),
            address_kind: data[10],
            address: u16::from_be_bytes([data[11], data[12]]),
            direction: data[13],
            speed: data[14],
            stop_mode: data[15],
            state: data[16],
        };
        (valid_throttle(
            result.address_kind,
            result.address,
            result.direction,
            result.speed,
            result.stop_mode,
        ) && result.state <= 1)
            .then_some(result)
    }
}

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

/// Commanded state for one district on the target board.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistrictConfig {
    pub target: u32,
    pub district: u8,
    pub mode: u8,
}

impl DistrictConfig {
    /// The CAN ID identifies the sender, not the target.
    pub fn can_id(sender: u32) -> Option<u32> {
        (sender <= MAX_NETWORK_ID).then_some((DISTRICT_CONTROL_CLASS << 24) | sender)
    }

    /// Exactly 7 bytes, corresponding to CAN FD DLC 7.
    pub fn data(&self) -> Option<[u8; 7]> {
        if self.target > MAX_NETWORK_ID || self.district > 3 || self.mode > 2 {
            return None;
        }
        let mut data = [0; 7];
        data[0..2].copy_from_slice(&[1, DISTRICT_CONFIG_TYPE]);
        data[2..5].copy_from_slice(&self.target.to_be_bytes()[1..]);
        data[5] = self.district;
        data[6] = self.mode;
        Some(data)
    }

    pub fn decode(can_id: u32, data: &[u8]) -> Option<Self> {
        if can_id >> 24 != DISTRICT_CONTROL_CLASS
            || data.len() != 7
            || data[0..2] != [1, DISTRICT_CONFIG_TYPE]
            || data[5] > 3
            || data[6] > 2
        {
            return None;
        }
        Some(Self {
            target: u32::from_be_bytes([0, data[2], data[3], data[4]]),
            district: data[5],
            mode: data[6],
        })
    }
}

/// Periodic observation of one district. State values follow `PROTOCOL.md`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DistrictStatus {
    pub network_id: u32,
    pub district: u8,
    pub mode: u8,
    pub state: u8,
    pub tripped: bool,
    pub current_valid: bool,
    pub average_ma: u16,
    pub peak_ma: u16,
    pub window_ms: u32,
}

impl DistrictStatus {
    pub fn can_id(&self) -> Option<u32> {
        (self.network_id <= MAX_NETWORK_ID)
            .then_some((DISTRICT_STATUS_CLASS << 24) | self.network_id)
    }

    /// Exactly 16 bytes, corresponding to CAN FD DLC 10.
    pub fn data(&self) -> Option<[u8; 16]> {
        if self.district > 3 || self.mode > 2 || self.state > 5 || self.window_ms == 0 {
            return None;
        }
        let mut data = [0; 16];
        data[0..2].copy_from_slice(&[1, DISTRICT_STATUS_TYPE]);
        data[2] = self.district;
        data[3] = self.mode;
        data[4] = self.state;
        data[5] = u8::from(self.tripped) | (u8::from(self.current_valid) << 1);
        data[6..8].copy_from_slice(&self.average_ma.to_be_bytes());
        data[8..10].copy_from_slice(&self.peak_ma.to_be_bytes());
        data[10..14].copy_from_slice(&self.window_ms.to_be_bytes());
        Some(data)
    }

    pub fn decode(can_id: u32, data: &[u8]) -> Option<Self> {
        if can_id >> 24 != DISTRICT_STATUS_CLASS
            || data.len() != 16
            || data[0..2] != [1, DISTRICT_STATUS_TYPE]
            || data[2] > 3
            || data[3] > 2
            || data[4] > 5
            || data[5] & !0x03 != 0
            || data[14..16] != [0, 0]
        {
            return None;
        }
        let window_ms = u32::from_be_bytes(data[10..14].try_into().ok()?);
        if window_ms == 0 {
            return None;
        }
        Some(Self {
            network_id: can_id & MAX_NETWORK_ID,
            district: data[2],
            mode: data[3],
            state: data[4],
            tripped: data[5] & 1 != 0,
            current_valid: data[5] & 2 != 0,
            average_ma: u16::from_be_bytes(data[6..8].try_into().ok()?),
            peak_ma: u16::from_be_bytes(data[8..10].try_into().ok()?),
            window_ms,
        })
    }
}

/// PSU link and idle-DCC observation. Current is invalid when the monitor is
/// unavailable; zero then carries no measurement meaning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PsuStatus {
    pub network_id: u32,
    pub state: u8,
    pub dcc_enabled: bool,
    pub monitor_valid: bool,
    pub link_mv: u16,
    pub link_ma: u16,
    pub uptime_ms: u32,
}

impl PsuStatus {
    pub fn can_id(&self) -> Option<u32> {
        (self.network_id <= MAX_NETWORK_ID).then_some((PSU_STATUS_CLASS << 24) | self.network_id)
    }

    /// Exactly 12 bytes, corresponding to CAN FD DLC 9.
    pub fn data(&self) -> Option<[u8; 12]> {
        if self.state > 3 || (!self.monitor_valid && (self.link_mv != 0 || self.link_ma != 0)) {
            return None;
        }
        let mut data = [0u8; 12];
        data[..2].copy_from_slice(&[1, PSU_STATUS_TYPE]);
        data[2] = self.state;
        data[3] = u8::from(self.dcc_enabled) | (u8::from(self.monitor_valid) << 1);
        data[4..6].copy_from_slice(&self.link_mv.to_be_bytes());
        data[6..8].copy_from_slice(&self.link_ma.to_be_bytes());
        data[8..12].copy_from_slice(&self.uptime_ms.to_be_bytes());
        Some(data)
    }

    pub fn decode(can_id: u32, data: &[u8]) -> Option<Self> {
        if can_id >> 24 != PSU_STATUS_CLASS
            || data.len() != 12
            || data[..2] != [1, PSU_STATUS_TYPE]
            || data[2] > 3
            || data[3] & !3 != 0
        {
            return None;
        }
        let monitor_valid = data[3] & 2 != 0;
        let link_mv = u16::from_be_bytes([data[4], data[5]]);
        let link_ma = u16::from_be_bytes([data[6], data[7]]);
        if !monitor_valid && (link_mv != 0 || link_ma != 0) {
            return None;
        }
        Some(Self {
            network_id: can_id & MAX_NETWORK_ID,
            state: data[2],
            dcc_enabled: data[3] & 1 != 0,
            monitor_valid,
            link_mv,
            link_ma,
            uptime_ms: u32::from_be_bytes(data[8..12].try_into().ok()?),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dcc_grant_and_status_reject_malformed_frames() {
        let grant = DccGrant { target: 0x123456 };
        let id = DccGrant::can_id(1).unwrap();
        assert_eq!(grant.data(), Some([1, 1, 0x12, 0x34, 0x56]));
        assert_eq!(DccGrant::decode(id, &grant.data().unwrap()), Some(grant));
        assert!(DccGrant::decode(id, &[1, 1, 0x12, 0x34]).is_none());
        assert!(DccGrant::decode(id, &[2, 1, 0x12, 0x34, 0x56]).is_none());
        assert!(DccGrant::decode(id, &[1, 2, 0x12, 0x34, 0x56]).is_none());
        assert!(DccGrant::decode(0x13000001, &grant.data().unwrap()).is_none());
        assert!(DccGrant { target: 0x1000000 }.data().is_none());
        let status = DccStatus {
            network_id: 0x123456,
            kind: 2,
            permitted: true,
            transmitting: false,
        };
        let id = status.can_id().unwrap();
        assert_eq!(DccStatus::decode(id, &status.data().unwrap()), Some(status));
        for bytes in [[1, 1, 3, 0], [1, 1, 0, 1], [1, 1, 1, 2], [1, 1, 1, 4]] {
            assert!(DccStatus::decode(id, &bytes).is_none());
        }
    }

    #[test]
    fn throttle_frames_round_trip_and_reject_invalid_fields() {
        let command = ThrottleSet {
            target: 0x123456,
            session: 7,
            sequence: 9,
            address_kind: 1,
            address: 1234,
            direction: 1,
            speed: 42,
            stop_mode: 0,
        };
        let id = ThrottleSet::can_id(0xabcdef).unwrap();
        let bytes = command.data().unwrap();
        assert_eq!(bytes.len(), 20);
        assert_eq!(ThrottleSet::decode(id, &bytes), Some(command));
        let mut bad = bytes;
        bad[19] = 1;
        assert_eq!(ThrottleSet::decode(id, &bad), None);
        bad = bytes;
        bad[18] = 1;
        assert_eq!(ThrottleSet::decode(id, &bad), None);
        let status = ThrottleStatus {
            network_id: 0x123456,
            session: 7,
            sequence: 9,
            address_kind: 1,
            address: 1234,
            direction: 1,
            speed: 42,
            stop_mode: 0,
            state: 1,
        };
        assert_eq!(
            ThrottleStatus::decode(status.can_id().unwrap(), &status.data().unwrap()),
            Some(status)
        );
    }

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

    #[test]
    fn district_config_round_trip_and_rejects_bad_envelope() {
        let config = DistrictConfig {
            target: 0xfa5138,
            district: 2,
            mode: 1,
        };
        let id = DistrictConfig::can_id(0x123456).unwrap();
        let data = config.data().unwrap();
        assert_eq!(id, 0x10123456);
        assert_eq!(data, [1, 1, 0xfa, 0x51, 0x38, 2, 1]);
        assert_eq!(DistrictConfig::decode(id, &data), Some(config));
        assert_eq!(DistrictConfig::decode(0x1f123456, &data), None);
        assert_eq!(DistrictConfig::decode(id, &data[..6]), None);
        let mut invalid = data;
        invalid[6] = 3;
        assert_eq!(DistrictConfig::decode(id, &invalid), None);
    }

    #[test]
    fn district_status_round_trip_and_reserved_bits() {
        let status = DistrictStatus {
            network_id: 0xfa5138,
            district: 2,
            mode: 1,
            state: 3,
            tripped: true,
            current_valid: true,
            average_ma: 150,
            peak_ma: 5_000,
            window_ms: 1001,
        };
        let id = status.can_id().unwrap();
        let data = status.data().unwrap();
        assert_eq!(id, 0x11fa5138);
        assert_eq!(
            data,
            [1, 1, 2, 1, 3, 3, 0, 150, 0x13, 0x88, 0, 0, 3, 0xe9, 0, 0]
        );
        assert_eq!(DistrictStatus::decode(id, &data), Some(status));
        let mut invalid = data;
        invalid[5] = 4;
        assert_eq!(DistrictStatus::decode(id, &invalid), None);
    }

    #[test]
    fn psu_status_round_trip() {
        let status = PsuStatus {
            network_id: 0x123456,
            state: 2,
            dcc_enabled: true,
            monitor_valid: true,
            link_mv: 15_000,
            link_ma: 500,
            uptime_ms: 20_000,
        };
        let data = status.data().unwrap();
        assert_eq!(
            PsuStatus::decode(status.can_id().unwrap(), &data),
            Some(status)
        );
    }
}
