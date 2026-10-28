mod ui;
mod worker;

use std::io::{self, Read};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ccc_app::escape_controls;
use ccc_channel::{
    open_phy, probe_payload_len, random_probe_payload, run_probe_receiver, run_probe_sender,
    run_silent_control,
};
use ccc_core::{
    AppKind, ChannelMode, Error, Role, RuntimeConfig, DEFAULT_PROBE_BITS, DEFAULT_PROBE_TRIALS,
};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "cpu-cache-chat",
    version,
    long_version = version_info(),
    about = "Two isolated clients over a CPU-cache or CPU-contention channel"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    Chat(ClientArgs),
    Game(GameArgs),
    Probe(ProbeArgs),
    Bench(BenchArgs),
    InspectIsolation,
}

#[derive(clap::Args, Debug)]
struct ClientArgs {
    #[arg(long, value_enum)]
    role: RoleArg,
    #[arg(long, value_enum)]
    channel: ChannelArg,
    /// Labeled software transport. Not the isolated hardware demo.
    #[arg(long)]
    sim: bool,
    #[arg(long)]
    sim_path: Option<String>,
    /// Write versioned semantic game evidence as JSONL.
    #[arg(long)]
    events_path: Option<String>,
    /// Line-oriented input/output. EOF leaves the session running; enter /q to quit.
    #[arg(long)]
    plain: bool,
    #[arg(long, default_value = "5")]
    slot_ms: u64,
}

#[derive(clap::Args, Debug)]
struct GameArgs {
    #[command(flatten)]
    client: ClientArgs,
    #[arg(long)]
    agent: bool,
    #[arg(long)]
    agent_cmd: Option<String>,
}

#[derive(clap::Args, Debug)]
struct ProbeArgs {
    #[arg(long, value_enum)]
    role: RoleArg,
    #[arg(long, value_enum)]
    channel: ChannelArg,
    #[arg(long, default_value_t = DEFAULT_PROBE_TRIALS)]
    trials: u32,
    #[arg(long, default_value_t = DEFAULT_PROBE_BITS)]
    bits: u32,
    #[arg(long, default_value = "50")]
    slot_ms: u64,
    /// Listen without a sender and report whether a frame was falsely decoded.
    #[arg(long)]
    silent: bool,
    #[arg(long, default_value = "5")]
    silent_seconds: u64,
}

#[derive(clap::Args, Debug)]
struct BenchArgs {
    #[arg(long, value_enum)]
    role: RoleArg,
    #[arg(long, value_enum)]
    channel: ChannelArg,
    #[arg(long, default_value = "50")]
    slot_ms: u64,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum RoleArg {
    A,
    B,
}

impl From<RoleArg> for Role {
    fn from(v: RoleArg) -> Self {
        match v {
            RoleArg::A => Role::A,
            RoleArg::B => Role::B,
        }
    }
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ChannelArg {
    Cache,
    Cpu,
}

impl ChannelArg {
    fn mode(self, sim: bool) -> ChannelMode {
        if sim {
            ChannelMode::Sim
        } else {
            match self {
                ChannelArg::Cache => ChannelMode::Cache,
                ChannelArg::Cpu => ChannelMode::Cpu,
            }
        }
    }
}

fn version_info() -> &'static str {
    static VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    VERSION.get_or_init(|| {
        format!(
            "{}\nprotocol={}\nsource={}",
            env!("CARGO_PKG_VERSION"),
            ccc_core::PROTOCOL_VERSION,
            option_env!("CCC_BUILD_REVISION").unwrap_or("unknown")
        )
    })
}

fn main() {
    if let Err(e) = run() {
        eprintln!(
            "cpu-cache-chat: {}",
            ccc_app::safe_diagnostic(&e.to_string())
        );
        std::process::exit(1);
    }
}

fn run() -> Result<(), Error> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Chat(args) => client(AppKind::Chat, args, false, None),
        Cmd::Game(args) => client(AppKind::Game, args.client, args.agent, args.agent_cmd),
        Cmd::Probe(args) => probe(args),
        Cmd::Bench(args) => bench(args),
        Cmd::InspectIsolation => inspect_isolation(),
    }
}

