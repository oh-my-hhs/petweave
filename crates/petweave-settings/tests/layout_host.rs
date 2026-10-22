//! Layout canvas against a **scripted host** (no compositor): the canvas
//! reads real output geometry + pet rectangles from the control channel and
//! pushes `SetPlacement` back while dragging.
//!
//! This covers the two behaviours that a pure geometry test cannot: F21 (page
//! per output, drawing host-reported rects) and F22 (drag → live placement on
//! the running host).

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use petweave_settings::app::{Page, SettingsApp};

const CONFIG: &str = r#"# 双显示器布局
[render]
width = 200
height = 120

[[pets]]
id = "cat"
name = "Cat"
kind = "demo"
output = "eDP-1"

[[pets]]
id = "dog"
name = "Dog"
kind = "demo"
output = "HDMI-A-1"
"#;

fn pet_info(id: &str, name: &str, output: &str, rect: [i32; 4]) -> Value {
    json!({
        "id": id,
        "name": name,
        "kind": "demo",
        "package": "",
        "enabled": true,
        "visible": true,
        "size": [rect[2], rect[3]],
        "rect": rect,
        "preferred_size": null,
        "natural_size": null,
        "size_owner": "config",
        "output": output,
        "layer": "top",
        "mode": "drag",
        "error": null,
    })
}

/// A minimal host: answers the ops the canvas needs and records them.
fn spawn_host(path: PathBuf) -> (mpsc::Receiver<(String, Value)>, thread::JoinHandle<()>) {
    let listener = UnixListener::bind(&path).expect("bind fake host");
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        let mut writer = stream.try_clone().expect("clone");
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        loop {
            line.clear();
            if reader.read_line(&mut line).unwrap_or(0) == 0 {
                break;
            }
            let Ok(request) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let id = request["id"].as_u64();
            let op = request["op"].as_str().unwrap_or("").to_string();
            tx.send((op.clone(), request.clone())).ok();
            let response = match op.as_str() {
                "Hello" => json!({
                    "id": id, "ok": true,
                    "data": {"server": "petweave", "protocol": 1, "pid": 1,
                             "caps": {"protocol": 1, "placement": true}}
                }),
                "ListPets" => json!({
                    "id": id, "ok": true,
                    "data": [
                        pet_info("cat", "Cat", "eDP-1", [100, 200, 200, 120]),
                        pet_info("dog", "Dog", "HDMI-A-1", [40, 300, 200, 120]),
                    ]
                }),
                "ListOutputs" => json!({
                    "id": id, "ok": true,
                    "data": [
                        {"name": "HDMI-A-1", "x": 1707, "y": 0, "w": 1920, "h": 1080, "scale": 1},
                        {"name": "eDP-1", "x": 0, "y": 0, "w": 1707, "h": 1067, "scale": 2},
                    ]
                }),
                "SetPlacement" => json!({
                    "id": id, "ok": true,
                    "data": {"rect": [100, 200, 200, 120]}
                }),
                "Reload" => json!({
                    "id": id, "ok": true,
                    "data": {"reloaded": true, "added": [], "removed": [],
                             "updated": [], "resized": []}
                }),
                _ => json!({"id": id, "ok": false,
                            "error": {"code": "unknown_op", "message": op}}),
            };
            if writeln!(writer, "{response}").is_err() || writer.flush().is_err() {
                break;
            }
        }
    });
    (rx, handle)
}

fn wait_ready(app: &mut SettingsApp, timeout: Duration) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        app.client.tick();
        if app.client.can_control()
            && app.client.pets.len() == 2
            && app.client.outputs.len() == 2
        {
            return;
        }
        thread::sleep(Duration::from_millis(5));
    }
    panic!("fake host handshake did not complete");
}

#[test]
fn canvas_uses_host_outputs_and_rects_then_pushes_placement() {
    let dir = tempfile::tempdir().expect("tempdir");
    let config_path = dir.path().join("petweave.toml");
    std::fs::write(&config_path, CONFIG).expect("write config");
    let socket = dir.path().join("control.sock");
    let (ops, _host) = spawn_host(socket.clone());

    let mut app = SettingsApp::new_at(config_path, socket).expect("app");
    app.page = Page::Layout;
    wait_ready(&mut app, Duration::from_secs(3));

    // F21: one page per output, sorted left-to-right, with real geometry.
    let pages = app.output_pages();
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].name, "eDP-1");
    assert_eq!(pages[0].scale, 2);
    assert_eq!((pages[1].name.as_str(), pages[1].rect.x), ("HDMI-A-1", 1707.0));
    assert!(!pages[0].synthetic);

    // Pets are paged by their configured output, drawn at the host's rect.
    let first = app.canvas_pets(&pages, 0);
    assert_eq!(first.len(), 1, "only the eDP-1 pet is on the first page");
    assert_eq!(first[0].id, "cat");
    assert!(first[0].live, "rect comes from ListPets");
    assert_eq!((first[0].rect.x, first[0].rect.y), (100.0, 200.0));
    let second = app.canvas_pets(&pages, 1);
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id, "dog");
    assert_eq!((second[0].rect.x, second[0].rect.y), (40.0, 300.0));

    // F22: a drag writes the draft *and* reaches the host.
    let placement = app
        .set_pet_position(0, 300.0, 500.0, &pages[0])
        .expect("placement");
    // The pet inherited the `bottom` anchor from `[render]`, so the drag keeps
    // it and only adds the missing horizontal side.
    assert_eq!(placement.0, "bottom|left");
    assert!(app.dirty());
    assert!(app.push_placement("cat", &placement));
    // The same placement is not re-sent every frame of a slow drag.
    assert!(!app.push_placement("cat", &placement));

    let mut seen = Vec::new();
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(2) {
        app.client.tick();
        while let Ok((op, request)) = ops.try_recv() {
            seen.push((op, request));
        }
        if seen.iter().any(|(op, _)| op == "SetPlacement") {
            break;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let placed = seen
        .iter()
        .find(|(op, _)| op == "SetPlacement")
        .map(|(_, request)| request)
        .expect("SetPlacement reached the host");
    assert_eq!(placed["pet"], "cat");
    assert_eq!(placed["anchor"], "bottom|left");
    assert_eq!(placed["margins"][3], 300, "left margin from the drag");
    // 1067px tall output, 120px pet, y = 500 → 447px from the bottom.
    assert_eq!(placed["margins"][2], 1067 - 500 - 120, "bottom margin");

    // Saving keeps the file valid and comment-preserving.
    app.save();
    assert!(app.error.is_none(), "{:?}", app.error);
    let text = std::fs::read_to_string(app.store.path.clone()).expect("saved");
    assert!(text.contains("# 双显示器布局"), "{text}");
    let config = petweave_core::Config::parse(&text).expect("parses");
    config.validate().expect("valid");
    assert_eq!(config.pets[0].margin_left, Some(300));
    assert_eq!(config.pets[0].margin_bottom, Some(447));
    assert_eq!(config.pets[0].margin_top, None, "unused key stays out");
}
