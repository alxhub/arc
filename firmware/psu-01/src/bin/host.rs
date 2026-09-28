//! Host PSU with the same power controller, throttle table, and DCC packets as the board.

use link::{Presence, PsuStatus, ThrottleSet};
use psu_01::{
    dcc,
    locos::{Apply, CAPACITY, Table},
    power::{Controller, Reading, State},
};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

struct Options {
    uid: [u8; 12],
    bus: String,
    signals: String,
    control: String,
    name: String,
}

struct ControlRequest {
    command: Value,
    reply: Sender<Value>,
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn options() -> Result<Options, String> {
    let mut uid = None;
    let mut bus = "127.0.0.1:17500".to_string();
    let mut signals = "127.0.0.1:17501".to_string();
    let mut control = "127.0.0.1:17610".to_string();
    let mut name = "psu-01-host".to_string();
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() % 2 != 0 {
        return Err("options require values".into());
    }
    for pair in args.chunks_exact(2) {
        match pair[0].as_str() {
            "--uid" => {
                if pair[1].len() != 24 {
                    return Err("UID must be 24 hex digits".into());
                }
                let bytes = pair[1]
                    .as_bytes()
                    .chunks_exact(2)
                    .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16))
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|error| error.to_string())?;
                uid = Some(bytes.try_into().map_err(|_| "UID must be 24 hex digits")?);
            }
            "--bus" => bus = pair[1].clone(),
            "--signals" => signals = pair[1].clone(),
            "--control" => control = pair[1].clone(),
            "--name" => name = pair[1].clone(),
            _ => return Err(format!("unknown option: {}", pair[0])),
        }
    }
    Ok(Options {
        uid: uid.ok_or("--uid required")?,
        bus,
        signals,
        control,
        name,
    })
}

fn control_server(address: &str) -> Result<Receiver<ControlRequest>, String> {
    let listener = TcpListener::bind(address).map_err(|error| error.to_string())?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(mut stream) = connection else { break };
            let Ok(read_stream) = stream.try_clone() else {
                continue;
            };
            let mut line = String::new();
            if BufReader::new(read_stream).read_line(&mut line).is_err() || line.len() > 4096 {
                continue;
            }
            let command = match serde_json::from_str::<Value>(&line) {
                Ok(value) => value,
                Err(error) => {
                    let _ = writeln!(
                        stream,
                        "{}",
                        json!({"ok": false, "error": error.to_string()})
                    );
                    continue;
                }
            };
            let (reply, answer) = mpsc::channel();
            if sender.send(ControlRequest { command, reply }).is_err() {
                break;
            }
            if let Ok(response) = answer.recv_timeout(Duration::from_secs(2)) {
                let _ = writeln!(stream, "{response}");
            }
        }
    });
    Ok(receiver)
}

fn signal_packet(packet: dcc::Packet) -> Value {
    let bits: String = (0..packet.len_bits())
        .map(|index| if packet.bit(index) { '1' } else { '0' })
        .collect();
    let half_us: Vec<u16> = (0..packet.len_bits())
        .map(|index| {
            if packet.bit(index) {
                dcc::ONE_HALF_US
            } else {
                dcc::ZERO_HALF_US
            }
        })
        .collect();
    json!({"kind": "dcc_packet", "bits": bits, "half_us": half_us})
}

fn can_receiver(stream: TcpStream) -> Receiver<(u32, Vec<u8>)> {
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for line in BufReader::new(stream).lines() {
            let Ok(line) = line else { break };
            let Ok(frame) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let (Some(id), Some(data)) = (
                frame.get("id").and_then(Value::as_u64),
                frame.get("data").and_then(Value::as_str),
            ) else {
                continue;
            };
            if id > 0x1fff_ffff || data.len() % 2 != 0 {
                continue;
            }
            let bytes = data
                .as_bytes()
                .chunks_exact(2)
                .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
                .collect::<Option<Vec<u8>>>();
            if let Some(bytes) = bytes
                && sender.send((id as u32, bytes)).is_err()
            {
                break;
            }
        }
    });
    receiver
}

fn send_status(can: &mut TcpStream, status: link::ThrottleStatus) -> Result<(), String> {
    writeln!(
        can,
        "{}",
        json!({"id": status.can_id().unwrap(), "data": hex_bytes(&status.data().unwrap())})
    )
    .map_err(|error| error.to_string())
}

fn state_code(state: State) -> u8 {
    match state {
        State::Off => 0,
        State::Starting => 1,
        State::Online => 2,
        State::Fault => 3,
    }
}

