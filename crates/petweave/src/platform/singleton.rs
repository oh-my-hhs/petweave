//! Process singleton: flock'd PID file in `$XDG_RUNTIME_DIR`.
//!
//! `flock` is released automatically if the process dies, avoiding stale-PID
//! races; the file is removed on graceful exit (Drop).

use std::fs;
use std::os::fd::{AsFd, OwnedFd};
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use nix::errno::Errno;
use nix::fcntl::{Flock, FlockArg, OFlag, open};
use nix::sys::stat::Mode;

pub struct Singleton {
    path: PathBuf,
    _lock: Flock<OwnedFd>,
}

impl Singleton {
    /// Acquire the singleton; errors if another instance is running.
    pub fn acquire() -> Result<Self> {
        let dir = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .ok_or_else(|| anyhow::anyhow!("XDG_RUNTIME_DIR is not set"))?;
        let path = dir.join("petweave.pid");
        let file = open(
            &path,
            OFlag::O_CREAT | OFlag::O_RDWR | OFlag::O_CLOEXEC,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )
        .with_context(|| format!("cannot open {}", path.display()))?;
        let lock = match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
            Ok(l) => l,
            Err((_, Errno::EWOULDBLOCK)) => {
                bail!(
                    "another petweave instance is already running ({} is locked)",
                    path.display()
                );
            }
            Err((_, e)) => return Err(e).with_context(|| format!("flock {}", path.display())),
        };
        // Write our pid for tooling (the lock is the real guard).
        let w = open(&path, OFlag::O_WRONLY | OFlag::O_CLOEXEC, Mode::empty())
            .with_context(|| format!("cannot reopen {}", path.display()))?;
        let _ = nix::unistd::ftruncate(w.as_fd(), 0);
        let _ = nix::unistd::write(w.as_fd(), format!("{}\n", std::process::id()).as_bytes());
        Ok(Self { path, _lock: lock })
    }
}

impl Drop for Singleton {
    fn drop(&mut self) {
        // The flock is still held here; the lock (and its fd) drops after us.
        let _ = fs::remove_file(&self.path);
    }
}
