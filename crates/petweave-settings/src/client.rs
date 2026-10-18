//! Control-channel client used by the settings GUI (`docs/GUI_ROADMAP.md` §6).
//!
//! The GUI must keep working when the host is not running (F33) and must not
//! block its frame on a stuck socket (N03), so the client is **non-blocking**:
//! [`Client::tick`] is called every frame, drains whatever the host sent and
//! retries the connection every [`RECONNECT_INTERVAL`].
//!
//! Protocol types are shared with the host
//! (`petweave::platform::control`), so a protocol change cannot drift between
//! the two binaries. Only the host's client-facing state is stored here;
//! config edits stay in [`crate::store`].

use std::collections::HashMap;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use petweave::platform::control::{
    ControlEvent, Op, OutputInfo, PetInfo, Request, Response, PROTOCOL,
};
use serde_json::Value;

/// How often a disconnected GUI retries the control socket.
pub const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);

/// A protocol-level line larger than this is treated as a broken stream
/// (the host caps requests at 64 KiB; responses may be a bit larger because
/// they carry N pets).
const MAX_INCOMING: usize = 512 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionState {
    /// No socket (host not running, or the connection dropped).
    Disconnected,
    /// Socket connected, waiting for the `Hello` answer.
    Connecting,
    /// `Hello` succeeded; requests/events flow.
    Connected,
    /// Host speaks another protocol version: config editing only.
    VersionMismatch(String),
}

impl ConnectionState {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusSnapshot {
    pub fps: u32,
    pub rss_kb: u64,
    pub pets: usize,
}

#[derive(Debug, Clone, Copy)]
enum Pending {
    Hello,
    ListPets,
    ListOutputs,
    Reload,
}

pub struct Client {
    path: PathBuf,
    stream: Option<UnixStream>,
    incoming: Vec<u8>,
    outgoing: Vec<u8>,
    state: ConnectionState,
    next_id: u64,
    pending: HashMap<u64, Pending>,
    pub pets: Vec<PetInfo>,
    pub outputs: Vec<OutputInfo>,
    pub status: Option<StatusSnapshot>,
    pub last_error: Option<String>,
    last_attempt: Instant,
}

impl Client {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            stream: None,
            incoming: Vec::new(),
            outgoing: Vec::new(),
            state: ConnectionState::Disconnected,
            next_id: 1,
            pending: HashMap::new(),
            pets: Vec::new(),
            outputs: Vec::new(),
            status: None,
            last_error: None,
            last_attempt: Instant::now(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn state(&self) -> &ConnectionState {
        &self.state
    }

    /// Control buttons (reload/quit) require a completed handshake.
    pub fn can_control(&self) -> bool {
        self.state.is_connected()
    }

    /// Try to connect now (also used on startup). Failure is non-fatal.
    pub fn connect(&mut self) -> Result<(), String> {
        self.last_attempt = Instant::now();
        if self.stream.is_some() {
            return Ok(());
        }
        let stream = UnixStream::connect(&self.path)
            .map_err(|e| format!("cannot connect to {}: {e}", self.path.display()))?;
        stream
            .set_nonblocking(true)
            .map_err(|e| format!("cannot configure control socket: {e}"))?;
        self.stream = Some(stream);
        self.incoming.clear();
        self.outgoing.clear();
        self.pending.clear();
        self.state = ConnectionState::Connecting;
        self.last_error = None;
        self.request_pending(
            Pending::Hello,
            Op::Hello {
                protocol: PROTOCOL,
                client: Some("petweave-settings".to_string()),
            },
        );
        Ok(())
    }

    /// Poll the socket, retry the connection and dispatch responses/events.
    /// Cheap; call once per frame.
    pub fn tick(&mut self) {
        if self.stream.is_none() && self.last_attempt.elapsed() >= RECONNECT_INTERVAL {
            // The host may simply not be running: that is a normal state (F33).
            let _ = self.connect();
        }

        self.flush_outgoing();
        self.read_available();
        self.dispatch_lines();
    }

    /// Ask the host to re-read its config (after a successful save, F31).
    pub fn reload(&mut self) -> bool {
        if !self.can_control() {
            return false;
        }
        self.request_pending(Pending::Reload, Op::Reload { pets: None })
    }

    pub fn list_pets(&mut self) -> bool {
        if self.stream.is_none() {
            return false;
        }
        self.request_pending(Pending::ListPets, Op::ListPets)
    }

    pub fn list_outputs(&mut self) -> bool {
        if self.stream.is_none() {
            return false;
        }
        self.request_pending(Pending::ListOutputs, Op::ListOutputs)
    }

    /// Send an arbitrary request (tests/forward-compat); returns the id.
    pub fn request(&mut self, op: Op) -> Option<u64> {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request { id: Some(id), op };
        let mut line = serde_json::to_string(&request).ok()?;
        line.push('\n');
        self.outgoing.extend_from_slice(line.as_bytes());
        self.flush_outgoing();
        Some(id)
    }

    fn request_pending(&mut self, pending: Pending, op: Op) -> bool {
        let id = self.next_id;
        self.next_id += 1;
        let request = Request { id: Some(id), op };
        let Ok(mut line) = serde_json::to_string(&request) else {
            return false;
        };
        line.push('\n');
        self.outgoing.extend_from_slice(line.as_bytes());
        self.pending.insert(id, pending);
        self.flush_outgoing();
        true
    }

    fn flush_outgoing(&mut self) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        while !self.outgoing.is_empty() {
            match stream.write(&self.outgoing) {
                Ok(0) => break,
                Ok(n) => {
                    self.outgoing.drain(..n);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) => {
                    let message = format!("control write failed: {e}");
                    self.drop_connection(Some(message));
                    return;
                }
            }
        }
    }

