//! Host build of the DST-01x4 firmware core, with a simulated CAN/HAL boundary.

#[cfg(not(any(feature = "rev1", feature = "rev2")))]
compile_error!("select exactly one hardware revision: rev1 or rev2");
#[cfg(all(feature = "rev1", feature = "rev2"))]
compile_error!("rev1 and rev2 are mutually exclusive");

use dst_01::district::{Config, Controller, Drive, Mode};
use dst_01::host::Board;
use link::{MAX_NETWORK_ID, Presence};
use serde_json::{Value, json};
use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

struct Options {
    uid: [u8; 12],
    bus: String,
    name: String,
    mode: Mode,
    source_ready: bool,
    fault_mask: u8,
    network_id: Option<u32>,
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
        return Err("usage: dst-01-host --uid <24-hex> [--bus host:port] [--name name] [--mode disabled|synced|programming] [--source-ready true|false] [--fault-mask 0..15] [--network-id 6-hex]".into());
    }
    let mut uid = None;
    let mut bus = "127.0.0.1:17500".to_string();
    let mut name = "dst-01-host".to_string();
    let mut mode = Mode::Disabled;
    let mut source_ready = false;
    let mut fault_mask = 0;
    let mut network_id = None;
    for pair in args.chunks_exact(2) {
        match pair[0].as_str() {
            "--uid" => {
                let bytes = parse_hex_bytes(&pair[1])?;
                uid = Some(bytes.try_into().map_err(|_| "UID must contain 12 bytes")?);
            }
            "--bus" => bus = pair[1].clone(),
            "--name" => name = pair[1].clone(),
            "--mode" => {
                mode = match pair[1].as_str() {
                    "disabled" => Mode::Disabled,
                    "synced" => Mode::Synced,
                    "programming" => Mode::Programming,
                    _ => return Err("invalid district mode".into()),
                }
            }
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
        name,
        mode,
        source_ready,
        fault_mask,
        network_id,
    })
}

fn observe_peer(name: &str, own: Presence, reader: TcpStream) {
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
        let Some(peer) = Presence::decode(id as u32, &bytes) else {
            continue;
        };
        if peer.uid == own.uid || !seen.insert((peer.network_id, peer.uid)) {
            continue;
        }
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
    let name = opts.name.clone();
    thread::spawn(move || observe_peer(&name, presence, receive));

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
    let config = Config {
        mode: opts.mode,
        ..Config::default()
    };
    for district in &mut districts {
        district
            .configure(config, 0)
            .map_err(|_| "invalid district config")?;
    }
    let mut previous = std::array::from_fn::<_, 4, _>(|index| districts[index].status().state);
    let mut applied = [Drive {
        mode: Mode::Disabled,
        enabled: false,
    }; 4];
    let start = Instant::now();
    let mut next_presence = Instant::now();
    loop {
        let now = Instant::now();
        if now >= next_presence {
            let frame = json!({"id": presence.can_id(), "data": hex_bytes(&presence.data())});
            writeln!(stream, "{frame}").map_err(|error| error.to_string())?;
            next_presence = now + Duration::from_secs(5);
        }
        let elapsed = start.elapsed().as_millis() as u64;
        for (index, district) in districts.iter_mut().enumerate() {
            if !opts.source_ready {
                board.inhibit(index);
            }
            let sample = board.sample(index);
            let drive = district.tick(elapsed, opts.source_ready, sample);
            if drive != applied[index] {
                board.apply(index, drive);
                applied[index] = drive;
            }
            let state = district.status().state;
            if state != previous[index] {
                println!("{}: district {} -> {state:?}", opts.name, index);
                previous[index] = state;
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