fn client(
    kind: AppKind,
    args: ClientArgs,
    agent: bool,
    agent_cmd: Option<String>,
) -> Result<(), Error> {
    let mut cfg = RuntimeConfig::new(args.role.into(), args.channel.mode(args.sim), kind);
    cfg.plain = args.plain;
    cfg.agent = agent;
    cfg.agent_cmd = agent_cmd;
    cfg.slot = Duration::from_millis(args.slot_ms);
    cfg.sim_path = args.sim_path;
    cfg.validate()?;

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc::set_handler(move || {
            stop.store(true, Ordering::SeqCst);
        });
    }

    let (commands, updates, worker) = worker::start(cfg.clone(), stop.clone(), args.events_path);
    let result = if cfg.plain {
        plain_loop(&cfg, stop.clone(), commands, updates)
    } else {
        ui::run(cfg, stop.clone(), commands, updates)
    };
    stop.store(true, Ordering::SeqCst);
    if worker.join().is_err() {
        return Err(Error::Io("worker panicked".into()));
    }
    result
}

fn plain_loop(
    cfg: &RuntimeConfig,
    stop: Arc<AtomicBool>,
    commands: std::sync::mpsc::SyncSender<worker::Command>,
    updates: worker::Updates,
) -> Result<(), Error> {
    println!(
        "cpu-cache-chat / {} / {} mode: {}",
        cfg.role,
        cfg.role.mark().glyph(),
        if cfg.channel.is_physical() {
            cfg.channel.to_string()
        } else {
            "SIMULATION (software only)".into()
        }
    );
    println!(
        "connecting; enter {} or /q to quit",
        if cfg.kind == AppKind::Chat {
            "messages"
        } else {
            "squares 1-9"
        }
    );
    let input_stop = stop.clone();
    std::thread::spawn(move || {
        let mut line = Vec::new();
        let mut overflow = false;
        let mut next_id = 0u64;
        for byte in io::stdin().lock().bytes() {
            let Ok(byte) = byte else { break };
            if byte != b'\n' {
                if line.len() < ccc_core::MAX_CHAT_BYTES {
                    line.push(byte);
                } else {
                    overflow = true;
                }
                continue;
            }
            if overflow {
                eprintln!("input too large");
            } else if let Ok(text) = String::from_utf8(std::mem::take(&mut line)) {
                if text.trim() == "/q" {
                    input_stop.store(true, Ordering::SeqCst);
                    break;
                }
                match commands.try_send(worker::Command {
                    id: next_id,
                    line: text,
                }) {
                    Ok(()) => {
                        next_id = next_id.wrapping_add(1);
                    }
                    Err(_) => eprintln!("command queue full or session unavailable"),
                }
            }
            line.clear();
            overflow = false;
        }
    });
    let mut previous = String::new();
    loop {
        match updates.try_recv() {
            Ok(worker::Update::Note(note)) => println!("{}", escape_controls(&note)),
            Ok(worker::Update::Snapshot(s)) => {
                let view = format!(
                    "link: {}\n{}{}",
                    s.link,
                    s.board.as_deref().unwrap_or(""),
                    s.outcome
                        .as_ref()
                        .map(|o| format!("\n{o}"))
                        .unwrap_or_default()
                );
                if view != previous {
                    println!("{view}");
                    previous = view;
                }
            }
            Ok(worker::Update::Accepted(_)) => {}
            Ok(worker::Update::Rejected(_, reason)) => {
                eprintln!("rejected: {}", escape_controls(&reason))
            }
            Ok(worker::Update::Fatal(e)) => return Err(e),
            Ok(worker::Update::Delivery(seq, state)) => println!("seq {seq}: {state}"),
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                return Err(Error::Io("worker disconnected".into()))
            }
        }
    }
    stop.store(true, Ordering::SeqCst);
    println!("stopped");
    Ok(())
}

fn measurement_stop() -> Result<Arc<AtomicBool>, Error> {
    let stop = Arc::new(AtomicBool::new(false));
    let flag = stop.clone();
    ctrlc::set_handler(move || flag.store(true, Ordering::SeqCst))
        .map_err(|e| Error::Io(format!("installing interrupt handler: {e}")))?;
    Ok(stop)
}

