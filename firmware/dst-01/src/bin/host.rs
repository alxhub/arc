//! Host build of the DST-01x4 firmware core, with a simulated CAN/HAL boundary.

#[cfg(not(any(feature = "rev1", feature = "rev2")))]
compile_error!("select exactly one hardware revision: rev1 or rev2");
#[cfg(all(feature = "rev1", feature = "rev2"))]
compile_error!("rev1 and rev2 are mutually exclusive");

use dst_01::district::{Config, Controller, Drive, Mode, State};
use dst_01::host::Board;
use dst_01::network::{self, StatusCadence};
use link::{MAX_NETWORK_ID, Presence};
use psu_01::dcc;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

struct Options {
    uid: [u8; 12],
    bus: String,
    signals: String,
    name: String,
    source_ready: bool,
    fault_mask: u8,
    network_id: Option<u32>,
    control: Option<String>,
}

struct ControlRequest {
    command: Value,
    reply: Sender<Value>,
}

#[derive(Default)]
struct LinkSignal {
    power: bool,
    last_packet: Option<Instant>,
    packets: u64,
}

fn valid_dcc_packet(value: &Value) -> bool {
    let Some(bits) = value.get("bits").and_then(Value::as_str) else {
        return false;
    };
    let Some(halves) = value.get("half_us").and_then(Value::as_array) else {
        return false;
    };
    dcc::valid_packet_bits(bits.as_bytes())
        && halves.len() == bits.len()
        && bits
            .bytes()
            .enumerate()
            .all(|(_, bit)| bit == b'1' || bit == b'0')
        && halves.iter().enumerate().all(|(index, half)| {
            half.as_u64()
                == Some(u64::from(if bits.as_bytes()[index] == b'1' {
                    dcc::ONE_HALF_US
                } else {
                    dcc::ZERO_HALF_US
                }))
        })
}

fn observe_signals(reader: TcpStream, shared: Arc<Mutex<LinkSignal>>) {
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let mut signal = shared.lock().expect("signal lock");
        match value.get("kind").and_then(Value::as_str) {
            Some("power") => {
                if let Some(enabled) = value.get("enabled").and_then(Value::as_bool) {
                    signal.power = enabled;
                    if !enabled {
                        signal.last_packet = None;
                    }
                }
            }
            Some("dcc_packet") if valid_dcc_packet(&value) => {
                if signal.power {
                    signal.last_packet = Some(Instant::now());
                    signal.packets += 1;
                }
            }
            _ => {}
        }
    }
    let mut signal = shared.lock().expect("signal lock");
    signal.power = false;
    signal.last_packet = None;
}

#[derive(Default)]
struct CurrentWindow {
    start_ms: u64,
    last_ms: u64,
    sum_ma_ms: u64,
    peak_ma: u16,
}

impl CurrentWindow {
    /// A sample describes the current delivered since the preceding tick.
    fn sample(&mut self, now_ms: u64, current_ma: u16) {
        let elapsed = now_ms.saturating_sub(self.last_ms);
        self.sum_ma_ms = self
            .sum_ma_ms
            .saturating_add(u64::from(current_ma) * elapsed);
        self.peak_ma = self.peak_ma.max(current_ma);
        self.last_ms = now_ms;
    }

    fn finish(&mut self, now_ms: u64) -> Option<(u16, u16, u32)> {
        let window_ms = now_ms.saturating_sub(self.start_ms);
        // A zero-length window contains no measurement. Keep a pending state
        // change for the next tick instead of inventing a window size.
        if window_ms == 0 {
            return None;
        }
        let average = (self.sum_ma_ms / window_ms).min(u64::from(u16::MAX)) as u16;
        let peak = self.peak_ma;
        self.start_ms = now_ms;
        self.sum_ma_ms = 0;
        self.peak_ma = 0;
        Some((average, peak, window_ms.min(u64::from(u32::MAX)) as u32))
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}

fn parse_hex_bytes(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("hexadecimal bytes need an even number of digits".into());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&value[index..index + 2], 16).map_err(|error| error.to_string())
        })
        .collect()
}

