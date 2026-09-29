//! Stop-and-wait CAN FD firmware transport. Flash policy lives on each board.

pub const CONTROL_CLASS: u32 = 0x16;
pub const STATUS_CLASS: u32 = 0x17;
const MAX_ID: u32 = 0x00ff_ffff;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ImageKind {
    DstRev1 = 1,
    DstRev2 = 2,
    Psu = 3,
}

impl ImageKind {
    fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            1 => Some(Self::DstRev1),
            2 => Some(Self::DstRev2),
            3 => Some(Self::Psu),
            _ => None,
        }
    }
}

pub fn control_id(sender: u32) -> Option<u32> {
    (sender <= MAX_ID).then_some((CONTROL_CLASS << 24) | sender)
}
pub fn status_id(sender: u32) -> Option<u32> {
    (sender <= MAX_ID).then_some((STATUS_CLASS << 24) | sender)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Begin {
    pub target: u32,
    pub kind: ImageKind,
    pub session: u32,
    pub size: u32,
    pub crc32: u32,
}
impl Begin {
    pub fn data(self) -> Option<[u8; 20]> {
        if self.target > MAX_ID || self.session == 0 || self.size == 0 {
            return None;
        }
        let mut out = [0; 20];
        out[..2].copy_from_slice(&[1, 1]);
        out[2..5].copy_from_slice(&self.target.to_be_bytes()[1..]);
        out[5] = self.kind as u8;
        out[6..10].copy_from_slice(&self.session.to_be_bytes());
        out[10..14].copy_from_slice(&self.size.to_be_bytes());
        out[14..18].copy_from_slice(&self.crc32.to_be_bytes());
        Some(out)
    }
    pub fn decode(id: u32, data: &[u8]) -> Option<Self> {
        if id >> 24 != CONTROL_CLASS
            || data.len() != 20
            || data[..2] != [1, 1]
            || data[18..] != [0, 0]
        {
            return None;
        }
        let value = Self {
            target: u32::from_be_bytes([0, data[2], data[3], data[4]]),
            kind: ImageKind::from_byte(data[5])?,
            session: u32::from_be_bytes(data[6..10].try_into().ok()?),
            size: u32::from_be_bytes(data[10..14].try_into().ok()?),
            crc32: u32::from_be_bytes(data[14..18].try_into().ok()?),
        };
        value.data()?;
        Some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub target: u32,
    pub session: u32,
    pub offset: u32,
    pub len: u8,
    pub bytes: [u8; 48],
}
impl Chunk {
    pub fn data(self) -> Option<[u8; 64]> {
        if self.target > MAX_ID
            || self.session == 0
            || self.len == 0
            || self.len > 48
            || self.offset % 48 != 0
        {
            return None;
        }
        let mut out = [0xff; 64];
        out[..2].copy_from_slice(&[1, 2]);
        out[2..5].copy_from_slice(&self.target.to_be_bytes()[1..]);
        out[5..9].copy_from_slice(&self.session.to_be_bytes());
        out[9..13].copy_from_slice(&self.offset.to_be_bytes());
        out[13] = self.len;
        out[14..62].copy_from_slice(&self.bytes);
        Some(out)
    }
    pub fn decode(id: u32, data: &[u8]) -> Option<Self> {
        if id >> 24 != CONTROL_CLASS
            || data.len() != 64
            || data[..2] != [1, 2]
            || data[62..] != [0xff, 0xff]
        {
            return None;
        }
        let value = Self {
            target: u32::from_be_bytes([0, data[2], data[3], data[4]]),
            session: u32::from_be_bytes(data[5..9].try_into().ok()?),
            offset: u32::from_be_bytes(data[9..13].try_into().ok()?),
            len: data[13],
            bytes: data[14..62].try_into().ok()?,
        };
        value.data()?;
        if value.bytes[usize::from(value.len)..]
            .iter()
            .any(|byte| *byte != 0xff)
        {
            return None;
        }
        Some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Finish {
    pub target: u32,
    pub session: u32,
}
impl Finish {
    pub fn data(self) -> Option<[u8; 12]> {
        if self.target > MAX_ID || self.session == 0 {
            return None;
        }
        let mut out = [0; 12];
        out[..2].copy_from_slice(&[1, 3]);
        out[2..5].copy_from_slice(&self.target.to_be_bytes()[1..]);
        out[5..9].copy_from_slice(&self.session.to_be_bytes());
        Some(out)
    }
    pub fn decode(id: u32, data: &[u8]) -> Option<Self> {
        if id >> 24 != CONTROL_CLASS
            || data.len() != 12
            || data[..2] != [1, 3]
            || data[9..] != [0, 0, 0]
        {
            return None;
        }
        let value = Self {
            target: u32::from_be_bytes([0, data[2], data[3], data[4]]),
            session: u32::from_be_bytes(data[5..9].try_into().ok()?),
        };
        value.data()?;
        Some(value)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Phase {
    Ready = 1,
    Progress = 2,
    Rebooting = 3,
    Rejected = 4,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub session: u32,
    pub next_offset: u32,
    pub phase: Phase,
    pub error: u8,
}
impl Status {
    pub fn data(self) -> [u8; 12] {
        let mut out = [0; 12];
        out[..2].copy_from_slice(&[1, 1]);
        out[2..6].copy_from_slice(&self.session.to_be_bytes());
        out[6..10].copy_from_slice(&self.next_offset.to_be_bytes());
        out[10] = self.phase as u8;
        out[11] = self.error;
        out
    }
    pub fn decode(id: u32, data: &[u8]) -> Option<Self> {
        if id >> 24 != STATUS_CLASS || data.len() != 12 || data[..2] != [1, 1] {
            return None;
        }
        let phase = match data[10] {
            1 => Phase::Ready,
            2 => Phase::Progress,
            3 => Phase::Rebooting,
            4 => Phase::Rejected,
            _ => return None,
        };
        Some(Self {
            session: u32::from_be_bytes(data[2..6].try_into().ok()?),
            next_offset: u32::from_be_bytes(data[6..10].try_into().ok()?),
            phase,
            error: data[11],
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Crc32(u32);
impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}
impl Crc32 {
    pub const fn new() -> Self {
        Self(0xffff_ffff)
    }
    pub fn update(&mut self, data: &[u8]) {
        for &byte in data {
            self.0 ^= u32::from(byte);
            for _ in 0..8 {
                self.0 = (self.0 >> 1) ^ (0xedb8_8320 & (0u32.wrapping_sub(self.0 & 1)));
            }
        }
    }
    pub const fn finish(self) -> u32 {
        !self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn frames_round_trip_and_reject_malformed_padding() {
        let id = control_id(0x123456).unwrap();
        let begin = Begin {
            target: 0xabcdef,
            kind: ImageKind::DstRev1,
            session: 7,
            size: 50,
            crc32: 9,
        };
        assert_eq!(Begin::decode(id, &begin.data().unwrap()), Some(begin));
        let chunk = Chunk {
            target: 0xabcdef,
            session: 7,
            offset: 0,
            len: 2,
            bytes: [0xff; 48],
        };
        let mut wire = chunk.data().unwrap();
        assert_eq!(Chunk::decode(id, &wire), Some(chunk));
        wire[17] = 1;
        assert_eq!(Chunk::decode(id, &wire), None);
        let finish = Finish {
            target: 0xabcdef,
            session: 7,
        };
        assert_eq!(Finish::decode(id, &finish.data().unwrap()), Some(finish));
    }
    #[test]
    fn crc_matches_standard_vector() {
        let mut crc = Crc32::new();
        crc.update(b"1234");
        crc.update(b"56789");
        assert_eq!(crc.finish(), 0xcbf4_3926);
    }
}