fn run() -> Result<(), String> {
    let opts = options()?;
    let presence = Presence::new(opts.uid);
    let mut can = TcpStream::connect(&opts.bus).map_err(|error| error.to_string())?;
    let mut signals = TcpStream::connect(&opts.signals).map_err(|error| error.to_string())?;
    can.set_nodelay(true).map_err(|error| error.to_string())?;
    let can_rx = can_receiver(can.try_clone().map_err(|error| error.to_string())?);
    signals
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;
    let control = control_server(&opts.control)?;
    let mut power = Controller::new();
    let mut locos = Table::new();
    let mut voltage_mv = 15_000u16;
    let mut current_ma = 250i16;
    let mut monitor_available = true;
    let start = Instant::now();
    let mut next_presence = start;
    let mut next_status = start;
    let mut next_packet = start;
    let mut last_state = State::Off;
    let mut last_power = None;
    let packet_us = dcc::Packet::idle().duration_us();
    println!("{}: PSU-01 id={:06x}", opts.name, presence.network_id);
    loop {
        while let Ok((id, data)) = can_rx.try_recv() {
            if power.dcc_enabled()
                && let Some(command) = ThrottleSet::decode(id, &data)
            {
                match locos.apply(command, presence.network_id) {
                    Apply::Changed | Apply::Repeated => {
                        for index in 0..CAPACITY {
                            if let Some(status) = locos.status_due(
                                index,
                                start.elapsed().as_millis() as u64,
                                presence.network_id,
                                power.dcc_enabled(),
                            ) {
                                send_status(&mut can, status)?;
                                locos.status_sent(
                                    index,
                                    status.sequence,
                                    start.elapsed().as_millis() as u64,
                                );
                            }
                        }
                    }
                    Apply::Rejected | Apply::Full => {}
                }
            }
        }
        while let Ok(request) = control.try_recv() {
            let result = match request.command.get("op").and_then(Value::as_str) {
                Some("status") => json!({
                    "ok": true, "network_id": format!("{:06x}", presence.network_id),
                    "state": format!("{:?}", power.state()).to_lowercase(),
                    "link_enabled": power.link_enabled(), "dcc_enabled": power.dcc_enabled(),
                    "voltage_mv": voltage_mv, "current_ma": current_ma,
                    "monitor_available": monitor_available, "packet_us": packet_us,
                    "loco_count": locos.len(),
                }),
                Some("set_power") => match (
                    request.command.get("voltage_mv").and_then(Value::as_u64),
                    request.command.get("current_ma").and_then(Value::as_i64),
                    request
                        .command
                        .get("monitor_available")
                        .and_then(Value::as_bool),
                ) {
                    (Some(v @ 0..=65535), Some(c @ -32768..=32767), Some(m)) => {
                        voltage_mv = v as u16;
                        current_ma = c as i16;
                        monitor_available = m;
                        json!({"ok": true})
                    }
                    _ => {
                        json!({"ok": false, "error": "set_power requires voltage_mv, current_ma, monitor_available"})
                    }
                },
                _ => json!({"ok": false, "error": "unknown control operation"}),
            };
            let _ = request.reply.send(result);
        }
        let now = Instant::now();
        let elapsed = start.elapsed().as_millis() as u64;
        if power.state() == State::Off {
            if monitor_available {
                power.start(elapsed);
            } else {
                power.fail();
            }
        }
        if power.link_enabled() {
            power.sample(
                elapsed,
                monitor_available.then_some(Reading {
                    millivolts: voltage_mv,
                    milliamps: current_ma,
                }),
            );
        }
        let enabled = power.link_enabled();
        let output = (
            enabled,
            if enabled { voltage_mv } else { 0 },
            if enabled { current_ma } else { 0 },
        );
        if last_power != Some(output) {
            writeln!(
                signals,
                "{}",
                json!({"kind": "power", "enabled": enabled,
                "voltage_mv": output.1, "current_ma": output.2})
            )
            .map_err(|error| error.to_string())?;
            last_power = Some(output);
        }
        if power.dcc_enabled() && now >= next_packet {
            let packet = locos.next_packet();
            writeln!(signals, "{}", signal_packet(packet)).map_err(|error| error.to_string())?;
            next_packet = now + Duration::from_micros(packet.duration_us());
        }
        if power.state() == State::Online {
            if now >= next_presence {
                writeln!(
                    can,
                    "{}",
                    json!({"id": presence.can_id(), "data": hex_bytes(&presence.data())})
                )
                .map_err(|error| error.to_string())?;
                next_presence = now + Duration::from_secs(5);
            }
            if power.state() != last_state || now >= next_status {
                let status = PsuStatus {
                    network_id: presence.network_id,
                    state: state_code(power.state()),
                    dcc_enabled: true,
                    monitor_valid: true,
                    link_mv: voltage_mv,
                    link_ma: current_ma.max(0) as u16,
                    uptime_ms: elapsed.min(u64::from(u32::MAX)) as u32,
                };
                writeln!(
                    can,
                    "{}",
                    json!({"id": status.can_id().unwrap(),
                    "data": hex_bytes(&status.data().unwrap())})
                )
                .map_err(|error| error.to_string())?;
                if now >= next_status {
                    next_status = now + Duration::from_secs(1);
                }
            }
            for index in 0..CAPACITY {
                if let Some(status) = locos.status_due(index, elapsed, presence.network_id, true) {
                    send_status(&mut can, status)?;
                    locos.status_sent(index, status.sequence, elapsed);
                }
            }
        }
        if power.state() != last_state {
            println!("{}: {:?} -> {:?}", opts.name, last_state, power.state());
            last_state = power.state();
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("psu-01-host: {error}");
        std::process::exit(1);
    }
}
