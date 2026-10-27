//! Labeled simulation transport. Not a hardware channel.

use std::collections::VecDeque;
use std::io::{self, ErrorKind};
use std::os::unix::net::UnixDatagram;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ccc_core::{Error, Role};
use ccc_protocol::Wire;

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
        self.sent += 1;
        if let Some(n) = self.drop_every {
            if n > 0 && self.sent.is_multiple_of(n) {
                return Ok(());
            }
        }
        self.tx
            .lock()
            .map_err(|_| Error::Channel("sim lock".into()))?
            .push_back(bytes.to_vec());
        Ok(())
    }

    fn recv_blob(&mut self, timeout: Duration) -> Result<Vec<u8>, Error> {
        let deadline = Instant::now() + timeout;
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
}

impl SimSocket {
    pub fn bind(role: Role, base: &str, stop: Arc<AtomicBool>) -> Result<Self, Error> {
        let base = Path::new(base);
        if let Some(parent) = base.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        let mine = match role {
            Role::A => base.with_extension("a.sock"),
            Role::B => base.with_extension("b.sock"),
        };
        let peer = match role {
            Role::A => base.with_extension("b.sock"),
            Role::B => base.with_extension("a.sock"),
        };
        let _ = std::fs::remove_file(&mine);
        let sock = UnixDatagram::bind(&mine).map_err(|e| Error::Io(e.to_string()))?;
        sock.set_nonblocking(true)
            .map_err(|e| Error::Io(e.to_string()))?;
        Ok(Self { sock, peer, stop })
    }
}

impl Wire for SimSocket {
    fn send_blob(&mut self, bytes: &[u8], timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now() + timeout;
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
        let deadline = Instant::now() + timeout;
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
