use layoutd::{
    CanNetwork, Health, Index, Layout, LayoutStatus, NodeFact, NodeObservation, Publisher,
    SetNodeCommand, apply_command, check_command, command_topic, index_topic, publish_layout,
    publish_status, status_topic, topic_segment,
};
use rumqttc::{Client, Event, Incoming, LastWill, MqttOptions, QoS};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

#[path = "../../../firmware/shared/host_clock.rs"]
mod host_clock;
use host_clock::HostClock;

#[derive(Deserialize)]
struct SimulatedCanState {
    nodes: Vec<NodeObservation>,
}

struct SimulatedCan {
    path: PathBuf,
}

impl CanNetwork for SimulatedCan {
    fn observe(&mut self) -> Result<Vec<NodeObservation>, String> {
        let bytes = fs::read(&self.path).map_err(|error| error.to_string())?;
        let state: SimulatedCanState =
            serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        Ok(state.nodes)
    }
    fn grant_dcc(&mut self, target: u32) -> Result<(), String> {
        let mut out = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.with_extension("commands.jsonl"))
            .map_err(|e| e.to_string())?;
        writeln!(out, "{}", layoutd::can::grant_frame(target)?).map_err(|e| e.to_string())
    }
}

enum CanBackend {
    File(SimulatedCan),
    Tcp(layoutd::can::TcpCan),
}
impl CanNetwork for CanBackend {
    fn observe(&mut self) -> Result<Vec<NodeObservation>, String> {
        match self {
            Self::File(can) => can.observe(),
            Self::Tcp(can) => can.observe(),
        }
    }
    fn grant_dcc(&mut self, target: u32) -> Result<(), String> {
        match self {
            Self::File(can) => can.grant_dcc(target),
            Self::Tcp(can) => can.grant_dcc(target),
        }
    }
}

impl CanBackend {
    fn set_time(&mut self, now_ms: u64) {
        if let Self::Tcp(can) = self {
            can.set_time(now_ms);
        }
    }

    fn wait_marker(&self, now_ms: u64) {
        if let Self::Tcp(can) = self {
            can.wait_marker(now_ms);
        }
    }
}

enum BrokerEvent {
    Connected,
    Subscribed,
    Acknowledged,
    Failed(String),
}

struct Message {
    topic: String,
    payload: Vec<u8>,
    retained: bool,
}

struct Mqtt {
    client: Client,
    events: Receiver<BrokerEvent>,
    messages: Receiver<Message>,
}

impl Mqtt {
    fn connect(host: &str, port: u16, prefix: &str) -> Result<Self, String> {
        let mut options = MqttOptions::new(format!("arc-layoutd-{prefix}"), host, port);
        options.set_keep_alive(Duration::from_secs(5));
        let offline = serde_json::to_vec(&LayoutStatus {
            state: Health::Offline,
            reasons: vec!["layoutd disconnected".into()],
        })
        .map_err(|error| error.to_string())?;
        options.set_last_will(LastWill::new(
            status_topic(prefix),
            offline,
            QoS::AtLeastOnce,
            true,
        ));
        let (client, mut connection) = Client::new(options, 16);
        let (event_sender, events) = mpsc::channel();
        let (message_sender, messages) = mpsc::channel();
        thread::spawn(move || {
            for event in connection.iter() {
                let notification = match event {
                    Ok(Event::Incoming(Incoming::ConnAck(_))) => Some(BrokerEvent::Connected),
                    Ok(Event::Incoming(Incoming::SubAck(_))) => Some(BrokerEvent::Subscribed),
                    Ok(Event::Incoming(Incoming::PubAck(_))) => Some(BrokerEvent::Acknowledged),
                    Ok(Event::Incoming(Incoming::Publish(publish))) => {
                        if message_sender
                            .send(Message {
                                topic: publish.topic,
                                payload: publish.payload.to_vec(),
                                retained: publish.retain,
                            })
                            .is_err()
                        {
                            break;
                        }
                        None
                    }
                    Err(error) => Some(BrokerEvent::Failed(error.to_string())),
                    _ => None,
                };
                if let Some(notification) = notification
                    && event_sender.send(notification).is_err()
                {
                    break;
                }
            }
        });
        let mut mqtt = Self {
            client,
            events,
            messages,
        };
        mqtt.wait_for(
            |event| matches!(event, BrokerEvent::Connected),
            "connection",
        )?;
        for topic in [
            index_topic(prefix),
            format!("/{prefix}/layout/node/+/fact"),
            command_topic(prefix),
        ] {
            mqtt.client
                .subscribe(topic, QoS::AtLeastOnce)
                .map_err(|error| error.to_string())?;
            mqtt.wait_for(
                |event| matches!(event, BrokerEvent::Subscribed),
                "subscription",
            )?;
        }
        Ok(mqtt)
    }