    fn read_available(&mut self) {
        let Some(stream) = self.stream.as_mut() else {
            return;
        };
        let mut buffer = [0u8; 4096];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => {
                    self.drop_connection(Some("host closed the control socket".to_string()));
                    return;
                }
                Ok(n) => {
                    self.incoming.extend_from_slice(&buffer[..n]);
                    if self.incoming.len() > MAX_INCOMING {
                        self.drop_connection(Some(
                            "control stream exceeded the size limit".to_string(),
                        ));
                        return;
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => return,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => {
                    let message = format!("control read failed: {e}");
                    self.drop_connection(Some(message));
                    return;
                }
            }
        }
    }

    fn dispatch_lines(&mut self) {
        while let Some(position) = self.incoming.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.incoming.drain(..=position).collect();
            let line = &line[..line.len().saturating_sub(1)];
            if line.iter().all(|b| b.is_ascii_whitespace()) {
                continue;
            }
            self.handle_line(line);
        }
    }

    fn handle_line(&mut self, line: &[u8]) {
        let Ok(value) = serde_json::from_slice::<Value>(line) else {
            return; // ignore framing noise
        };
        if value.get("event").is_some() {
            if let Ok(event) = serde_json::from_value::<ControlEvent>(value) {
                self.apply_event(event);
            }
            return;
        }
        match serde_json::from_value::<Response>(value) {
            Ok(response) => self.apply_response(response),
            Err(e) => self.last_error = Some(format!("bad control response: {e}")),
        }
    }

    fn apply_response(&mut self, response: Response) {
        let pending = response.id.and_then(|id| self.pending.remove(&id));
        if !response.ok {
            let code = response
                .error
                .as_ref()
                .map(|e| e.code.as_str())
                .unwrap_or("unknown");
            let message = response
                .error
                .as_ref()
                .map(|e| e.message.clone())
                .unwrap_or_default();
            if code == "protocol_mismatch" {
                self.state = ConnectionState::VersionMismatch(message.clone());
            }
            self.last_error = Some(format!("{code}: {message}"));
            return;
        }
        match pending {
            Some(Pending::Hello) => {
                self.state = ConnectionState::Connected;
                self.last_error = None;
                self.list_pets();
                self.list_outputs();
            }
            Some(Pending::ListPets) => {
                if let Some(data) = response.data {
                    if let Ok(pets) = serde_json::from_value::<Vec<PetInfo>>(data) {
                        self.pets = pets;
                    }
                }
            }
            Some(Pending::ListOutputs) => {
                if let Some(data) = response.data {
                    if let Ok(outputs) = serde_json::from_value::<Vec<OutputInfo>>(data) {
                        self.outputs = outputs;
                    }
                }
            }
            Some(Pending::Reload) => {
                self.list_pets();
            }
            None => {}
        }
    }

