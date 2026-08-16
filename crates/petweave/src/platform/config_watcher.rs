//! inotify-based config file watcher (hot reload).
//!
//! Watches the config file's parent directory (handles atomic
//! write-temp-then-rename saves) for the config filename, debounces, and
//! emits `HostCommand::ReloadConfig` on the host command channel.

use std::ffi::OsStr;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use nix::errno::Errno;
use nix::poll::{poll, PollFd, PollFlags};
use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};

use crate::app::HostCommand;

const DEBOUNCE: Duration = Duration::from_millis(300);

pub struct ConfigWatcher {
    stop: Arc<AtomicBool>,
    _handle: thread::JoinHandle<()>,
}

impl ConfigWatcher {
    /// Watch `path`; returns `None` when no config path is configured.
    pub fn start(path: Option<PathBuf>, tx: calloop::channel::Sender<HostCommand>) -> Option<Self> {
        let path = path?;
        let parent = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let file_name = path.file_name()?.to_string_lossy().to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = thread::spawn(move || watch_loop(parent, file_name, tx, stop2));
        Some(Self {
            stop,
            _handle: handle,
        })
    }
}

impl Drop for ConfigWatcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn watch_loop(
    parent: PathBuf,
    file_name: String,
    tx: calloop::channel::Sender<HostCommand>,
    stop: Arc<AtomicBool>,
) {
    let inotify = match Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC) {
        Ok(i) => i,
        Err(e) => {
            tracing::warn!("inotify unavailable ({e}) — config hot reload disabled");
            return;
        }
    };
    let wd = match inotify.add_watch(
        &parent,
        AddWatchFlags::IN_CLOSE_WRITE | AddWatchFlags::IN_MOVED_TO | AddWatchFlags::IN_CREATE,
    ) {
        Ok(wd) => wd,
        Err(e) => {
            tracing::warn!("cannot watch {} ({e}) — config hot reload disabled", parent.display());
            return;
        }
    };

    let mut pending: Option<Instant> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut fds = [PollFd::new(inotify.as_fd(), PollFlags::POLLIN)];
        match poll(&mut fds, 300u16) {
            Ok(0) => {}
            Ok(_) => match inotify.read_events() {
                Ok(events) => {
                    for ev in events {
                        if ev.wd == wd
                            && ev.name.as_deref().and_then(OsStr::to_str) == Some(file_name.as_str())
                        {
                            pending = Some(Instant::now());
                        }
                    }
                }
                Err(Errno::EAGAIN) => {}
                Err(e) => {
                    tracing::warn!("inotify read failed ({e}) — watcher stopped");
                    break;
                }
            },
            Err(_) => break,
        }
        if let Some(t) = pending {
            if t.elapsed() >= DEBOUNCE {
                pending = None;
                tracing::info!("config file changed — reloading");
                if tx.send(HostCommand::ReloadConfig).is_err() {
                    break; // host is gone
                }
            }
        }
    }
}