    fn wait_for(
        &mut self,
        expected: impl Fn(&BrokerEvent) -> bool,
        context: &str,
    ) -> Result<(), String> {
        match self.events.recv_timeout(Duration::from_secs(10)) {
            Ok(event) if expected(&event) => Ok(()),
            Ok(BrokerEvent::Failed(error)) => Err(format!("MQTT {context}: {error}")),
            Ok(_) => Err(format!("unexpected MQTT event during {context}")),
            Err(error) => Err(format!("waiting for MQTT {context}: {error}")),
        }
    }

    fn publish(&mut self, topic: &str, payload: &[u8], retained: bool) -> Result<(), String> {
        self.client
            .publish(topic, QoS::AtLeastOnce, retained, payload)
            .map_err(|error| error.to_string())?;
        self.wait_for(|event| matches!(event, BrokerEvent::Acknowledged), "PUBACK")
    }

    fn check_connection(&mut self) -> Result<(), String> {
        if let Ok(event) = self.events.try_recv() {
            match event {
                BrokerEvent::Failed(error) => Err(format!("MQTT connection lost: {error}")),
                BrokerEvent::Connected => {
                    Err("MQTT reconnected; restart layoutd to rebuild retained state".into())
                }
                _ => Err("unexpected MQTT acknowledgement".into()),
            }
        } else {
            Ok(())
        }
    }
}

impl Publisher for Mqtt {
    fn publish_retained(&mut self, topic: &str, payload: &[u8]) -> Result<(), String> {
        self.publish(topic, payload, true)
    }
}

#[derive(Default)]
struct RetainedFacts {
    index: Option<Index>,
    facts: BTreeMap<String, NodeFact>,
}

impl RetainedFacts {
    fn ingest(&mut self, prefix: &str, message: &Message) -> Result<(), String> {
        if !message.retained {
            return Ok(());
        }
        if message.topic == index_topic(prefix) {
            self.index = Some(
                serde_json::from_slice(&message.payload)
                    .map_err(|error| format!("invalid retained index: {error}"))?,
            );
        } else if let Some(id) = message
            .topic
            .strip_prefix(&format!("/{prefix}/layout/node/"))
            .and_then(|suffix| suffix.strip_suffix("/fact"))
            && topic_segment(id)
        {
            if message.payload.is_empty() {
                self.facts.remove(id);
            } else {
                let fact: NodeFact = serde_json::from_slice(&message.payload)
                    .map_err(|error| format!("invalid retained fact for {id}: {error}"))?;
                self.facts.insert(id.into(), fact);
            }
        }
        Ok(())
    }

    fn layout(&self) -> Option<Layout> {
        let index = self.index.as_ref()?;
        let nodes = index
            .iter()
            .map(|id| self.facts.get(id).map(|fact| (id.clone(), fact.clone())))
            .collect::<Option<BTreeMap<_, _>>>()?;
        Some(Layout { nodes })
    }
}

#[derive(Serialize)]
struct CommandResult {
    request_id: Option<String>,
    accepted: bool,
    error: Option<String>,
}

fn send_result(mqtt: &mut Mqtt, prefix: &str, result: CommandResult) -> Result<(), String> {
    let payload = serde_json::to_vec(&result).map_err(|error| error.to_string())?;
    mqtt.publish(&format!("/{prefix}/layout/command/result"), &payload, false)
}

