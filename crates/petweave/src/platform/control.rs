//! Control channel (`docs/GUI_ROADMAP.md` G1).
//!
//! The settings GUI (`crates/petweave-settings`) needs to read the *real*
//! surface rectangles of every running pet and to change placement/size while
//! the host is running. The host therefore listens on a Unix socket:
//!
//! ```text
//! ${XDG_RUNTIME_DIR}/petweave/control.sock     dir 0700, socket 0600
//! ```
//!
//! Protocol: **JSON Lines**, one object per line, `\n` terminated, at most
//! [`MAX_LINE`] bytes per line. Requests carry `{"id": N, "op": "ListPets"}`,
//! responses echo the id and are either `{"id":N,"ok":true,"data":…}` or
//! `{"id":N,"ok":false,"error":{"code":…,"message":…}}`. Events are pushed
//! unsolicited and have no id (`{"event":"PetMoved",…}`).
//!
//! Architecture invariant: **this module never touches pet/Wayland state**.
//! Client threads only parse lines and hand a [`ControlCall`] to the main
//! loop through the host command channel; the main loop (calloop thread)
//! applies it and answers through the call's [`Outbox`]. Replies and events
//! are written by a per-client writer thread, so the main loop never blocks
//! on a slow client.
//!
//! Version negotiation: `Hello` must carry `protocol = 1`
//! ([`PROTOCOL`]); a mismatch is answered with `protocol_mismatch` and the
//! GUI falls back to config editing only.
//!
//! `XDG_RUNTIME_DIR` missing or the socket unbindable is not fatal: the
//! channel is disabled with a warning and the host keeps running.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Protocol version spoken by this host. Bump when an op changes shape.
pub const PROTOCOL: u32 = 1;

/// Maximum bytes of one request line; longer lines close the connection.
pub const MAX_LINE: usize = 64 * 1024;

/// Request ops this host understands (used to answer `unknown_op` clearly
/// instead of a generic serde error).
pub const KNOWN_OPS: &[&str] = &[
    "Hello",
    "ListPets",
    "ListOutputs",
    "Reload",
    "SetPlacement",
    "SetSize",
    "SetVisible",
    "SetMoveMode",
    "SetPaused",
    "RenderPet",
    "Quit",
];

/// `${XDG_RUNTIME_DIR}/petweave/control.sock`, if the runtime dir is set.
pub fn socket_path() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(|dir| PathBuf::from(dir).join("petweave").join("control.sock"))
}

// --- requests --------------------------------------------------------------

/// One request line: `{"id": 7, "op": "SetPlacement", …}`.
#[derive(Debug, Clone, Deserialize)]
pub struct Request {
    /// Echoed in the response. Optional so a bare `{"op":"ListPets"}` works.
    #[serde(default)]
    pub id: Option<u64>,
    #[serde(flatten)]
    pub op: Op,
}

/// Request operation (internally tagged by `op`).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "op")]
pub enum Op {
    /// Handshake: `{"op":"Hello","protocol":1,"client":"petweave-settings"}`.
    Hello {
        #[serde(default)]
        protocol: u32,
        #[serde(default)]
        client: Option<String>,
    },
    /// Running pets with their real rects/sizes.
    ListPets,
    /// Outputs known to the compositor (xdg-output names + rects).
    ListOutputs,
    /// Re-read the config file, or apply an in-memory patch (`pets`) without
    /// writing it (GUI slider preview).
    Reload {
        #[serde(default)]
        pets: Option<Vec<PetPatch>>,
    },
    /// Immediately change one pet's anchor + margins (runtime-only).
    SetPlacement {
        pet: String,
        anchor: String,
        margins: [i32; 4],
    },
    /// Immediately resize one pet (runtime-only). `0` derives that side from
    /// the artwork aspect ratio; both sides `0` is an error.
    SetSize {
        pet: String,
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
    },
    /// Show/hide one pet, or every pet when `pet` is omitted.
    SetVisible {
        #[serde(default)]
        pet: Option<String>,
        visible: bool,
    },
    /// Switch drag/physics/fixed for one pet or all of them (runtime-only).
    SetMoveMode {
        #[serde(default)]
        pet: Option<String>,
        mode: String,
    },
    /// Pause/resume pet animation ticks (idle CPU / debugging).
    SetPaused { paused: bool },
    /// Render one pet on the host (not implemented yet: GUI-side preview
    /// reuses `petweave`'s runtime instead; see G2/G3).
    RenderPet {
        pet: String,
        #[serde(default)]
        width: u32,
        #[serde(default)]
        height: u32,
    },
    /// Ask the host to exit.
    Quit,
}

