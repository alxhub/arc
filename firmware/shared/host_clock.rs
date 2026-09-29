//! Logical clock client for simulator host binaries.
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

pub struct HostClock {
    write: TcpStream,
    read: BufReader<TcpStream>,
    name: String,
    now_ms: u64,
}

impl HostClock {
    pub fn connect(address: &str, name: &str) -> Result<Self, String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        let write = loop {
            match TcpStream::connect(address) {
                Ok(stream) => break stream,
                Err(error) if Instant::now() >= deadline => return Err(format!("clock connection: {error}")),
                Err(_) => thread::sleep(Duration::from_millis(50)),
            }
        };
        write.set_nodelay(true).map_err(|error| error.to_string())?;
        let read = BufReader::new(write.try_clone().map_err(|error| error.to_string())?);
        let mut clock = Self { write, read, name: name.to_string(), now_ms: 0 };
        clock.now_ms = clock.request(json!({"op": "clock_register", "name": name}))?["time_ms"]
            .as_u64().ok_or("invalid clock registration")?;
        Ok(clock)
    }

    fn request(&mut self, command: Value) -> Result<Value, String> {
        writeln!(self.write, "{command}").map_err(|error| error.to_string())?;
        let mut line = String::new();
        if self.read.read_line(&mut line).map_err(|error| error.to_string())? == 0 {
            return Err("clock connection closed".into());
        }
        let reply: Value = serde_json::from_str(&line).map_err(|error| error.to_string())?;
        if reply["ok"].as_bool() != Some(true) {
            return Err(format!("clock rejected {command}: {reply}"));
        }
        Ok(reply)
    }

    pub fn wait(&mut self) -> Result<Option<u64>, String> {
        let reply = self.request(json!({"op": "clock_wait", "name": self.name, "after_ms": self.now_ms}))?;
        if reply["pending"].as_bool() == Some(true) {
            return Ok(None);
        }
        let now = reply["time_ms"].as_u64().ok_or("invalid clock tick")?;
        if now != self.now_ms + 1 {
            return Err(format!("clock skipped tick {} to {now}", self.now_ms));
        }
        self.now_ms = now;
        Ok(Some(now))
    }

    #[allow(dead_code)]
    pub fn now_ms(&self) -> u64 {
        self.now_ms
    }

    pub fn ack(&mut self) -> Result<(), String> {
        self.request(json!({"op": "clock_ack", "name": self.name, "time_ms": self.now_ms}))?;
        Ok(())
    }
}
