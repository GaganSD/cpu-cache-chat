//! Independent, cancellable agent jobs. The caller owns session state and must
//! compare the returned identifiers and board generation before applying a move.
use ccc_core::Error;
use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Clone, Debug)]
pub struct AgentRequest {
    pub session: u32,
    pub generation: u64,
    pub request_id: u64,
    pub board: String,
    pub turn: String,
    pub legal: Vec<u8>,
}
#[derive(Debug)]
pub struct AgentResult {
    pub session: u32,
    pub generation: u64,
    pub request_id: u64,
    pub square: u8,
    pub stderr: Vec<u8>,
}
#[derive(Clone, Debug)]
pub enum CommandSpec {
    Executable {
        program: String,
        args: Vec<String>,
    },
    /// Executes trusted local code, not a sandbox. Never construct from remote input.
    TrustedShell(String),
}
impl CommandSpec {
    /// Compatibility with the existing expert command override.
    pub fn legacy(cmd: Option<&str>) -> Result<Self, Error> {
        match cmd {
            Some(s) if s.trim().is_empty() => Err(Error::Agent("empty agent command".into())),
            Some(s) if s.chars().any(char::is_whitespace) => {
                Ok(Self::TrustedShell(format!("exec {s}")))
            }
            Some(s) => Ok(Self::Executable {
                program: s.into(),
                args: vec!["/opt/ccc/agent/src/play.mjs".into()],
            }),
            None => Ok(Self::Executable {
                program: "node".into(),
                args: vec!["/opt/ccc/agent/src/play.mjs".into()],
            }),
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(120),
            stdout_bytes: 4096,
            stderr_bytes: 1024 * 1024,
        }
    }
}
pub struct AgentJob {
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<AgentResult, Error>>,
    thread: Option<JoinHandle<()>>,
}
impl AgentJob {
    pub fn spawn(
        command: CommandSpec,
        request: AgentRequest,
        limits: Limits,
        external_cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Self, Error> {
        if request.board.len() > 64
            || request.turn.len() > 8
            || request.legal.len() > 9
            || limits.timeout.is_zero()
            || limits.stdout_bytes == 0
            || limits.stderr_bytes == 0
        {
            return Err(Error::Agent("invalid agent request or limits".into()));
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("agent-supervisor".into())
            .spawn(move || {
                let result = run(command, request, limits, &flag, external_cancel.as_deref());
                let _ = tx.send(result);
            })
            .map_err(|e| Error::Agent(e.to_string()))?;
        Ok(Self {
            cancel,
            rx,
            thread: Some(thread),
        })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }
    pub fn try_result(&self) -> Result<Option<Result<AgentResult, Error>>, mpsc::TryRecvError> {
        match self.rx.try_recv() {
            Ok(v) => Ok(Some(v)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(e) => Err(e),
        }
    }
    pub fn wait(mut self) -> Result<AgentResult, Error> {
        let result = self
            .rx
            .recv()
            .map_err(|_| Error::Agent("agent supervisor failed".into()))?;
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        result
    }
}
impl Drop for AgentJob {
    fn drop(&mut self) {
        self.cancel();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}
struct Guard(Child);
impl Drop for Guard {
    fn drop(&mut self) {
        // The group is created exclusively for this child. Kill it even after the
        // leader exits, because its descendants can still hold pipe handles.
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}
fn nonblocking<T: std::os::fd::AsRawFd>(fd: &T) -> std::io::Result<()> {
    let fd = fd.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}
fn drain<R: Read>(
    mut r: R,
    limit: usize,
    done: Arc<AtomicBool>,
    exceeded: Arc<AtomicBool>,
) -> std::io::Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = [0; 8192];
    loop {
        match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if n > limit.saturating_sub(out.len()) {
                    exceeded.store(true, Ordering::SeqCst);
                    break;
                }
                out.extend_from_slice(&buf[..n]);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if done.load(Ordering::SeqCst) {
                    break;
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(out)
}
fn run(
    spec: CommandSpec,
    req: AgentRequest,
    limits: Limits,
    cancel: &AtomicBool,
    external: Option<&AtomicBool>,
) -> Result<AgentResult, Error> {
    let mut command = match spec {
        CommandSpec::Executable { program, args } => {
            let mut c = Command::new(program);
            c.args(args);
            c
        }
        CommandSpec::TrustedShell(script) => {
            let mut c = Command::new("sh");
            c.arg("-c").arg(script);
            c
        }
    };
    command
        .env_remove("CCC_ALLOW_FIRST_LEGAL")
        .process_group(0)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = Guard(command.spawn().map_err(|e| Error::Agent(e.to_string()))?);
    let mut stdin = child
        .0
        .stdin
        .take()
        .ok_or_else(|| Error::Agent("missing stdin".into()))?;
    let stdout = child
        .0
        .stdout
        .take()
        .ok_or_else(|| Error::Agent("missing stdout".into()))?;
    let stderr = child
        .0
        .stderr
        .take()
        .ok_or_else(|| Error::Agent("missing stderr".into()))?;
    nonblocking(&stdin)
        .and_then(|_| nonblocking(&stdout))
        .and_then(|_| nonblocking(&stderr))
        .map_err(|e| Error::Agent(e.to_string()))?;
    let done = Arc::new(AtomicBool::new(false));
    let exceeded = Arc::new(AtomicBool::new(false));
    let d = done.clone();
    let x = exceeded.clone();
    let out = thread::spawn(move || drain(stdout, limits.stdout_bytes, d, x));
    let d = done.clone();
    let x = exceeded.clone();
    let err = thread::spawn(move || drain(stderr, limits.stderr_bytes, d, x));
    let body =
        serde_json::json!({"board": req.board, "turn": req.turn, "legal": req.legal}).to_string();
    let deadline = Instant::now()
        .checked_add(limits.timeout)
        .ok_or_else(|| Error::Agent("invalid deadline".into()));
    let result = (|| {
        let deadline = deadline?;
        let mut input = body.as_bytes();
        loop {
            if cancel.load(Ordering::SeqCst) || external.is_some_and(|v| v.load(Ordering::SeqCst)) {
                return Err(Error::Interrupted);
            }
            if exceeded.load(Ordering::SeqCst) {
                return Err(Error::Agent(
                    "agent output quota exceeded; evidence incomplete".into(),
                ));
            }
            if Instant::now() >= deadline {
                return Err(Error::Agent("agent timed out".into()));
            }
            if !input.is_empty() {
                match stdin.write(input) {
                    Ok(0) => return Err(Error::Agent("agent stdin closed".into())),
                    Ok(n) => input = &input[n..],
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(Error::Agent(format!("agent stdin: {e}"))),
                }
            } else {
                drop(stdin);
                break;
            }
            thread::sleep(Duration::from_millis(5));
        }
        loop {
            if cancel.load(Ordering::SeqCst) || external.is_some_and(|v| v.load(Ordering::SeqCst)) {
                return Err(Error::Interrupted);
            }
            if exceeded.load(Ordering::SeqCst) {
                return Err(Error::Agent(
                    "agent output quota exceeded; evidence incomplete".into(),
                ));
            }
            if Instant::now() >= deadline {
                return Err(Error::Agent("agent timed out".into()));
            }
            if let Some(status) = child
                .0
                .try_wait()
                .map_err(|e| Error::Agent(e.to_string()))?
            {
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(5));
        }
    })();
    drop(child);
    done.store(true, Ordering::SeqCst);
    let stdout = out
        .join()
        .map_err(|_| Error::Agent("stdout reader panicked".into()))?
        .map_err(|e| Error::Agent(e.to_string()))?;
    let stderr = err
        .join()
        .map_err(|_| Error::Agent("stderr reader panicked".into()))?
        .map_err(|e| Error::Agent(e.to_string()))?;
    if exceeded.load(Ordering::SeqCst) {
        return Err(Error::Agent(
            "agent output quota exceeded; evidence incomplete".into(),
        ));
    }
    let status = result?;
    if !status.success() {
        return Err(Error::Agent(format!(
            "agent exit {status}: {}",
            crate::escape_controls(&String::from_utf8_lossy(&stderr))
        )));
    }
    let square = super::parse_square(
        std::str::from_utf8(&stdout).map_err(|e| Error::Agent(e.to_string()))?,
    )?;
    if !req.legal.contains(&square) {
        return Err(Error::Agent("agent returned illegal square".into()));
    }
    Ok(AgentResult {
        session: req.session,
        generation: req.generation,
        request_id: req.request_id,
        square,
        stderr,
    })
}
