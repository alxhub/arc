use link::{DistrictStatus, ThrottleSet, ThrottleStatus, valid_loco_address};
use rumqttc::{Client, Event, Incoming, MqttOptions, QoS};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

#[derive(Clone, Deserialize)]
struct Loco {
    address_kind: u8,
    address: u16,
}

#[derive(Deserialize)]
struct Config {
    layout: String,
    sender_id: String,
    psu_id: String,
    session: u32,
    occupancy_threshold_ma: u16,
    locos: BTreeMap<String, Loco>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Throttle {
    direction: String,
    speed: u8,
    stop_mode: String,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Command {
    Throttle {
        request_id: String,
        target: Throttle,
    },
}

#[derive(Default)]
struct LocoState {
    target: Option<Throttle>,
    active: Option<Value>,
    sequence: u32,
    last_active: Option<Instant>,
}

enum Input {
    Mqtt(String, Vec<u8>, bool),
    Can(u32, Vec<u8>),
    Failed(String),
}

fn network_id(value: &str) -> Result<u32, String> {
    if value.len() != 6 || !value.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("network ID must be six hex digits: {value}"));
    }
    u32::from_str_radix(value, 16).map_err(|error| error.to_string())
}

fn topic_segment(value: &str) -> bool {
    !value.is_empty() && !value.contains(['/', '+', '#'])
}

fn parse_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

fn throttle_values(target: &Throttle) -> Result<(u8, u8), String> {
    let direction = match target.direction.as_str() {
        "reverse" => 0,
        "forward" => 1,
        _ => return Err("direction must be forward or reverse".into()),
    };
    let stop = match target.stop_mode.as_str() {
        "normal" => 0,
        "emergency" => 1,
        _ => return Err("stop_mode must be normal or emergency".into()),
    };
    if target.speed > 126 || (stop == 1 && target.speed != 0) {
        return Err("speed must be 0..126; emergency stop requires zero".into());
    }
    Ok((direction, stop))
}

fn occupancy(status: &DistrictStatus, threshold_ma: u16) -> &'static str {
    if status.state != 4 || !status.current_valid {
        "unknown"
    } else if status.average_ma >= threshold_ma {
        "occupied"
    } else {
        "clear"
    }
}

fn publish(client: &Client, topic: String, body: &Value, retain: bool) -> Result<(), String> {
    client
        .publish(topic, QoS::AtLeastOnce, retain, body.to_string())
        .map_err(|error| error.to_string())
}

fn publish_loco(client: &Client, layout: &str, id: &str, state: &LocoState) -> Result<(), String> {
    publish(
        client,
        format!("/{layout}/loco/{id}/throttle"),
        &json!({"target": state.target, "active": state.active}),
        true,
    )
}

