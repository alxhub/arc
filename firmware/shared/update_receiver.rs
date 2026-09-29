// Shared application-side CAN transfer and embassy-boot staging policy.
use embassy_boot_stm32::BlockingFirmwareUpdater;
use embedded_storage::nor_flash::NorFlash;
use link::update::{self, Begin, Chunk, Crc32, Finish, ImageKind, Phase, Status};

pub struct Receiver<'a, DFU: NorFlash, STATE: NorFlash> {
    updater: BlockingFirmwareUpdater<'a, DFU, STATE>,
    own_id: u32,
    kind: ImageKind,
    max_size: u32,
    active: Option<Begin>,
    next: u32,
}

impl<'a, DFU: NorFlash, STATE: NorFlash> Receiver<'a, DFU, STATE> {
    pub fn new(
        updater: BlockingFirmwareUpdater<'a, DFU, STATE>,
        own_id: u32,
        kind: ImageKind,
        max_size: u32,
    ) -> Self {
        Self {
            updater,
            own_id,
            kind,
            max_size,
            active: None,
            next: 0,
        }
    }

    pub fn confirm_boot(&mut self) {
        self.updater.mark_booted().expect("mark booted");
    }

    fn reply(&self, session: u32, phase: Phase, error: u8) -> Status {
        Status {
            session,
            next_offset: self.next,
            phase,
            error,
        }
    }

    pub fn handle(&mut self, id: u32, data: &[u8]) -> Option<Status> {
        if id >> 24 != update::CONTROL_CLASS {
            return None;
        }
        if let Some(begin) = Begin::decode(id, data) {
            if begin.target != self.own_id {
                return None;
            }
            if begin.kind != self.kind || begin.size > self.max_size || begin.size < 8 {
                return Some(self.reply(begin.session, Phase::Rejected, 1));
            }
            self.active = None;
            self.next = 0;
            if self.updater.prepare_update().is_err() {
                return Some(self.reply(begin.session, Phase::Rejected, 2));
            }
            self.active = Some(begin);
            return Some(self.reply(begin.session, Phase::Ready, 0));
        }
        if let Some(chunk) = Chunk::decode(id, data) {
            if chunk.target != self.own_id {
                return None;
            }
            let Some(begin) = self.active else {
                return Some(self.reply(chunk.session, Phase::Rejected, 3));
            };
            if chunk.session != begin.session {
                return Some(self.reply(chunk.session, Phase::Rejected, 3));
            }
            if chunk.offset < self.next {
                return Some(self.reply(chunk.session, Phase::Progress, 0));
            }
            if chunk.offset != self.next || u32::from(chunk.len) != (begin.size - self.next).min(48)
            {
                return Some(self.reply(chunk.session, Phase::Rejected, 4));
            }
            let padded = usize::from(chunk.len).div_ceil(8) * 8;
            if self
                .updater
                .write_firmware(chunk.offset as usize, &chunk.bytes[..padded])
                .is_err()
            {
                self.active = None;
                return Some(self.reply(chunk.session, Phase::Rejected, 2));
            }
            self.next += u32::from(chunk.len);
            return Some(self.reply(chunk.session, Phase::Progress, 0));
        }
        if let Some(finish) = Finish::decode(id, data) {
            if finish.target != self.own_id {
                return None;
            }
            let Some(begin) = self.active else {
                return Some(self.reply(finish.session, Phase::Rejected, 3));
            };
            if finish.session != begin.session || self.next != begin.size {
                return Some(self.reply(finish.session, Phase::Rejected, 4));
            }
            let mut crc = Crc32::new();
            let mut offset = 0;
            let mut buf = [0; 48];
            while offset < begin.size {
                let len = (begin.size - offset).min(48) as usize;
                if self.updater.read_dfu(offset, &mut buf[..len]).is_err() {
                    return Some(self.reply(finish.session, Phase::Rejected, 2));
                }
                crc.update(&buf[..len]);
                offset += len as u32;
            }
            if crc.finish() != begin.crc32 {
                self.active = None;
                return Some(self.reply(finish.session, Phase::Rejected, 5));
            }
            let mut vectors = [0u8; 8];
            if self.updater.read_dfu(0, &mut vectors).is_err() {
                return Some(self.reply(finish.session, Phase::Rejected, 2));
            }
            let stack = u32::from_le_bytes(vectors[..4].try_into().unwrap());
            let reset = u32::from_le_bytes(vectors[4..].try_into().unwrap());
            let ram_end = if self.kind == ImageKind::DstRev1 {
                0x2002_3ff0
            } else {
                0x2000_7800
            };
            if !(0x2000_0000..=ram_end).contains(&stack)
                || reset & 1 == 0
                || !(0x0800_4000..0x0800_4000 + begin.size).contains(&(reset & !1))
            {
                self.active = None;
                return Some(self.reply(finish.session, Phase::Rejected, 6));
            }
            if self.updater.mark_updated().is_err() {
                return Some(self.reply(finish.session, Phase::Rejected, 2));
            }
            self.active = None;
            return Some(self.reply(finish.session, Phase::Rebooting, 0));
        }
        None
    }
}