    fn apply_event(&mut self, event: ControlEvent) {
        match event {
            ControlEvent::PetsChanged { pets } => self.pets = pets,
            ControlEvent::PetMoved { pet, rect } => {
                if let Some(info) = self.pets.iter_mut().find(|p| p.id == pet) {
                    info.rect = rect;
                }
            }
            ControlEvent::Status { fps, rss_kb, pets } => {
                self.status = Some(StatusSnapshot { fps, rss_kb, pets });
            }
        }
    }

    fn drop_connection(&mut self, message: Option<String>) {
        self.stream = None;
        self.incoming.clear();
        self.outgoing.clear();
        self.pending.clear();
        self.state = ConnectionState::Disconnected;
        if let Some(message) = message {
            self.last_error = Some(message);
        }
        // Retry soon but not in a hot loop.
        self.last_attempt = Instant::now();
    }

    /// Human-readable host status for the top bar.
    pub fn state_label(&self) -> String {
        match &self.state {
            ConnectionState::Disconnected => "宠物未运行".to_string(),
            ConnectionState::Connecting => "正在连接…".to_string(),
            ConnectionState::Connected => format!("已连接（{} 只宠物）", self.pets.len()),
            ConnectionState::VersionMismatch(message) => {
                format!("协议版本不一致：{message}")
            }
        }
    }

    /// `ListOutputs` names for the output picker, plus the configured value
    /// when it is not present (e.g. a disconnected monitor).
    pub fn output_names(&self, configured: &str) -> Vec<String> {
        let mut names: Vec<String> = self.outputs.iter().map(|o| o.name.clone()).collect();
        names.sort();
        names.dedup();
        if !configured.is_empty() && !names.iter().any(|n| n == configured) {
            names.push(configured.to_string());
        }
        names
    }
}

/// One event-line builder for tests (`PetsChanged`).
#[cfg(test)]
fn pets_changed_event(pets: &[PetInfo]) -> String {
    serde_json::to_string(&ControlEvent::PetsChanged {
        pets: pets.to_vec(),
    })
    .unwrap()
}

