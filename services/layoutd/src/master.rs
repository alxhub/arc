//! Select once after a good layout, then grant only that source until restart.
use crate::{CanNetwork, Health, LayoutStatus, NodeObservation};

#[derive(Default)]
pub struct Master {
    selected: Option<u32>,
    next_grant_ms: u64,
}

impl Master {
    pub fn selected(&self) -> Option<u32> {
        self.selected
    }

    pub fn update(
        &mut self,
        status: &LayoutStatus,
        nodes: &[NodeObservation],
        now_ms: u64,
        can: &mut impl CanNetwork,
    ) -> Result<(), String> {
        if status.state != Health::Good {
            return Ok(());
        }
        // Wait for capability reports from all accounted-for hardware before choosing.
        if nodes.is_empty() || nodes.iter().any(|node| node.dcc.is_none()) {
            return Ok(());
        }
        if self.selected.is_none() {
            let mut candidates = nodes
                .iter()
                .filter_map(|node| {
                    let dcc = node.dcc?;
                    if !matches!(dcc.kind, 1 | 2) {
                        return None;
                    }
                    let id = parse_id(&node.id)?;
                    Some((!dcc.permitted, dcc.kind, id))
                })
                .collect::<Vec<_>>();
            candidates.sort_unstable();
            // A layoutd restart reuses an already-granted source instead of assigning another.
            if let Some((_, _, id)) = candidates.first() {
                self.selected = Some(*id);
            }
        }
        let Some(target) = self.selected else {
            return Ok(());
        };
        let node = nodes.iter().find(|node| parse_id(&node.id) == Some(target));
        if node
            .and_then(|node| node.dcc)
            .is_some_and(|dcc| !dcc.permitted)
            && now_ms >= self.next_grant_ms
        {
            can.grant_dcc(target)?;
            self.next_grant_ms = now_ms.saturating_add(1_000);
        }
        Ok(())
    }
}

pub fn parse_id(id: &str) -> Option<u32> {
    if id.len() != 6
        || !id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return None;
    }
    u32::from_str_radix(id, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::DccObservation;
    #[derive(Default)]
    struct Can(Vec<u32>);
    impl CanNetwork for Can {
        fn observe(&mut self) -> Result<Vec<NodeObservation>, String> {
            unreachable!()
        }
        fn grant_dcc(&mut self, target: u32) -> Result<(), String> {
            self.0.push(target);
            Ok(())
        }
    }
    fn node(id: &str, kind: u8, permitted: bool) -> NodeObservation {
        NodeObservation {
            id: id.into(),
            dcc: Some(DccObservation {
                kind,
                permitted,
                transmitting: permitted,
            }),
        }
    }
    #[test]
    fn waits_for_good_and_capabilities_then_latches_psu_and_retries_only_it() {
        let mut master = Master::default();
        let mut can = Can::default();
        let mut status = LayoutStatus {
            state: Health::Degraded,
            reasons: vec![],
        };
        let mut nodes = vec![
            node("000001", 2, false),
            node("000002", 1, false),
            node("000003", 0, false),
        ];
        master.update(&status, &nodes, 0, &mut can).unwrap();
        assert!(can.0.is_empty());
        status.state = Health::Good;
        nodes[2].dcc = None;
        master.update(&status, &nodes, 0, &mut can).unwrap();
        assert!(can.0.is_empty());
        nodes[2] = node("000003", 0, false);
        master.update(&status, &nodes, 0, &mut can).unwrap();
        assert_eq!(can.0, [2]);
        master.update(&status, &nodes, 1, &mut can).unwrap();
        assert_eq!(can.0, [2]);
        nodes[1].dcc.as_mut().unwrap().permitted = true;
        master.update(&status, &nodes, 1000, &mut can).unwrap();
        assert_eq!(can.0, [2]);
        nodes.remove(1);
        master.update(&status, &nodes, 2000, &mut can).unwrap();
        assert_eq!(master.selected(), Some(2));
        assert_eq!(can.0, [2]);
    }
    #[test]
    fn selects_rev1_without_psu_and_reuses_existing_grant_on_daemon_restart() {
        let status = LayoutStatus {
            state: Health::Good,
            reasons: vec![],
        };
        let mut can = Can::default();
        let mut master = Master::default();
        master
            .update(
                &status,
                &[node("000004", 2, false), node("000003", 2, false)],
                0,
                &mut can,
            )
            .unwrap();
        assert_eq!(can.0, [3]);
        let mut master = Master::default();
        master
            .update(
                &status,
                &[node("000004", 2, true), node("000001", 1, false)],
                0,
                &mut can,
            )
            .unwrap();
        assert_eq!(master.selected(), Some(4));
        assert_eq!(can.0, [3]);
    }
}
