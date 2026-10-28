//! Labeled simulation transport. Not a hardware channel.

use std::collections::VecDeque;
use std::io::{self, ErrorKind};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ccc_core::{Error, Role};
use ccc_protocol::Wire;

pub const SIM_QUEUE_CAPACITY: usize = 256;
const SIM_MAX_BLOB: usize = 2048;

pub struct SimWire {
    tx: Arc<Mutex<VecDeque<Vec<u8>>>>,
    rx: Arc<Mutex<VecDeque<Vec<u8>>>>,
    stop: Arc<AtomicBool>,
    drop_every: Option<u32>,
    sent: u32,
}

pub struct SimPair {
    pub a: SimWire,
    pub b: SimWire,
}

impl SimPair {
    pub fn new() -> Self {
        Self::lossy(None)
    }

    pub fn lossy(drop_every: Option<u32>) -> Self {
        let ab = Arc::new(Mutex::new(VecDeque::new()));
        let ba = Arc::new(Mutex::new(VecDeque::new()));
        let stop = Arc::new(AtomicBool::new(false));
        Self {
            a: SimWire {
                tx: ab.clone(),
                rx: ba.clone(),
                stop: stop.clone(),
                drop_every,
                sent: 0,
            },
            b: SimWire {
                tx: ba,
                rx: ab,
                stop,
                drop_every,
                sent: 0,
            },
        }
    }
}

impl Default for SimPair {
    fn default() -> Self {
        Self::new()
    }
}

impl Wire for SimWire {
    fn send_blob(&mut self, bytes: &[u8], _timeout: Duration) -> Result<(), Error> {
        if self.stop.load(Ordering::SeqCst) {
            return Err(Error::Interrupted);
        }
        if bytes.len() > SIM_MAX_BLOB {
            return Err(Error::TooLarge(bytes.len()));
        }
        self.sent = self.sent.wrapping_add(1);
        if let Some(n) = self.drop_every {
            if n > 0 && self.sent.is_multiple_of(n) {
                return Ok(());
            }
        }
        let mut queue = self
            .tx
            .lock()
            .map_err(|_| Error::Channel("sim lock".into()))?;
        if queue.len() >= SIM_QUEUE_CAPACITY {
            return Err(Error::Channel("simulation queue full".into()));
        }
        queue.push_back(bytes.to_vec());
        Ok(())
    }

    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::InvalidInput("timeout too large".into()))?;
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(Error::Interrupted);
            }
            if let Some(msg) = self
                .rx
                .lock()
                .map_err(|_| Error::Channel("sim lock".into()))?
                .pop_front()
            {
                return Ok(msg);
            }
            if Instant::now() >= deadline {
                return Err(Error::Timeout);
            }
            thread::sleep(Duration::from_millis(1));
        }
    }

    fn interrupt(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

pub struct SimSocket {
    sock: UnixDatagram,
    peer: PathBuf,
    stop: Arc<AtomicBool>,
    mine: PathBuf,
    dev: u64,
    ino: u64,
}

impl SimSocket {
    pub fn bind(role: Role, base: &str, stop: Arc<AtomicBool>) -> Result<Self, Error> {
        let base = Path::new(base);
        let parent = base
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| {
                Error::InvalidInput("simulation path requires a private parent directory".into())
            })?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        let meta = std::fs::symlink_metadata(parent)?;
        if !meta.is_dir() || meta.permissions().mode() & 0o077 != 0 {
            return Err(Error::InvalidInput(format!(
                "simulation directory {} must be private (mode 0700)",
                parent.display()
            )));
        }
        let suffix = |suffix: &str| {
            let mut p = base.as_os_str().to_os_string();
            p.push(suffix);
            PathBuf::from(p)
        };
        let (mine, peer) = match role {
            Role::A => (suffix(".a.sock"), suffix(".b.sock")),
            Role::B => (suffix(".b.sock"), suffix(".a.sock")),
        };
        let sock = UnixDatagram::bind(&mine)
            .map_err(|e| Error::Io(format!("bind {}: {e}", mine.display())))?;
        let meta = std::fs::symlink_metadata(&mine)?;
        let socket = Self {
            sock,
            peer,
            stop,
            mine,
            dev: meta.dev(),
            ino: meta.ino(),
        };
        std::fs::set_permissions(&socket.mine, std::fs::Permissions::from_mode(0o600))?;
        socket.sock.set_nonblocking(true)?;
        Ok(socket)
    }
}

impl Drop for SimSocket {
    fn drop(&mut self) {
        if let Ok(meta) = std::fs::symlink_metadata(&self.mine) {
            if meta.dev() == self.dev && meta.ino() == self.ino {
                let _ = std::fs::remove_file(&self.mine);
            }
        }
    }
}

impl Wire for SimSocket {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        if bytes.len() > SIM_MAX_BLOB {
            return Err(Error::TooLarge(bytes.len()));
        }
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::InvalidInput("timeout too large".into()))?;
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(Error::Interrupted);
            }
            match self.sock.send_to(bytes, &self.peer) {
                Ok(_) => return Ok(()),
                Err(e)
                    if e.kind() == ErrorKind::NotFound
                        || e.kind() == ErrorKind::ConnectionRefused =>
                {
                    if Instant::now() >= deadline {
                        return Err(Error::Timeout);
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) if is_again(&e) => {
                    if Instant::now() >= deadline {
                        return Err(Error::Timeout);
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(Error::Io(e.to_string())),
            }
        }
    }

    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::InvalidInput("timeout too large".into()))?;
        let mut buf = vec![0u8; 2048];
        loop {
            if self.stop.load(Ordering::SeqCst) {
                return Err(Error::Interrupted);
            }
            match self.sock.recv_from(&mut buf) {
                Ok((n, _)) => return Ok(buf[..n].to_vec()),
                Err(e) if is_again(&e) => {
                    if Instant::now() >= deadline {
                        return Err(Error::Timeout);
                    }
                    thread::sleep(Duration::from_millis(2));
                }
                Err(e) => return Err(Error::Io(e.to_string())),
            }
        }
    }

    fn interrupt(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn is_again(e: &io::Error) -> bool {
    matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::Interrupted)
}