fn handle_command(
    mqtt: &mut Mqtt,
    can: &mut impl CanNetwork,
    prefix: &str,
    current: &mut Option<Layout>,
    published: &mut LayoutStatus,
    message: &Message,
) -> Result<(), String> {
    if message.retained {
        return Ok(());
    }
    let command: SetNodeCommand = match serde_json::from_slice(&message.payload) {
        Ok(command) => command,
        Err(error) => {
            return send_result(
                mqtt,
                prefix,
                CommandResult {
                    request_id: None,
                    accepted: false,
                    error: Some(format!("invalid command: {error}")),
                },
            );
        }
    };
    let error = check_command(current.as_ref(), &command).err();
    if let Some(error) = error {
        return send_result(
            mqtt,
            prefix,
            CommandResult {
                request_id: Some(command.request_id),
                accepted: false,
                error: Some(error),
            },
        );
    }
    let next = apply_command(current.as_ref(), &command);
    *published = publish_layout(mqtt, prefix, &next, current.as_ref(), can)?;
    *current = Some(next);
    send_result(
        mqtt,
        prefix,
        CommandResult {
            request_id: Some(command.request_id),
            accepted: true,
            error: None,
        },
    )
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if !(3..=5).contains(&args.len()) {
        return Err(
            "usage: layoutd <layout-prefix> <sim-can.json|tcp://host:port> [mqtt-host] [mqtt-port]"
                .into(),
        );
    }
    let prefix = &args[1];
    if !topic_segment(prefix) {
        return Err("layout prefix must be one MQTT topic segment".into());
    }
    let host = args.get(3).map(String::as_str).unwrap_or("127.0.0.1");
    let port = args
        .get(4)
        .map(|value| value.parse::<u16>())
        .transpose()
        .map_err(|error| error.to_string())?
        .unwrap_or(1883);
    let clock_address = std::env::var("ARC_SIM_CLOCK").ok().filter(|value| !value.is_empty());
    let mut clock = clock_address.as_deref().map(|address| HostClock::connect(address, "layoutd")).transpose()?;
    let mut can = if let Some(address) = args[2].strip_prefix("tcp://") {
        CanBackend::Tcp(layoutd::can::TcpCan::connect(address, clock.is_some())?)
    } else {
        CanBackend::File(SimulatedCan {
            path: PathBuf::from(&args[2]),
        })
    };
    let mut mqtt = Mqtt::connect(host, port, prefix)?;
    let mut published = LayoutStatus {
        state: Health::Updating,
        reasons: vec![],
    };
    publish_status(&mut mqtt, prefix, &published)?;
    let mut master = layoutd::master::Master::default();
    let mut published_master = None;
    mqtt.publish(&format!("/{prefix}/layout/dcc_master"), b"null", true)?;
    let started = Instant::now();
    let mut retained = RetainedFacts::default();
    let mut current: Option<Layout> = None;
    let mut bootstrap_complete = false;
    let bootstrap_deadline_ms = 2_000u64;
    eprintln!("layoutd: serving {prefix}");

    loop {
        mqtt.check_connection()?;
        let message = if clock.is_some() {
            mqtt.messages.try_recv().ok()
        } else {
            mqtt.messages.recv_timeout(Duration::from_millis(200)).ok()
        };
        if let Some(message) = &message {
            retained.ingest(prefix, message)?;
            if current.is_none()
                && let Some(layout) = retained.layout()
            {
                published = publish_layout(&mut mqtt, prefix, &layout, None, &mut can)?;
                current = Some(layout);
                bootstrap_complete = true;
            }
            if message.topic == command_topic(prefix) && !message.retained {
                if !bootstrap_complete {
                    send_result(
                        &mut mqtt,
                        prefix,
                        CommandResult {
                            request_id: None,
                            accepted: false,
                            error: Some("retained layout state is still loading; retry".into()),
                        },
                    )?;
                } else if current.is_none() && retained.index.is_some() {
                    send_result(
                        &mut mqtt,
                        prefix,
                        CommandResult {
                            request_id: None,
                            accepted: false,
                            error: Some("retained index references missing node facts".into()),
                        },
                    )?;
                } else {
                    handle_command(
                        &mut mqtt,
                        &mut can,
                        prefix,
                        &mut current,
                        &mut published,
                        message,
                    )?;
                }
            }
        }
        let elapsed = if let Some(clock) = &mut clock {
            let Some(now) = clock.wait()? else { continue };
            can.wait_marker(now);
            can.set_time(now);
            now
        } else {
            started.elapsed().as_millis() as u64
        };
        if !bootstrap_complete && elapsed >= bootstrap_deadline_ms {
            bootstrap_complete = true;
            published = LayoutStatus {
                state: Health::Invalid,
                reasons: vec![if retained.index.is_some() {
                    "retained index references missing node facts".into()
                } else {
                    "no retained layout index; waiting for the first command".into()
                }],
            };
            publish_status(&mut mqtt, prefix, &published)?;
        }
        if let Some(layout) = &current {
            let observation = can.observe();
            let status = layoutd::evaluate(
                layout,
                observation
                    .as_ref()
                    .map(Vec::as_slice)
                    .map_err(String::as_str),
            );
            if let Ok(nodes) = &observation {
                master.update(
                    &status,
                    nodes,
                    elapsed,
                    &mut can,
                )?;
                if master.selected() != published_master {
                    let payload =
                        serde_json::to_vec(&master.selected().map(|id| format!("{id:06x}")))
                            .map_err(|e| e.to_string())?;
                    mqtt.publish(&format!("/{prefix}/layout/dcc_master"), &payload, true)?;
                    published_master = master.selected();
                }
            }
            if status != published {
                publish_status(&mut mqtt, prefix, &status)?;
                published = status;
            }
        }
        if let Some(clock) = &mut clock {
            clock.ack()?;
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("layoutd: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use layoutd::{BoardEndpoint, Kind, NodeFact, Topology};

    #[test]
    fn index_waits_for_each_named_fact() {
        let fact = NodeFact {
            kind: Kind::District,
            topology: Topology::Configured {
                connections: vec![],
            },
            binding: Some(BoardEndpoint {
                node_id: "board".into(),
                output: 0,
            }),
        };
        let mut retained = RetainedFacts::default();
        retained
            .ingest(
                "test",
                &Message {
                    topic: index_topic("test"),
                    payload: serde_json::to_vec(&Index::from(["a".into(), "b".into()])).unwrap(),
                    retained: true,
                },
            )
            .unwrap();
        for id in ["a", "b"] {
            retained
                .ingest(
                    "test",
                    &Message {
                        topic: layoutd::fact_topic("test", id),
                        payload: serde_json::to_vec(&fact).unwrap(),
                        retained: true,
                    },
                )
                .unwrap();
            if id == "a" {
                assert!(retained.layout().is_none());
            }
        }
        assert_eq!(retained.layout().unwrap().nodes.len(), 2);
    }
}
