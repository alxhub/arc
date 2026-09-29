//! Boot-lifetime permission shared by embedded and simulated sources.
#[derive(Default)]
pub struct Permission(bool);
impl Permission {
    pub const fn new() -> Self {
        Self(false)
    }
    pub fn granted(&self) -> bool {
        self.0
    }
    pub fn receive(&mut self, id: u32, data: &[u8], own_id: u32) -> bool {
        if link::DccGrant::decode(id, data).is_some_and(|grant| grant.target == own_id) {
            self.0 = true;
            true
        } else {
            false
        }
    }
    pub fn transmitting(&self, power_ready: bool) -> bool {
        self.0 && power_ready
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grant_is_targeted_idempotent_and_survives_power_loss_until_restart() {
        let mut permission = Permission::new();
        let id = link::DccGrant::can_id(1).unwrap();
        let data = link::DccGrant { target: 2 }.data().unwrap();
        assert!(!permission.transmitting(true));
        assert!(!permission.receive(id, &data, 3));
        assert!(!permission.receive(id, &data[..4], 2));
        assert!(permission.receive(id, &data, 2));
        assert!(permission.receive(id, &data, 2));
        assert!(!permission.transmitting(false));
        assert!(permission.transmitting(true));
        assert!(!Permission::new().granted());
    }
}