#[cfg(test)]
fn sample_pet(id: &str) -> PetInfo {
    PetInfo {
        id: id.into(),
        name: id.into(),
        kind: "demo".into(),
        package: String::new(),
        enabled: true,
        visible: true,
        size: [64, 64],
        rect: [0, 0, 64, 64],
        preferred_size: None,
        natural_size: None,
        size_owner: "default".into(),
        output: String::new(),
        layer: "top".into(),
        mode: "drag".into(),
        error: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::{BufRead, BufReader};
    use std::os::unix::net::UnixListener;
    use std::sync::mpsc;
    use std::thread;

    /// A scripted host: answers `Hello`/`ListPets`/`ListOutputs`/`Reload`
    /// and forwards the request ops it saw to the test.
    fn spawn_host(
        path: PathBuf,
        hello_ok: bool,
    ) -> (mpsc::Receiver<String>, thread::JoinHandle<()>) {
        let listener = UnixListener::bind(&path).expect("bind fake host");
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let (stream, _) = listener.accept().expect("accept");
            let mut writer = stream.try_clone().expect("clone");
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            loop {
                line.clear();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                let request: Value = serde_json::from_str(&line).unwrap();
                let id = request["id"].as_u64();
                let op = request["op"].as_str().unwrap_or("");
                tx.send(op.to_string()).ok();
                let response = match op {
                    "Hello" if !hello_ok => json!({
                        "id": id, "ok": false,
                        "error": {"code": "protocol_mismatch", "message": "host speaks 2"}
                    }),
                    "Hello" => json!({
                        "id": id, "ok": true,
                        "data": {"server": "petweave", "protocol": PROTOCOL, "pid": 1,
                                 "caps": {"protocol": PROTOCOL, "placement": true}}
                    }),
                    "ListPets" => json!({
                        "id": id, "ok": true,
                        "data": [sample_pet("a"), sample_pet("b")]
                    }),
                    "ListOutputs" => json!({
                        "id": id, "ok": true,
                        "data": [{"name": "eDP-1", "x": 0, "y": 0, "w": 1707, "h": 1067,
                                  "scale": 2}]
                    }),
                    "Reload" => json!({
                        "id": id, "ok": true,
                        "data": {"reloaded": true, "added": [], "removed": [],
                                 "updated": ["a"], "resized": []}
                    }),
                    _ => json!({"id": id, "ok": false,
                                "error": {"code": "unknown_op", "message": op}}),
                };
                if writeln!(writer, "{response}").is_err() || writer.flush().is_err() {
                    break; // client went away
                }
            }
        });
        (rx, handle)
    }

    fn wait_until(
        client: &mut Client,
        timeout: Duration,
        mut ready: impl FnMut(&Client) -> bool,
    ) -> bool {
        let start = Instant::now();
        while start.elapsed() < timeout {
            client.tick();
            if ready(client) {
                return true;
            }
            thread::sleep(Duration::from_millis(5));
        }
        false
    }

    #[test]
    fn connects_handshakes_and_reads_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let (ops, _host) = spawn_host(path.clone(), true);

        let mut client = Client::new(path);
        client.connect().expect("connect");
        assert!(wait_until(&mut client, Duration::from_secs(2), |c| {
            c.state() == &ConnectionState::Connected && c.pets.len() == 2 && c.outputs.len() == 1
        }));
        assert_eq!(client.outputs[0].name, "eDP-1");
        assert_eq!(client.state_label(), "已连接（2 只宠物）");

        assert!(client.reload());
        let mut seen = Vec::new();
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(2) {
            client.tick();
            while let Ok(op) = ops.try_recv() {
                seen.push(op);
            }
            if seen.iter().any(|op| op == "Reload") {
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        assert!(seen.iter().any(|op| op == "Reload"), "ops: {seen:?}");
    }

    #[test]
    fn protocol_mismatch_is_reported_and_disables_control() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let (_ops, _host) = spawn_host(path.clone(), false);
        let mut client = Client::new(path);
        client.connect().expect("connect");
        assert!(wait_until(&mut client, Duration::from_secs(2), |c| {
            matches!(c.state(), ConnectionState::VersionMismatch(_))
        }));
        assert!(!client.can_control());
        assert!(client.state_label().contains("协议版本不一致"));
        assert!(client
            .last_error
            .as_deref()
            .unwrap_or("")
            .contains("protocol_mismatch"));
        assert!(
            !client.reload(),
            "control is disabled on a version mismatch"
        );
    }

    #[test]
    fn receives_events_and_updates_state() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("control.sock");
        let (_ops, host) = spawn_host(path.clone(), true);
        let mut client = Client::new(path);
        client.connect().expect("connect");
        assert!(wait_until(&mut client, Duration::from_secs(2), |c| {
            c.state() == &ConnectionState::Connected && c.pets.len() == 2
        }));

        // The fake host is single-connection and answers requests; drive an
        // event to the client by writing it from a second connection is not
        // possible, so use the request/event handler directly on a crafted
        // line (the socket framing is covered by the host's own tests).
        let moved = json!({"event": "PetMoved", "pet": "a", "rect": [10, 20, 64, 64]});
        client.handle_line(moved.to_string().as_bytes());
        assert_eq!(client.pets[0].rect, [10, 20, 64, 64]);

        client.handle_line(
            json!({"event": "Status", "fps": 60, "rss_kb": 1234, "pets": 2})
                .to_string()
                .as_bytes(),
        );
        assert_eq!(
            client.status,
            Some(StatusSnapshot {
                fps: 60,
                rss_kb: 1234,
                pets: 2
            })
        );

        let changed = pets_changed_event(&[sample_pet("c")]);
        client.handle_line(changed.as_bytes());
        assert_eq!(client.pets.len(), 1);
        assert_eq!(client.pets[0].id, "c");

        // Disconnection is noticed on the next poll.
        drop(host);
    }

    #[test]
    fn disconnected_client_still_reports_state_and_retries_later() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing/control.sock");
        let mut client = Client::new(path);
        assert!(!client.can_control());
        assert!(client.connect().is_err());
        client.tick(); // must not panic without a host
        assert_eq!(client.state(), &ConnectionState::Disconnected);
        assert!(!client.reload());
        assert!(client.state_label().contains("未运行"));
    }
}