fn options() -> Result<Options, String> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() < 2 || args.len() % 2 != 0 {
        return Err("usage: dst-01-host --uid <24-hex> [--bus host:port] [--control host:port] [--name name] [--source-ready true|false] [--fault-mask 0..15] [--network-id 6-hex]".into());
    }
    let mut uid = None;
    let mut bus = "127.0.0.1:17500".to_string();
    let mut signals = "127.0.0.1:17501".to_string();
    let mut name = "dst-01-host".to_string();
    let mut source_ready = false;
    let mut fault_mask = 0;
    let mut network_id = None;
    let mut control = None;
    for pair in args.chunks_exact(2) {
        match pair[0].as_str() {
            "--uid" => {
                let bytes = parse_hex_bytes(&pair[1])?;
                uid = Some(bytes.try_into().map_err(|_| "UID must contain 12 bytes")?);
            }
            "--bus" => bus = pair[1].clone(),
            "--signals" => signals = pair[1].clone(),
            "--control" => control = Some(pair[1].clone()),
            "--name" => name = pair[1].clone(),
            "--source-ready" => {
                source_ready = pair[1]
                    .parse()
                    .map_err(|_| "source-ready must be true or false")?
            }
            "--fault-mask" => {
                fault_mask = pair[1].parse().map_err(|_| "fault-mask must be 0..15")?;
                if fault_mask > 15 {
                    return Err("fault-mask must be 0..15".into());
                }
            }
            "--network-id" => {
                let id = u32::from_str_radix(&pair[1], 16).map_err(|error| error.to_string())?;
                if id > MAX_NETWORK_ID {
                    return Err("network-id must be 24 bits".into());
                }
                network_id = Some(id);
            }
            _ => return Err(format!("unknown option: {}", pair[0])),
        }
    }
    Ok(Options {
        uid: uid.ok_or("--uid is required")?,
        bus,
        signals,
        name,
        source_ready,
        fault_mask,
        network_id,
        control,
    })
}

fn observe_peer(
    name: &str,
    own: Presence,
    reader: TcpStream,
    peers: Arc<AtomicUsize>,
    configs: Sender<(usize, Config)>,
) {
    let mut seen = HashSet::new();
    for line in BufReader::new(reader).lines() {
        let Ok(line) = line else { break };
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = value.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Some(data) = value.get("data").and_then(Value::as_str) else {
            continue;
        };
        let Ok(bytes) = parse_hex_bytes(data) else {
            continue;
        };
        if let Some(config) = network::decode_command(id as u32, &bytes, own.network_id) {
            let _ = configs.send(config);
            continue;
        }
        let Some(peer) = Presence::decode(id as u32, &bytes) else {
            continue;
        };
        if peer.uid == own.uid || !seen.insert((peer.network_id, peer.uid)) {
            continue;
        }
        peers.fetch_add(1, Ordering::Relaxed);
        if peer.network_id == own.network_id {
            println!(
                "{name}: COLLISION id={:06x} peer_uid={}",
                own.network_id,
                hex_bytes(&peer.uid)
            );
        } else {
            println!(
                "{name}: peer id={:06x} uid={}",
                peer.network_id,
                hex_bytes(&peer.uid)
            );
        }
    }
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
                Ok(command) => command,
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

fn state_name(state: State) -> &'static str {
    match state {
        State::Disabled => "disabled",
        State::WaitingForSource => "waiting_for_source",
        State::Probing => "probing",
        State::CoolingDown => "cooling_down",
        State::Running => "running",
        State::HardwareFault => "hardware_fault",
    }
}

fn mode_name(mode: Mode) -> &'static str {
    match mode {
        Mode::Disabled => "disabled",
        Mode::Synced => "synced",
        Mode::Programming => "programming",
    }
}

fn respond_to_control(
    request: ControlRequest,
    board: &mut Board,
    districts: &[Controller; 4],
    source_ready: &mut bool,
    link_power: bool,
    dcc_packets: u64,
    dcc_valid: bool,
    trips: &[u32; 4],
    peers: usize,
    name: &str,
    revision: &str,
    network_id: u32,
) {
    let command = &request.command;
    let result = match command.get("op").and_then(Value::as_str) {
        Some("status") => json!({
            "ok": true,
            "name": name,
            "revision": revision,
            "network_id": format!("{network_id:06x}"),
            "peers": peers,
            "source_ready": *source_ready && link_power && dcc_valid,
            "link_power": link_power,
            "dcc_packets": dcc_packets,
            "districts": (0..4).map(|index| {
                let status = districts[index].status();
                json!({
                    "state": state_name(status.state),
                    "mode": mode_name(status.config.mode),
                    "enabled": status.drive.enabled,
                    "gate": board.gate(index),
                    "shorted": board.is_shorted(index),
                    "current_ma": board.current_ma(index),
                    "trips": trips[index],
                    "next_off_ms": status.next_off_ms,
                    "clear_probes": status.clear_probes,
                    "protection_trip": status.tripped,
                })
            }).collect::<Vec<_>>()
        }),
        Some("set_short") => {
            match (
                command.get("district").and_then(Value::as_u64),
                command.get("value").and_then(Value::as_bool),
            ) {
                (Some(index @ 0..=3), Some(value)) => {
                    board.inject_short(index as usize, value);
                    json!({"ok": true})
                }
                _ => {
                    json!({"ok": false, "error": "set_short requires district 0..3 and boolean value"})
                }
            }
        }
        Some("set_source_ready") => match command.get("value").and_then(Value::as_bool) {
            Some(value) => {
                *source_ready = value;
                json!({"ok": true})
            }
            None => json!({"ok": false, "error": "set_source_ready requires boolean value"}),
        },
        _ => json!({"ok": false, "error": "unknown control operation"}),
    };
    let _ = request.reply.send(result);
}

