use ccc_app::agent::{AgentJob, AgentRequest, CommandSpec, Limits};
use std::time::{Duration, Instant};

fn request() -> AgentRequest {
    AgentRequest {
        session: 42,
        generation: 3,
        request_id: 9,
        board: "123456789".into(),
        turn: "X".into(),
        legal: (1..=9).collect(),
    }
}
fn run(script: &str, limits: Limits) -> Result<ccc_app::agent::AgentResult, ccc_core::Error> {
    AgentJob::spawn(
        CommandSpec::TrustedShell(script.into()),
        request(),
        limits,
        None,
    )
    .unwrap()
    .wait()
}
fn limits() -> Limits {
    Limits {
        timeout: Duration::from_secs(3),
        stdout_bytes: 200_000,
        stderr_bytes: 200_000,
    }
}

#[test]
fn drains_both_pipes_before_exit() {
    let result = run("head -c 100000 /dev/zero | tr '\\0' ' ' >&2; head -c 100000 /dev/zero | tr '\\0' ' '; printf '{\"square\":1,\"source\":\"pi\"}'", limits()).unwrap();
    assert_eq!(
        (
            result.session,
            result.generation,
            result.request_id,
            result.square
        ),
        (42, 3, 9, 1)
    );
    assert_eq!(result.stderr.len(), 100000);
}
#[test]
fn malformed_and_oversized_output_fail() {
    // Consume stdin first: a child that exits without reading can race the
    // request-body write and fail with EPIPE instead of the parse error.
    assert!(run("cat >/dev/null; printf nonsense", limits())
        .unwrap_err()
        .to_string()
        .contains("parse"));
    let mut small = limits();
    small.stderr_bytes = 1000;
    assert!(run("head -c 100000 /dev/zero >&2; sleep 10", small)
        .unwrap_err()
        .to_string()
        .contains("quota exceeded"));
    assert!(run(
        "cat >/dev/null; printf '{\"square\":1,\"source\":\"test\"}'",
        limits()
    )
    .is_err());
}
#[test]
fn timeout_and_cancel_are_bounded() {
    let mut short = limits();
    short.timeout = Duration::from_millis(100);
    let start = Instant::now();
    assert!(run("trap '' TERM; sleep 30", short)
        .unwrap_err()
        .to_string()
        .contains("timed out"));
    assert!(start.elapsed() < Duration::from_secs(2));
    let job = AgentJob::spawn(
        CommandSpec::TrustedShell("sleep 30".into()),
        request(),
        limits(),
        None,
    )
    .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    let start = Instant::now();
    job.cancel();
    assert!(job.wait().is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
}
#[test]
fn kills_descendant_on_timeout() {
    let path = std::env::temp_dir().join(format!("ccc-agent-child-{}", std::process::id()));
    let script = format!("sleep 30 & echo $! > {}; wait", path.display());
    let mut short = limits();
    short.timeout = Duration::from_millis(200);
    assert!(run(&script, short).is_err());
    let pid: i32 = std::fs::read_to_string(&path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert_descendant_dead(pid);
}
#[test]
fn closed_stdin_fails_bounded_and_kills_descendants() {
    let path = std::env::temp_dir().join(format!("ccc-agent-stdin-{}", std::process::id()));
    // The child spawns a descendant and exits without reading stdin. Depending
    // on the exit/write race the error is EPIPE on the request write or a parse
    // failure; either way it must be bounded and the guard must kill the group.
    let script = format!("sleep 30 & echo $! > {}; printf nonsense", path.display());
    let start = Instant::now();
    assert!(run(&script, limits()).is_err());
    assert!(start.elapsed() < Duration::from_secs(2));
    let pid: i32 = std::fs::read_to_string(&path)
        .unwrap()
        .trim()
        .parse()
        .unwrap();
    std::fs::remove_file(path).unwrap();
    assert_descendant_dead(pid);
}
/// A killed orphan may temporarily remain as a zombie until its parent reaps it.
fn assert_descendant_dead(pid: i32) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let exists = unsafe { libc::kill(pid, 0) } == 0;
        #[cfg(target_os = "linux")]
        let zombie = std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| s.contains(") Z "))
            .unwrap_or(false);
        #[cfg(not(target_os = "linux"))]
        let zombie = false;
        if !exists || zombie {
            break;
        }
        assert!(Instant::now() < deadline, "descendant {pid} survived");
        std::thread::sleep(Duration::from_millis(20));
    }
}
