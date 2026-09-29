//! CAN lab TCP backend: observe real firmware frames and send targeted grants.
use crate::{CanNetwork, DccObservation, NodeObservation};
use link::{DccGrant, DccStatus, Presence};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Write},
    net::TcpStream,
    sync::{Arc, atomic::{AtomicU64, Ordering}},
    sync::mpsc::{self, Receiver},
    thread,
    time::Instant,
};

pub const SENDER_ID: u32 = 1;

pub fn grant_frame(target: u32) -> Result<Value, String> {
    let data = DccGrant { target }.data().ok_or("invalid grant target")?;
    Ok(
        json!({"id": DccGrant::can_id(SENDER_ID).unwrap(), "data": data.iter().map(|b| format!("{b:02x}")).collect::<String>()}),
    )
}

pub struct TcpCan {
    stream: TcpStream,
    disconnected: bool,
    incoming: Receiver<Result<(u32, Vec<u8>), String>>,
    nodes: BTreeMap<(u32, [u8; 12]), u64>,
    sources: BTreeMap<u32, (DccObservation, u64)>,
    started: Instant,
    now_ms: u64,
    clocked: bool,
    marker: Arc<AtomicU64>,
}

impl TcpCan {
    pub fn connect(address: &str, clocked: bool) -> Result<Self, String> {
        let stream = TcpStream::connect(address).map_err(|e| e.to_string())?;
        stream.set_nodelay(true).map_err(|e| e.to_string())?;
        let reader = stream.try_clone().map_err(|e| e.to_string())?;
        let (tx, incoming) = mpsc::channel();
        let marker = Arc::new(AtomicU64::new(0));
        let receive_marker = Arc::clone(&marker);
        thread::spawn(move || {
            for line in BufReader::new(reader).lines() {
                let Ok(line) = line else {
                    break;
                };
                if line.len() > 512 {
                    continue;
                }
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if value["kind"].as_str() == Some("can_tick") {
                        if let Some(now) = value["time_ms"].as_u64() {
                            receive_marker.store(now, Ordering::Release);
                        }
                        continue;
                    }
                }
                let Some(frame) = decode_frame(&line) else {
                    continue;
                };
                if tx.send(Ok(frame)).is_err() {
                    return;
                }
            }
            let _ = tx.send(Err("virtual CAN connection closed".into()));
        });
        Ok(Self {
            stream,
            disconnected: false,
            incoming,
            nodes: BTreeMap::new(),
            sources: BTreeMap::new(),
            started: Instant::now(),
            now_ms: 0,
            clocked,
            marker,
        })
    }

    pub fn set_time(&mut self, now_ms: u64) {
        self.now_ms = now_ms;
    }

    pub fn wait_marker(&self, now_ms: u64) {
        if now_ms % 5 == 0 {
            while self.marker.load(Ordering::Acquire) < now_ms {
                thread::yield_now();
            }
        }
    }
}

fn decode_frame(line: &str) -> Option<(u32, Vec<u8>)> {
    let value: Value = serde_json::from_str(line).ok()?;
    let id = value.get("id")?.as_u64()?;
    let data = value.get("data")?.as_str()?;
    if id > 0x1fff_ffff || !data.len().is_multiple_of(2) {
        return None;
    }
    let bytes = data
        .as_bytes()
        .chunks_exact(2)
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).ok()?, 16).ok())
        .collect::<Option<Vec<_>>>()?;
    Some((id as u32, bytes))
}

impl CanNetwork for TcpCan {
    fn observe(&mut self) -> Result<Vec<NodeObservation>, String> {
        if self.disconnected {
            return Err("virtual CAN connection closed".into());
        }
        let now_ms = if self.clocked { self.now_ms } else { self.started.elapsed().as_millis() as u64 };
        while let Ok(frame) = self.incoming.try_recv() {
            let (id, data) = match frame {
                Ok(frame) => frame,
                Err(error) => {
                    self.disconnected = true;
                    return Err(error);
                }
            };
            if let Some(presence) = Presence::decode(id, &data) {
                self.nodes
                    .insert((presence.network_id, presence.uid), now_ms);
            }
            if let Some(status) = DccStatus::decode(id, &data) {
                self.sources.insert(
                    status.network_id,
                    (
                        DccObservation {
                            kind: status.kind,
                            permitted: status.permitted,
                            transmitting: status.transmitting,
                        },
                        now_ms,
                    ),
                );
            }
        }
        self.nodes
            .retain(|_, seen| now_ms.saturating_sub(*seen) < 15_000);
        self.sources
            .retain(|_, (_, seen)| now_ms.saturating_sub(*seen) < 3_000);
        Ok(self
            .nodes
            .keys()
            .map(|(id, _)| NodeObservation {
                id: format!("{id:06x}"),
                dcc: self.sources.get(id).map(|(source, _)| *source),
            })
            .collect())
    }
    fn grant_dcc(&mut self, target: u32) -> Result<(), String> {
        writeln!(self.stream, "{}", grant_frame(target)?).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn presence_expires_on_logical_time_only() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let presence = Presence::new(*b"abcdefghijkl");
            let data = presence.data().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
            writeln!(stream, "{}", json!({"id": presence.can_id(), "data": data})).unwrap();
            writeln!(stream, "{}", json!({"kind": "can_tick", "time_ms": 5})).unwrap();
            done_rx.recv().unwrap();
        });
        let mut can = TcpCan::connect(&address.to_string(), true).unwrap();
        can.wait_marker(5);
        can.set_time(5);
        assert_eq!(can.observe().unwrap().len(), 1);
        can.set_time(15_004);
        assert_eq!(can.observe().unwrap().len(), 1);
        can.set_time(15_005);
        assert!(can.observe().unwrap().is_empty());
        done_tx.send(()).unwrap();
        server.join().unwrap();
    }
}