fn run() -> Result<(), String> {
    let opts = options()?;
    let mut presence = Presence::new(opts.uid);
    if let Some(id) = opts.network_id {
        presence.network_id = id;
    }
    let mut stream = TcpStream::connect(&opts.bus).map_err(|error| error.to_string())?;
    stream
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;
    let receive = stream.try_clone().map_err(|error| error.to_string())?;
    let signal_stream = TcpStream::connect(&opts.signals).map_err(|error| error.to_string())?;
    let link_signal = Arc::new(Mutex::new(LinkSignal::default()));
    let signal_shared = Arc::clone(&link_signal);
    thread::spawn(move || observe_signals(signal_stream, signal_shared));
    let peers = Arc::new(AtomicUsize::new(0));
    let name = opts.name.clone();
    let peer_counter = Arc::clone(&peers);
    let (config_tx, config_rx) = mpsc::channel();
    thread::spawn(move || observe_peer(&name, presence, receive, peer_counter, config_tx));
    let control = opts.control.as_deref().map(control_server).transpose()?;

    let revision = if cfg!(feature = "rev1") {
        "rev1"
    } else {
        "rev2"
    };
    println!(
        "{}: DST-01x4 {revision} online id={:06x} uid={}",
        opts.name,
        presence.network_id,
        hex_bytes(&opts.uid)
    );

    let mut districts: [Controller; 4] = std::array::from_fn(|_| Controller::new());
    let mut board = Board::new();
    for index in 0..4 {
        board.inject_short(index, opts.fault_mask & (1 << index) != 0);
    }
    let mut previous = std::array::from_fn::<_, 4, _>(|index| districts[index].status().state);
    let mut cadence = [StatusCadence::new(); 4];
    let mut trips = [0u32; 4];
    let mut current_windows: [CurrentWindow; 4] = std::array::from_fn(|_| CurrentWindow::default());
    let mut source_ready = opts.source_ready;
    let mut applied = [Drive {
        mode: Mode::Disabled,
        enabled: false,
    }; 4];
    let start = Instant::now();
    let mut next_presence = Instant::now();
    loop {
        while let Ok((index, config)) = config_rx.try_recv() {
            if districts[index]
                .configure(config, start.elapsed().as_millis() as u64)
                .is_err()
            {
                eprintln!(
                    "{}: rejected invalid district {} configuration",
                    opts.name, index
                );
            }
        }
        if let Some(control) = &control {
            while let Ok(request) = control.try_recv() {
                let signal = link_signal.lock().expect("signal lock");
                let dcc_valid = signal
                    .last_packet
                    .is_some_and(|last| last.elapsed() < Duration::from_millis(30));
                respond_to_control(
                    request,
                    &mut board,
                    &districts,
                    &mut source_ready,
                    signal.power,
                    signal.packets,
                    dcc_valid,
                    &trips,
                    peers.load(Ordering::Relaxed),
                    &opts.name,
                    revision,
                    presence.network_id,
                );
            }
        }
        let now = Instant::now();
        if now >= next_presence {
            let frame = json!({"id": presence.can_id(), "data": hex_bytes(&presence.data())});
            writeln!(stream, "{frame}").map_err(|error| error.to_string())?;
            next_presence = now + Duration::from_secs(5);
        }
        let elapsed = start.elapsed().as_millis() as u64;
        let signal = link_signal.lock().expect("signal lock");
        let source_ready = source_ready
            && signal.power
            && signal
                .last_packet
                .is_some_and(|last| last.elapsed() < Duration::from_millis(30));
        drop(signal);
        for (index, district) in districts.iter_mut().enumerate() {
            if !source_ready {
                board.inhibit(index);
            }
            current_windows[index].sample(elapsed, board.current_ma(index));
            let sample = board.sample(index);
            let before = district.status().state;
            let drive = district.tick(elapsed, source_ready, sample);
            if drive != applied[index] {
                board.apply(index, drive);
                applied[index] = drive;
            }
            let state = district.status().state;
            if state == State::CoolingDown
                && matches!(before, State::Probing | State::Running)
                && (sample.fault || sample.overcurrent)
            {
                trips[index] += 1;
            }
            if state != previous[index] {
                println!("{}: district {} -> {state:?}", opts.name, index);
                previous[index] = state;
            }
            let status = district.status();
            if cadence[index].due(elapsed, status)
                && let Some((average_ma, peak_ma, window_ms)) =
                    current_windows[index].finish(elapsed)
            {
                let frame = network::status_frame(
                    presence.network_id,
                    index,
                    status,
                    Some((average_ma, peak_ma)),
                    window_ms,
                );
                let data = frame.data().expect("valid district status");
                let can_id = frame.can_id().expect("valid network ID");
                writeln!(
                    stream,
                    "{}",
                    json!({"id": can_id, "data": hex_bytes(&data)})
                )
                .map_err(|error| error.to_string())?;
                cadence[index].sent(elapsed, status);
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("dst-01-host: {error}");
        std::process::exit(1);
    }
}
