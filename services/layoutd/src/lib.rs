pub mod can;
pub mod master;

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Layout {
    pub nodes: BTreeMap<String, NodeFact>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeFact {
    pub kind: Kind,
    pub topology: Topology,
    pub binding: Option<BoardEndpoint>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct BoardEndpoint {
    pub node_id: String,
    pub output: u8,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum Topology {
    Unknown,
    Configured { connections: Vec<Connection> },
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Connection {
    pub port: String,
    pub neighbor_id: String,
    pub neighbor_port: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    District,
    Turnout,
    /// An expected backbone board without a track endpoint (for example PSU).
    Infrastructure,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct NodeObservation {
    pub id: String,
    #[serde(default)]
    pub dcc: Option<DccObservation>,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct DccObservation {
    pub kind: u8,
    pub permitted: bool,
    pub transmitting: bool,
}

/// A CAN backend supplies board observations and sends targeted transmit grants.
pub trait CanNetwork {
    fn observe(&mut self) -> Result<Vec<NodeObservation>, String>;
    fn grant_dcc(&mut self, target: u32) -> Result<(), String> {
        let _ = target;
        Err("CAN backend cannot send DCC grants".into())
    }
}

/// A successful call means the broker acknowledged the retained QoS 1 message.
pub trait Publisher {
    fn publish_retained(&mut self, topic: &str, payload: &[u8]) -> Result<(), String>;
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Updating,
    Good,
    Invalid,
    Degraded,
    Offline,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct LayoutStatus {
    pub state: Health,
    pub reasons: Vec<String>,
}

/// Serialized as a plain JSON array of node IDs.
pub type Index = BTreeSet<String>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SetNodeCommand {
    pub request_id: String,
    pub id: String,
    pub fact: Option<NodeFact>,
}

pub fn topic_segment(value: &str) -> bool {
    !value.is_empty() && !value.contains('/') && !value.contains(['+', '#'])
}

pub fn status_topic(prefix: &str) -> String {
    format!("/{prefix}/layout/status")
}

pub fn index_topic(prefix: &str) -> String {
    format!("/{prefix}/layout/index")
}

pub fn fact_topic(prefix: &str, id: &str) -> String {
    format!("/{prefix}/layout/node/{id}/fact")
}

pub fn command_topic(prefix: &str) -> String {
    format!("/{prefix}/layout/command/set")
}

pub fn check_command(current: Option<&Layout>, command: &SetNodeCommand) -> Result<(), String> {
    if command.request_id.is_empty() || !topic_segment(&command.id) {
        return Err("request_id and node id must be nonempty; id must be one topic segment".into());
    }
    if current.is_none() && command.fact.is_none() {
        return Err("cannot delete from an unconfigured layout".into());
    }
    Ok(())
}

pub fn apply_command(current: Option<&Layout>, command: &SetNodeCommand) -> Layout {
    let mut next = current.cloned().unwrap_or(Layout {
        nodes: BTreeMap::new(),
    });
    match &command.fact {
        Some(fact) => {
            next.nodes.insert(command.id.clone(), fact.clone());
        }
        None => {
            next.nodes.remove(&command.id);
        }
    }
    next
}

pub fn validate(layout: &Layout) -> Vec<String> {
    let mut errors = Vec::new();
    let mut endpoints = BTreeSet::new();
    for (id, node) in &layout.nodes {
        if !topic_segment(id) {
            errors.push(format!("{id}: invalid node id"));
        }
        if matches!(node.kind, Kind::District | Kind::Infrastructure) && node.binding.is_none() {
            errors.push(format!("{id}: node has no physical binding"));
        }
        if let Some(binding) = &node.binding
            && !endpoints.insert(binding)
        {
            errors.push(format!("{id}: physical endpoint is bound twice"));
        }
        let Topology::Configured { connections } = &node.topology else {
            errors.push(format!("{id}: topology is unknown"));
            continue;
        };
        let mut ports = BTreeSet::new();
        for edge in connections {
            if edge.port.is_empty() || edge.neighbor_port.is_empty() {
                errors.push(format!("{id}: connection ports must not be empty"));
            }
            if !ports.insert(&edge.port) {
                errors.push(format!("{id}: port {} is used twice", edge.port));
            }
            let Some(neighbor) = layout.nodes.get(&edge.neighbor_id) else {
                errors.push(format!("{id}: neighbor {} is missing", edge.neighbor_id));
                continue;
            };
            let Topology::Configured {
                connections: reverse,
            } = &neighbor.topology
            else {
                errors.push(format!(
                    "{id}: neighbor {} is unconfigured",
                    edge.neighbor_id
                ));
                continue;
            };
            if !reverse.iter().any(|candidate| {
                candidate.port == edge.neighbor_port
                    && candidate.neighbor_id == *id
                    && candidate.neighbor_port == edge.port
            }) {
                errors.push(format!(
                    "{id}: {}:{} has no reciprocal connection",
                    edge.neighbor_id, edge.neighbor_port
                ));
            }
        }
    }
    errors
}

pub fn evaluate(layout: &Layout, observation: Result<&[NodeObservation], &str>) -> LayoutStatus {
    let errors = validate(layout);
    if !errors.is_empty() {
        return LayoutStatus {
            state: Health::Invalid,
            reasons: errors,
        };
    }
    let nodes = match observation {
        Ok(nodes) => nodes,
        Err(error) => {
            return LayoutStatus {
                state: Health::Degraded,
                reasons: vec![format!("CAN observation unavailable: {error}")],
            };
        }
    };
    let expected: BTreeSet<_> = layout
        .nodes
        .values()
        .filter_map(|node| {
            node.binding
                .as_ref()
                .map(|binding| binding.node_id.as_str())
        })
        .collect();
    let observed: BTreeSet<_> = nodes.iter().map(|node| node.id.as_str()).collect();
    let mut reasons = Vec::new();
    if nodes.len() != observed.len() {
        reasons.push("duplicate CAN node identity".into());
    }
    for id in &expected {
        if !observed.contains(id) {
            reasons.push(format!("{id}: expected CAN node is absent"));
        }
    }
    for id in &observed {
        if !expected.contains(id) {
            reasons.push(format!("{id}: unexpected CAN node"));
        }
    }
    LayoutStatus {
        state: if reasons.is_empty() {
            Health::Good
        } else {
            Health::Degraded
        },
        reasons,
    }
}

pub fn publish_status(
    publisher: &mut impl Publisher,
    prefix: &str,
    status: &LayoutStatus,
) -> Result<(), String> {
    let bytes = serde_json::to_vec(status).map_err(|error| error.to_string())?;
    publisher.publish_retained(&status_topic(prefix), &bytes)
}

pub fn publish_layout(
    publisher: &mut impl Publisher,
    prefix: &str,
    layout: &Layout,
    previous: Option<&Layout>,
    can: &mut impl CanNetwork,
) -> Result<LayoutStatus, String> {
    publish_status(
        publisher,
        prefix,
        &LayoutStatus {
            state: Health::Updating,
            reasons: vec![],
        },
    )?;
    for (id, fact) in &layout.nodes {
        let bytes = serde_json::to_vec(fact).map_err(|error| error.to_string())?;
        publisher.publish_retained(&fact_topic(prefix, id), &bytes)?;
    }
    if let Some(previous) = previous {
        for id in previous
            .nodes
            .keys()
            .filter(|id| !layout.nodes.contains_key(*id))
        {
            publisher.publish_retained(&fact_topic(prefix, id), &[])?;
        }
    }
    let index: Index = layout.nodes.keys().cloned().collect();
    let bytes = serde_json::to_vec(&index).map_err(|error| error.to_string())?;
    publisher.publish_retained(&index_topic(prefix), &bytes)?;
    let observation = can.observe();
    let status = evaluate(
        layout,
        observation
            .as_ref()
            .map(Vec::as_slice)
            .map_err(String::as_str),
    );
    publish_status(publisher, prefix, &status)?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Memory(Vec<(String, Vec<u8>)>);
    impl Publisher for Memory {
        fn publish_retained(&mut self, topic: &str, payload: &[u8]) -> Result<(), String> {
            self.0.push((topic.into(), payload.into()));
            Ok(())
        }
    }
    struct FakeCan(Vec<NodeObservation>);
    impl CanNetwork for FakeCan {
        fn observe(&mut self) -> Result<Vec<NodeObservation>, String> {
            Ok(self.0.clone())
        }
    }
    fn fact(id: &str) -> NodeFact {
        NodeFact {
            kind: Kind::District,
            topology: Topology::Configured {
                connections: if id == "a" {
                    vec![Connection {
                        port: "east".into(),
                        neighbor_id: "b".into(),
                        neighbor_port: "west".into(),
                    }]
                } else {
                    vec![Connection {
                        port: "west".into(),
                        neighbor_id: "a".into(),
                        neighbor_port: "east".into(),
                    }]
                },
            },
            binding: Some(BoardEndpoint {
                node_id: "stm32-1".into(),
                output: if id == "a" { 0 } else { 1 },
            }),
        }
    }

    #[test]
    fn commits_status_after_indexed_node_facts() {
        let layout = Layout {
            nodes: BTreeMap::from([("a".into(), fact("a")), ("b".into(), fact("b"))]),
        };
        let mut publisher = Memory::default();
        let mut can = FakeCan(vec![NodeObservation {
            id: "stm32-1".into(),
            dcc: None,
        }]);
        let status = publish_layout(&mut publisher, "test", &layout, None, &mut can).unwrap();
        assert_eq!(status.state, Health::Good);
        assert_eq!(publisher.0.first().unwrap().0, "/test/layout/status");
        assert_eq!(publisher.0.last().unwrap().0, "/test/layout/status");
        assert!(
            publisher
                .0
                .iter()
                .any(|(topic, _)| topic == "/test/layout/node/a/fact")
        );
        let index_position = publisher
            .0
            .iter()
            .position(|(topic, _)| topic == "/test/layout/index")
            .unwrap();
        assert!(index_position > 1 && index_position < publisher.0.len() - 1);
    }

    #[test]
    fn missing_reciprocal_edge_and_missing_board_block_good() {
        let mut layout = Layout {
            nodes: BTreeMap::from([("a".into(), fact("a")), ("b".into(), fact("b"))]),
        };
        layout.nodes.get_mut("b").unwrap().topology = Topology::Unknown;
        assert_eq!(evaluate(&layout, Ok(&[])).state, Health::Invalid);
        layout.nodes.insert("b".into(), fact("b"));
        assert_eq!(evaluate(&layout, Ok(&[])).state, Health::Degraded);
    }

    #[test]
    fn delete_clears_fact_and_index_membership() {
        let initial = Layout {
            nodes: BTreeMap::from([("a".into(), fact("a"))]),
        };
        let command = SetNodeCommand {
            request_id: "req".into(),
            id: "a".into(),
            fact: None,
        };
        assert!(check_command(Some(&initial), &command).is_ok());
        let next = apply_command(Some(&initial), &command);
        let mut publisher = Memory::default();
        publish_layout(
            &mut publisher,
            "test",
            &next,
            Some(&initial),
            &mut FakeCan(vec![]),
        )
        .unwrap();
        let index: Index = serde_json::from_slice(
            &publisher
                .0
                .iter()
                .find(|(topic, _)| topic == "/test/layout/index")
                .unwrap()
                .1,
        )
        .unwrap();
        assert!(index.is_empty());
        assert!(
            publisher.0.iter().any(|(topic, payload)| {
                topic == "/test/layout/node/a/fact" && payload.is_empty()
            })
        );
    }
}