/// In-memory config patch (never written by the host, D10).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PetPatch {
    pub id: String,
    /// `0` = inherit/derive (same as a config file), matching `SetSize`.
    #[serde(default)]
    pub width: Option<u32>,
    /// `0` = inherit/derive (same as a config file), matching `SetSize`.
    #[serde(default)]
    pub height: Option<u32>,
    /// `top|right|bottom|left` (`|` separated).
    #[serde(default)]
    pub anchor: Option<String>,
    /// `[top, right, bottom, left]`.
    #[serde(default)]
    pub margins: Option<[i32; 4]>,
    #[serde(default)]
    pub layer: Option<String>,
    #[serde(default)]
    pub mode: Option<String>,
    #[serde(default)]
    pub output: Option<String>,
    #[serde(default)]
    pub click_through: Option<bool>,
    #[serde(default)]
    pub disable_fullscreen_hide: Option<bool>,
    #[serde(default)]
    pub enabled: Option<bool>,
}

// --- responses / events ----------------------------------------------------

/// One response line. `data` is op-specific, `error` is set on failure.
#[derive(Debug, Clone, Serialize)]
pub struct Response {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ProtocolError>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ProtocolError {
    pub code: String,
    pub message: String,
}

impl Response {
    pub fn ok(id: Option<u64>, data: Value) -> Self {
        Self {
            id,
            ok: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn err(id: Option<u64>, code: &str, message: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            data: None,
            error: Some(ProtocolError {
                code: code.to_string(),
                message: message.into(),
            }),
        }
    }

    pub fn error_code(&self) -> Option<&str> {
        self.error.as_ref().map(|e| e.code.as_str())
    }
}

/// Host → client push event (no request id).
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event")]
pub enum ControlEvent {
    /// Pet set / parameters / visibility changed.
    PetsChanged { pets: Vec<PetInfo> },
    /// A pet moved (drag or physics) — keeps the GUI canvas in sync.
    PetMoved { pet: String, rect: [i32; 4] },
    /// Periodic diagnostics snapshot.
    Status { fps: u32, rss_kb: u64, pets: usize },
}

/// One pet as reported by `ListPets` / `PetsChanged`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PetInfo {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub package: String,
    pub enabled: bool,
    pub visible: bool,
    /// Current surface size in logical pixels.
    pub size: [u32; 2],
    /// On-screen rectangle `[x, y, w, h]` in output-logical pixels.
    pub rect: [i32; 4],
    /// What the pet asks for (`Pet::preferred_size`), if anything.
    pub preferred_size: Option<[u32; 2]>,
    /// Intrinsic artwork size (same as `preferred_size` today).
    pub natural_size: Option<[u32; 2]>,
    /// Who owns the current size: `config`, `pet`, `script` or `default`.
    pub size_owner: String,
    pub output: String,
    pub layer: String,
    pub mode: String,
    /// Why the pet is not running (package load failure), if it failed.
    pub error: Option<String>,
}

/// One output as reported by `ListOutputs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OutputInfo {
    pub name: String,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub scale: i32,
}

// --- errors ----------------------------------------------------------------

/// An op failure the main loop wants to report to the client.
#[derive(Debug, Clone)]
pub struct ControlError {
    pub code: &'static str,
    pub message: String,
}