fn probe(args: ProbeArgs) -> Result<(), Error> {
    let mode = args.channel.mode(false);
    let slot = Duration::from_millis(args.slot_ms);
    ccc_core::validate_slot(slot)?;
    probe_payload_len(args.bits)?;
    if args.trials != 1 {
        return Err(Error::InvalidInput(
            "probe supports exactly --trials 1".into(),
        ));
    }
    if args.silent_seconds == 0 || args.silent_seconds > 3600 {
        return Err(Error::InvalidInput("silent seconds must be 1..3600".into()));
    }
    let stop = measurement_stop()?;
    let role = Role::from(args.role);
    if args.silent {
        if role.is_initiator() || args.silent_seconds == 0 {
            return Err(Error::InvalidInput(
                "silent control requires role b and a positive duration".into(),
            ));
        }
        let report = run_silent_control(
            mode,
            role,
            slot,
            stop,
            Duration::from_secs(args.silent_seconds),
        )?;
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
        return if report.has_failures() {
            Err(Error::Calibration(
                "silent control decoded a frame or failed".into(),
            ))
        } else {
            Ok(())
        };
    }
    if role.is_initiator() {
        // Sender-only OS entropy. The receiver never receives expected bytes.
        let payload = random_probe_payload(args.bits)?;
        run_probe_sender(mode, args.bits, slot, stop, &payload)?;
        let payload_hex: String = payload.iter().map(|byte| format!("{byte:02x}")).collect();
        println!(
            "{}",
            serde_json::json!({
                "schema_version": 1, "role": "A", "mode": mode,
                "requested_bits": args.bits, "sent_bytes": payload.len(), "payload_hex": payload_hex,
            })
        );
        return Ok(());
    }
    let report = run_probe_receiver(mode, role, args.trials, args.bits, slot, stop, None)?;
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if report.has_failures() {
        return Err(Error::Calibration(
            "probe did not decode a valid frame; see JSON report".into(),
        ));
    }
    Ok(())
}

fn bench(args: BenchArgs) -> Result<(), Error> {
    let mode = args.channel.mode(false);
    let slot = Duration::from_millis(args.slot_ms);
    ccc_core::validate_slot(slot)?;
    let stop = measurement_stop()?;
    let mut phy = open_phy(mode, slot, stop)?;
    let send = Role::from(args.role).is_initiator();
    println!(
        "bench mode={} role={} slot_ms={} send={send}",
        mode,
        Role::from(args.role),
        args.slot_ms
    );
    let samples = phy.bench_pattern(send)?;
    if samples.is_empty() {
        println!("{{\"role\":\"A\",\"sent\":true}}");
        return Ok(());
    }
    let ns: Vec<u128> = samples.iter().map(|d| d.as_nanos()).collect();
    let mean = ns.iter().sum::<u128>() / ns.len().max(1) as u128;
    let min = *ns.iter().min().unwrap_or(&0);
    let max = *ns.iter().max().unwrap_or(&0);
    println!(
        "n={} mean_ns={mean} min_ns={min} max_ns={max} ratio={:.2} samples={ns:?}",
        ns.len(),
        if min == 0 {
            0.0
        } else {
            max as f64 / min as f64
        }
    );
    Ok(())
}

fn inspect_isolation() -> Result<(), Error> {
    println!("cpu-cache-chat isolation self-check (container view)");
    let mut bad = Vec::new();
    println!("pid1: {}", read("/proc/1/comm").trim());
    if path_exists("/var/run/docker.sock") || path_exists("/run/docker.sock") {
        bad.push("docker.sock is visible");
    }
    if path_exists("/dev/kmsg") {
        // not fatal
    }
    match std::fs::read_to_string("/proc/net/unix") {
        Ok(s) if s.lines().count() > 2 => {
            println!("unix sockets present (loopback/internal only is ok)");
        }
        _ => {}
    }
    if let Ok(s) = std::fs::read_to_string("/proc/net/tcp") {
        let established = s.lines().skip(1).count();
        println!("proc/net/tcp entries: {established} (should be none for peer links)");
    }
    println!("hostname: {}", hostname());
    if !bad.is_empty() {
        return Err(Error::Isolation(bad.join("; ")));
    }
    println!("no docker.sock; this is not proof that the host launch flags were correct");
    println!("run scripts/inspect-isolation.sh on the host for the real policy");
    Ok(())
}

fn read(path: &str) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

fn path_exists(p: &str) -> bool {
    std::path::Path::new(p).exists()
}

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .or_else(|_| std::fs::read_to_string("/proc/sys/kernel/hostname"))
        .unwrap_or_else(|_| "unknown".into())
        .trim()
        .to_string()
}

#[allow(dead_code)]
fn _escape(s: &str) -> String {
    escape_controls(s)
}