fn run() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if !(3..=5).contains(&args.len()) {
        return Err(
            "usage: dispatchd <config.json> <sim-can-host:port> [mqtt-host] [mqtt-port]".into(),
        );
    }
    let config: Config =
        serde_json::from_slice(&std::fs::read(&args[1]).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    if !topic_segment(&config.layout) || config.locos.keys().any(|id| !topic_segment(id)) {
        return Err("layout and loco IDs must each be one MQTT topic segment".into());
    }
    for (id, loco) in &config.locos {
        if !valid_loco_address(loco.address_kind, loco.address) {
            return Err(format!("invalid DCC address for {id}"));
        }
    }
    if config.occupancy_threshold_ma == 0 {
        return Err("occupancy_threshold_ma must be positive".into());
    }
    let mut addresses = std::collections::BTreeSet::new();
    if config
        .locos
        .values()
        .any(|loco| !addresses.insert((loco.address_kind, loco.address)))
    {
        return Err("DCC addresses must be unique in the loco roster".into());
    }
    let sender_id = network_id(&config.sender_id)?;
    let psu_id = network_id(&config.psu_id)?;
    if sender_id == psu_id || config.session == 0 {
        return Err("sender and PSU IDs must differ; session must be nonzero".into());
    }
    let mut can = TcpStream::connect(&args[2]).map_err(|e| e.to_string())?;
    can.set_nodelay(true).map_err(|e| e.to_string())?;
    let can_read = can.try_clone().map_err(|e| e.to_string())?;
    let (tx, rx): (_, Receiver<Input>) = mpsc::channel();
    let can_tx = tx.clone();
    thread::spawn(move || {
        for line in BufReader::new(can_read).lines() {
            let Ok(line) = line else { break };
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let Some(id) = frame.get("id").and_then(Value::as_u64) else {
                continue;
            };
            let Some(data) = frame
                .get("data")
                .and_then(Value::as_str)
                .and_then(parse_hex)
            else {
                continue;
            };
            if id <= 0x1fff_ffff && can_tx.send(Input::Can(id as u32, data)).is_err() {
                return;
            }
        }
        let _ = can_tx.send(Input::Failed("sim CAN connection closed".into()));
    });
    let mqtt_host = args.get(3).map(String::as_str).unwrap_or("127.0.0.1");
    let mqtt_port = args
        .get(4)
        .map(|s| s.parse::<u16>())
        .transpose()
        .map_err(|e| e.to_string())?
        .unwrap_or(1883);
    let mut options = MqttOptions::new(
        format!("arc-dispatchd-{}", config.layout),
        mqtt_host,
        mqtt_port,
    );
    options.set_keep_alive(Duration::from_secs(5));
    let (client, mut connection) = Client::new(options, 32);
    let mqtt_tx = tx;
    thread::spawn(move || {
        for event in connection.iter() {
            let input = match event {
                Ok(Event::Incoming(Incoming::Publish(p))) => {
                    Some(Input::Mqtt(p.topic, p.payload.to_vec(), p.retain))
                }
                Err(error) => Some(Input::Failed(format!("MQTT: {error}"))),
                _ => None,
            };
            if let Some(input) = input
                && mqtt_tx.send(input).is_err()
            {
                return;
            }
        }
        let _ = mqtt_tx.send(Input::Failed("MQTT connection closed".into()));
    });
    client
        .subscribe(
            format!("/{}/loco/+/command", config.layout),
            QoS::AtLeastOnce,
        )
        .map_err(|e| e.to_string())?;
    let mut states: BTreeMap<String, LocoState> = config
        .locos
        .keys()
        .map(|id| (id.clone(), LocoState::default()))
        .collect();
    for (id, state) in &states {
        publish_loco(&client, &config.layout, id, state)?;
    }
    let mut last_district: BTreeMap<(u32, u8), Instant> = BTreeMap::new();
    loop {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(Input::Mqtt(topic, bytes, retained)) => {
                if retained {
                    continue;
                }
                let Some(id) = topic
                    .strip_prefix(&format!("/{}/loco/", config.layout))
                    .and_then(|s| s.strip_suffix("/command"))
                else {
                    continue;
                };
                let supplied_request_id = serde_json::from_slice::<Value>(&bytes)
                    .ok()
                    .and_then(|value| value.get("request_id")?.as_str().map(str::to_owned));
                let result = (|| -> Result<String, String> {
                    let loco = config.locos.get(id).ok_or("unknown locomotive")?;
                    let Command::Throttle { request_id, target } =
                        serde_json::from_slice(&bytes)
                            .map_err(|e| format!("invalid command: {e}"))?;
                    if request_id.is_empty() {
                        return Err("request_id is required".into());
                    }
                    let (direction, stop_mode) = throttle_values(&target)?;
                    let state = states.get_mut(id).ok_or("unknown locomotive")?;
                    let sequence = state.sequence.checked_add(1).ok_or("sequence exhausted")?;
                    let set = ThrottleSet {
                        target: psu_id,
                        session: config.session,
                        sequence,
                        address_kind: loco.address_kind,
                        address: loco.address,
                        direction,
                        speed: target.speed,
                        stop_mode,
                    };
                    let data = set.data().ok_or("invalid CAN throttle command")?;
                    writeln!(
                        can,
                        "{}",
                        json!({"id": ThrottleSet::can_id(sender_id).unwrap(),
                        "data": data.iter().map(|b| format!("{b:02x}")).collect::<String>()})
                    )
                    .map_err(|e| e.to_string())?;
                    state.sequence = sequence;
                    state.target = Some(target);
                    publish_loco(&client, &config.layout, id, state)?;
                    Ok(request_id)
                })();
                let (request_id, accepted, error) = match result {
                    Ok(id) => (Some(id), true, None),
                    Err(e) => (supplied_request_id, false, Some(e)),
                };
                publish(
                    &client,
                    format!("/{}/loco/{id}/result", config.layout),
                    &json!({"request_id": request_id, "accepted": accepted, "error": error}),
                    false,
                )?;
            }
            Ok(Input::Can(id, data)) => {
                if let Some(status) = DistrictStatus::decode(id, &data) {
                    let key = (status.network_id, status.district);
                    last_district.insert(key, Instant::now());
                    let occupancy = occupancy(&status, config.occupancy_threshold_ma);
                    publish(
                        &client,
                        format!(
                            "/{}/district/{:06x}-{}/status",
                            config.layout,
                            status.network_id,
                            status.district + 1
                        ),
                        &json!({
                            "occupancy": occupancy, "mode": status.mode, "state": status.state,
                            "current_valid": status.current_valid, "average_ma": status.average_ma,
                            "peak_ma": status.peak_ma, "window_ms": status.window_ms,
                            "protection_trip": status.tripped
                        }),
                        false,
                    )?;
                }
                if let Some(status) = ThrottleStatus::decode(id, &data)
                    && status.network_id == psu_id
                {
                    for (loco_id, loco) in &config.locos {
                        if loco.address_kind == status.address_kind
                            && loco.address == status.address
                        {
                            let state = states.get_mut(loco_id).unwrap();
                            state.active = Some(
                                json!({"direction": if status.direction == 1 {"forward"} else {"reverse"},
                                "speed": status.speed, "stop_mode": if status.stop_mode == 1 {"emergency"} else {"normal"},
                                "eligible": status.state == 1, "session": status.session,
                                "sequence": status.sequence,
                                "observed_at_ms": SystemTime::now().duration_since(UNIX_EPOCH)
                                    .map_err(|e| e.to_string())?.as_millis()}),
                            );
                            state.last_active = Some(Instant::now());
                            publish_loco(&client, &config.layout, loco_id, state)?;
                        }
                    }
                }
            }
            Ok(Input::Failed(error)) => return Err(error),
            Err(mpsc::RecvTimeoutError::Disconnected) => return Err("input closed".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        for (id, state) in &mut states {
            if state
                .last_active
                .is_some_and(|at| at.elapsed() > Duration::from_secs(3))
            {
                state.active = None;
                state.last_active = None;
                publish_loco(&client, &config.layout, id, state)?;
            }
        }
        let stale: Vec<_> = last_district
            .iter()
            .filter(|(_, at)| at.elapsed() > Duration::from_secs(3))
            .map(|(key, _)| *key)
            .collect();
        for (board, district) in stale {
            last_district.remove(&(board, district));
            publish(
                &client,
                format!(
                    "/{}/district/{board:06x}-{}/status",
                    config.layout,
                    district + 1
                ),
                &json!({"occupancy": "unknown", "reason": "status_timeout"}),
                false,
            )?;
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("dispatchd: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occupancy_requires_running_and_valid_current() {
        let mut status = DistrictStatus {
            network_id: 1,
            district: 0,
            mode: 1,
            state: 4,
            tripped: false,
            current_valid: true,
            average_ma: 60,
            peak_ma: 80,
            window_ms: 1000,
        };
        assert_eq!(occupancy(&status, 50), "occupied");
        status.average_ma = 40;
        assert_eq!(occupancy(&status, 50), "clear");
        status.current_valid = false;
        assert_eq!(occupancy(&status, 50), "unknown");
        status.current_valid = true;
        status.state = 3;
        assert_eq!(occupancy(&status, 50), "unknown");
    }
}
