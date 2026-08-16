//! Periodic system-resource sampling -> `Event::System` for pets.
//!
//! A lightweight thread reads CPU/memory via `sysinfo` on a fixed interval and
//! pushes a snapshot into the host event channel (same channel as input).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use petweave_core::events::{Event, SystemSnapshot};

pub struct SystemSampler {
    stop: Arc<AtomicBool>,
    _handle: thread::JoinHandle<()>,
}

impl SystemSampler {
    /// Start sampling; returns `None` when the interval is zero (disabled).
    pub fn start(tx: calloop::channel::Sender<Event>, interval: Duration) -> Option<Self> {
        if interval.is_zero() {
            return None;
        }
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = stop.clone();
        let handle = thread::spawn(move || {
            let mut sys = sysinfo::System::new_all();
            tracing::debug!("system sampler started (interval {interval:?})");
            loop {
                if stop2.load(Ordering::Relaxed) {
                    break;
                }
                sys.refresh_cpu_usage();
                sys.refresh_memory();
                let mem_pct = if sys.total_memory() > 0 {
                    (sys.used_memory() as f32 / sys.total_memory() as f32) * 100.0
                } else {
                    0.0
                };
                let snapshot = SystemSnapshot {
                    cpu_usage_percent: sys.global_cpu_usage(),
                    mem_usage_percent: mem_pct,
                };
                if tx.send(Event::System(snapshot)).is_err() {
                    break; // host is gone
                }
                thread::sleep(interval);
            }
        });
        Some(Self {
            stop,
            _handle: handle,
        })
    }
}

impl Drop for SystemSampler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}