impl ControlError {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    pub fn pet_not_found(id: &str) -> Self {
        Self::new("pet_not_found", format!("no running pet with id {id:?}"))
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new("invalid_argument", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

/// `Hello` protocol check (pure, unit-tested).
pub fn check_protocol(protocol: u32) -> std::result::Result<(), ControlError> {
    if protocol == PROTOCOL {
        Ok(())
    } else {
        Err(ControlError::new(
            "protocol_mismatch",
            format!(
                "host speaks protocol {PROTOCOL}, client sent {protocol}; \
                 upgrade petweave and petweave-settings together"
            ),
        ))
    }
}

// --- server plumbing -------------------------------------------------------

/// Writes to one client. Cloned into every call so the main loop can answer
/// without holding a socket (and without blocking on it).
#[derive(Debug, Clone)]
pub struct Outbox {
    tx: mpsc::Sender<String>,
}

impl Outbox {
    pub fn respond(&self, response: &Response) {
        if let Ok(line) = serde_json::to_string(response) {
            let _ = self.tx.send(line);
        }
    }

    pub fn send_event(&self, event: &ControlEvent) {
        if let Ok(line) = serde_json::to_string(event) {
            let _ = self.tx.send(line);
        }
    }
}

/// A parsed request plus the channel to answer it on.
#[derive(Debug, Clone)]
pub struct ControlCall {
    pub id: Option<u64>,
    pub op: Op,
    pub out: Outbox,
}

/// How the control threads hand a request to the main loop.
pub type ControlSink = Box<dyn Fn(ControlCall) + Send + Sync>;

struct Hub {
    clients: Mutex<HashMap<u64, mpsc::Sender<String>>>,
    next_id: AtomicU64,
    stop: AtomicBool,
}

impl Default for Hub {
    fn default() -> Self {
        Self {
            clients: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        }
    }
}

impl Hub {
    fn clients(&self) -> std::sync::MutexGuard<'_, HashMap<u64, mpsc::Sender<String>>> {
        // A panic in a client thread must not poison the registry.
        self.clients.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// Handle held by the host: broadcast events, and clean up the socket on exit.
pub struct ControlHandle {
    hub: Arc<Hub>,
    path: PathBuf,
}

impl ControlHandle {
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Number of connected control clients (diagnostics/tests).
    pub fn client_count(&self) -> usize {
        self.hub.clients().len()
    }

    /// Send one event to every connected client; dead clients are pruned.
    pub fn broadcast(&self, event: &ControlEvent) {
        let Ok(line) = serde_json::to_string(event) else {
            return;
        };
        let mut clients = self.hub.clients();
        clients.retain(|_, tx| tx.send(line.clone()).is_ok());
    }
}

impl Drop for ControlHandle {
    fn drop(&mut self) {
        self.hub.stop.store(true, Ordering::SeqCst);
        // Wake the blocking `accept` so the listener thread can exit, then
        // unlink the socket so a later host starts cleanly.
        let _ = std::os::unix::net::UnixStream::connect(&self.path);
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Start the control channel at the default path, or disable it with a
/// warning when the runtime dir is unavailable/unbindable.
pub fn start(sink: ControlSink) -> Option<ControlHandle> {
    let path = socket_path().or_else(|| {
        tracing::warn!("XDG_RUNTIME_DIR is not set — control channel disabled");
        None
    })?;
    match start_at(path.clone(), sink) {
        Ok(handle) => {
            tracing::info!("control channel listening on {}", path.display());
            Some(handle)
        }
        Err(e) => {
            tracing::warn!("control channel disabled ({}): {e:#}", path.display());
            None
        }
    }
}

/// Start the server at an explicit path (tests / `--socket`-style setups).
///
/// Creates the parent directory 0700, removes a stale socket, binds, then
/// restricts the socket to 0600.
pub fn start_at(path: PathBuf, sink: ControlSink) -> Result<ControlHandle> {
    if let Some(dir) = path.parent() {
        if !dir.as_os_str().is_empty() {
            std::fs::create_dir_all(dir)
                .with_context(|| format!("cannot create {}", dir.display()))?;
            set_mode(dir, 0o700)?;
        }
    }
    // A leftover socket from a crashed host must not block startup; the PID
    // singleton guarantees no live host owns it at this point.
    let _ = std::fs::remove_file(&path);
    let listener = std::os::unix::net::UnixListener::bind(&path)
        .with_context(|| format!("cannot bind {}", path.display()))?;
    set_mode(&path, 0o600)?;

    let hub = Arc::new(Hub::default());
    let sink = Arc::new(sink);
    let accept_hub = hub.clone();
    std::thread::Builder::new()
        .name("petweave-control".into())
        .spawn(move || accept_loop(listener, sink, accept_hub))
        .context("cannot spawn control listener thread")?;

    Ok(ControlHandle { hub, path })
}

fn set_mode(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

fn accept_loop(listener: std::os::unix::net::UnixListener, sink: Arc<ControlSink>, hub: Arc<Hub>) {
    loop {
        match listener.accept() {
            Ok((stream, _addr)) => {
                if hub.stop.load(Ordering::SeqCst) {
                    break;
                }
                serve_client(stream, &sink, &hub);
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => {
                if !hub.stop.load(Ordering::SeqCst) {
                    tracing::warn!("control channel accept failed: {e}");
                }
                break;
            }
        }
    }
}

/// Wire one accepted connection: a writer thread draining the outbox and a
/// reader thread parsing request lines.
fn serve_client(stream: std::os::unix::net::UnixStream, sink: &Arc<ControlSink>, hub: &Arc<Hub>) {
    let Ok(write_stream) = stream.try_clone() else {
        tracing::warn!("control: cannot clone client socket");
        return;
    };
    let id = hub.next_id.fetch_add(1, Ordering::Relaxed);
    let (tx, rx) = mpsc::channel::<String>();
    hub.clients().insert(id, tx.clone());

    let spawned_writer = std::thread::Builder::new()
        .name(format!("petweave-control-w{id}"))
        .spawn(move || {
            let mut writer = write_stream;
            for line in rx {
                if writeln!(writer, "{line}")
                    .and_then(|_| writer.flush())
                    .is_err()
                {
                    break;
                }
            }
            let _ = writer.shutdown(std::net::Shutdown::Both);
        })
        .is_ok();
    if !spawned_writer {
        hub.clients().remove(&id);
        return;
    }

    let read_hub = hub.clone();
    let read_sink = sink.clone();
    let spawned_reader = std::thread::Builder::new()
        .name(format!("petweave-control-r{id}"))
        .spawn(move || {
            client_read_loop(stream, tx, read_sink);
            // Closing the outbox (drops the last non-hub sender after the
            // registry entry is gone) ends the writer thread.
            read_hub.clients().remove(&id);
        })
        .is_ok();
    if !spawned_reader {
        hub.clients().remove(&id);
    }
}

fn client_read_loop(
    stream: std::os::unix::net::UnixStream,
    out: mpsc::Sender<String>,
    sink: Arc<ControlSink>,
) {
    let mut reader = BufReader::new(stream);
    let outbox = Outbox { tx: out };
    loop {
        match read_limited_line(&mut reader, MAX_LINE) {
            Ok(None) => break,
            Ok(Some(line)) => {
                if line.iter().all(|b| b.is_ascii_whitespace()) {
                    continue;
                }
                match parse_request(&line) {
                    Ok(request) => sink(ControlCall {
                        id: request.id,
                        op: request.op,
                        out: outbox.clone(),
                    }),
                    Err(response) => outbox.respond(&response),
                }
            }
            Err(LineError::TooLong) => {
                outbox.respond(&Response::err(
                    None,
                    "line_too_long",
                    format!("request exceeds {MAX_LINE} bytes; closing connection"),
                ));
                break;
            }
            Err(LineError::Io(e)) => {
                tracing::debug!("control client read ended: {e}");
                break;
            }
        }
    }
}

/// Parse one request line. Returns a ready-to-send error response on failure
/// (bad JSON, missing/unknown `op`, or a field with the wrong type).
pub fn parse_request(line: &[u8]) -> std::result::Result<Request, Response> {
    let value: Value = match serde_json::from_slice(line) {
        Ok(v) => v,
        Err(e) => return Err(Response::err(None, "bad_json", e.to_string())),
    };
    let id = value.get("id").and_then(Value::as_u64);
    let Some(op) = value.get("op").and_then(Value::as_str) else {
        return Err(Response::err(
            id,
            "bad_request",
            "missing \"op\" (expected a JSON object such as {\"id\":1,\"op\":\"ListPets\"})",
        ));
    };
    if !KNOWN_OPS.contains(&op) {
        return Err(Response::err(
            id,
            "unknown_op",
            format!("unknown op {op:?} (known: {})", KNOWN_OPS.join(", ")),
        ));
    }
    serde_json::from_value(value).map_err(|e| Response::err(id, "bad_request", e.to_string()))
}

// --- line framing ----------------------------------------------------------

#[derive(Debug)]
enum LineError {
    TooLong,
    Io(std::io::Error),
}

/// Read one `\n`-terminated line without ever growing past `max` bytes.
///
/// `Ok(None)` is EOF. A final line without a trailing newline is returned as
/// a line (clients may close right after writing).
fn read_limited_line<R: BufRead>(
    reader: &mut R,
    max: usize,
) -> std::result::Result<Option<Vec<u8>>, LineError> {
    let mut out = Vec::new();
    loop {
        let (chunk, consumed, done) = {
            let buf = reader.fill_buf().map_err(LineError::Io)?;
            if buf.is_empty() {
                (None, 0, true)
            } else if let Some(pos) = buf.iter().position(|b| *b == b'\n') {
                (Some(buf[..pos].to_vec()), pos + 1, true)
            } else {
                (Some(buf.to_vec()), buf.len(), false)
            }
        };
        if let Some(chunk) = chunk {
            if out.len() + chunk.len() > max {
                return Err(LineError::TooLong);
            }
            out.extend_from_slice(&chunk);
        }
        if consumed > 0 {
            reader.consume(consumed);
        }
        if done {
            return Ok(if out.is_empty() { None } else { Some(out) });
        }
    }
}

/// Resident set size of this process in KiB (`/proc/self/statm`).
pub fn rss_kb() -> u64 {
    let Ok(statm) = std::fs::read_to_string("/proc/self/statm") else {
        return 0;
    };
    let Some(pages) = statm
        .split_whitespace()
        .nth(1)
        .and_then(|v| v.parse::<u64>().ok())
    else {
        return 0;
    };
    // SAFETY: sysconf with a valid constant has no preconditions.
    let page_size = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
    let page_size = if page_size > 0 {
        page_size as u64
    } else {
        4096
    };
    pages * page_size / 1024
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::os::unix::fs::PermissionsExt;
    use std::os::unix::net::UnixStream;
    use std::time::Duration;

    fn parse(line: &str) -> Request {
        parse_request(line.as_bytes()).expect("valid request")
    }

    #[test]
    fn parses_golden_requests() {
        assert!(matches!(
            parse(r#"{"id":1,"op":"ListPets"}"#).op,
            Op::ListPets
        ));
        let r = parse(
            r#"{"id":2,"op":"SetPlacement","pet":"cat","anchor":"bottom|left","margins":[1,2,3,4]}"#,
        );
        match r.op {
            Op::SetPlacement {
                pet,
                anchor,
                margins,
            } => {
                assert_eq!(pet, "cat");
                assert_eq!(anchor, "bottom|left");
                assert_eq!(margins, [1, 2, 3, 4]);
            }
            other => panic!("unexpected op: {other:?}"),
        }
        // `pet` is optional for SetVisible / SetMoveMode.
        match parse(r#"{"id":3,"op":"SetVisible","visible":false}"#).op {
            Op::SetVisible { pet, visible } => {
                assert!(pet.is_none());
                assert!(!visible);
            }
            other => panic!("unexpected op: {other:?}"),
        }
        assert!(matches!(parse(r#"{"op":"Quit"}"#).op, Op::Quit));
        assert_eq!(parse(r#"{"op":"Quit"}"#).id, None);
        // Reload patch shape.
        match parse(r#"{"id":4,"op":"Reload","pets":[{"id":"cat","width":200}]}"#).op {
            Op::Reload { pets } => {
                let patch = pets.expect("patch present").pop().expect("one patch");
                assert_eq!(patch.id, "cat");
                assert_eq!(patch.width, Some(200));
                assert_eq!(patch.height, None);
            }
            other => panic!("unexpected op: {other:?}"),
        }
    }

    #[test]
    fn rejects_unknown_ops_and_bad_json() {
        let resp = parse_request(br#"{"id":9,"op":"FlyAway"}"#).unwrap_err();
        assert_eq!(resp.id, Some(9));
        assert_eq!(resp.error_code(), Some("unknown_op"));

        let resp = parse_request(br#"{"id":10}"#).unwrap_err();
        assert_eq!(resp.error_code(), Some("bad_request"));

        let resp = parse_request(b"not json at all").unwrap_err();
        assert_eq!(resp.error_code(), Some("bad_json"));

        // Correct op name, wrong field type: still a clear error with the id.
        let resp =
            parse_request(br#"{"id":11,"op":"SetSize","pet":"x","width":"wide"}"#).unwrap_err();
        assert_eq!(resp.id, Some(11));
        assert_eq!(resp.error_code(), Some("bad_request"));
    }

    #[test]
    fn responses_have_the_documented_shape() {
        let ok = Response::ok(Some(7), serde_json::json!({"rect": [1, 2, 3, 4]}));
        let v = serde_json::to_value(&ok).unwrap();
        assert_eq!(v["id"], 7);
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["rect"], serde_json::json!([1, 2, 3, 4]));
        assert!(v.get("error").is_none());

        let err = Response::err(Some(8), "pet_not_found", "no pet");
        let v = serde_json::to_value(&err).unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "pet_not_found");
        assert!(v.get("data").is_none());
    }

    #[test]
    fn events_are_tagged_and_have_no_id() {
        let ev = ControlEvent::PetMoved {
            pet: "cat".into(),
            rect: [10, 20, 30, 40],
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(v["event"], "PetMoved");
        assert_eq!(v["pet"], "cat");
        assert!(v.get("id").is_none());

        let ev = ControlEvent::Status {
            fps: 60,
            rss_kb: 1234,
            pets: 3,
        };
        assert_eq!(serde_json::to_value(&ev).unwrap()["event"], "Status");
    }

    #[test]
    fn protocol_version_is_enforced() {
        assert!(check_protocol(PROTOCOL).is_ok());
        let e = check_protocol(PROTOCOL + 1).unwrap_err();
        assert_eq!(e.code, "protocol_mismatch");
        let e = check_protocol(0).unwrap_err();
        assert_eq!(e.code, "protocol_mismatch");
    }

    #[test]
    fn limited_reader_handles_lines_eof_and_oversize() {
        let mut r = Cursor::new(b"{\"a\":1}\n{\"b\":2}\n".to_vec());
        assert_eq!(
            read_limited_line(&mut r, MAX_LINE).unwrap(),
            Some(br#"{"a":1}"#.to_vec())
        );
        assert_eq!(
            read_limited_line(&mut r, MAX_LINE).unwrap(),
            Some(br#"{"b":2}"#.to_vec())
        );
        assert_eq!(read_limited_line(&mut r, MAX_LINE).unwrap(), None);

        // Final line without a newline (client closed right after writing).
        let mut r = Cursor::new(b"{\"a\":1}".to_vec());
        assert_eq!(
            read_limited_line(&mut r, MAX_LINE).unwrap(),
            Some(br#"{"a":1}"#.to_vec())
        );
        assert_eq!(read_limited_line(&mut r, MAX_LINE).unwrap(), None);

        // Exactly max bytes is allowed, one more is not.
        let mut r = Cursor::new(vec![b'x'; 8]);
        assert_eq!(read_limited_line(&mut r, 8).unwrap(), Some(vec![b'x'; 8]));
        let mut r = Cursor::new(vec![b'x'; 9]);
        assert!(matches!(
            read_limited_line(&mut r, 8),
            Err(LineError::TooLong)
        ));
    }

    /// End-to-end wire test without Wayland: a socket client sends a request,
    /// the test main "loop" (a plain channel) answers, the client reads it
    /// back; a broadcast event then arrives on the same connection.
    #[test]
    fn socket_round_trip_and_event_broadcast() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("petweave/control.sock");
        let (tx, rx) = mpsc::channel::<ControlCall>();
        let handle = start_at(
            path.clone(),
            Box::new(move |call| {
                tx.send(call).expect("main loop channel");
            }),
        )
        .expect("start server");

        let mut client = UnixStream::connect(&path).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        writeln!(client, r#"{{"id":1,"op":"ListPets"}}"#).unwrap();

        let call = rx.recv_timeout(Duration::from_secs(5)).expect("request");
        assert_eq!(call.id, Some(1));
        assert!(matches!(call.op, Op::ListPets));
        call.out
            .respond(&Response::ok(Some(1), serde_json::json!([])));

        let mut reader = BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["id"], 1);
        assert_eq!(resp["ok"], true);
        assert_eq!(resp["data"], serde_json::json!([]));

        // Events share the connection and are tagged instead of id-carrying.
        handle.broadcast(&ControlEvent::PetMoved {
            pet: "cat".into(),
            rect: [1, 2, 3, 4],
        });
        line.clear();
        reader.read_line(&mut line).unwrap();
        let ev: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(ev["event"], "PetMoved");
        assert_eq!(ev["rect"], serde_json::json!([1, 2, 3, 4]));

        // Disconnect is noticed and the registry shrinks.
        drop(reader);
        drop(client);
        for _ in 0..100 {
            if handle.client_count() == 0 {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(handle.client_count(), 0, "client pruned after disconnect");
    }

    #[test]
    fn socket_permissions_are_private() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("petweave/control.sock");
        let (_tx, _rx) = mpsc::channel::<ControlCall>();
        let handle = start_at(path.clone(), Box::new(move |_| {})).expect("start server");
        let dir_mode = std::fs::metadata(path.parent().unwrap())
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        let sock_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700, "control directory is user-only");
        assert_eq!(sock_mode, 0o600, "control socket is user-only");
        assert_eq!(handle.path(), path.as_path());
    }

    #[test]
    fn oversized_line_is_rejected_and_connection_closed() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("control.sock");
        let (tx, rx) = mpsc::channel::<ControlCall>();
        let _handle = start_at(
            path.clone(),
            Box::new(move |call| {
                let _ = tx.send(call);
            }),
        )
        .unwrap();

        let mut client = UnixStream::connect(&path).expect("connect");
        client
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut huge = vec![b'x'; MAX_LINE + 10];
        huge.push(b'\n');
        client.write_all(&huge).unwrap();

        let mut reader = BufReader::new(client);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let resp: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(resp["ok"], false);
        assert_eq!(resp["error"]["code"], "line_too_long");
        // The connection is closed afterwards, and no request reached the
        // main loop.
        let mut rest = String::new();
        assert_eq!(reader.read_line(&mut rest).unwrap(), 0);
        assert!(rx.recv_timeout(Duration::from_millis(50)).is_err());
    }
}
