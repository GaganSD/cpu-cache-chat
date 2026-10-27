mod ui;
mod worker;

use std::io::{self, BufRead};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ccc_app::escape_controls;
use ccc_channel::{open_phy, run_probe_receiver, run_probe_sender};
use ccc_core::{
    AppKind, ChannelMode, Error, Role, RuntimeConfig, DEFAULT_PROBE_BITS, DEFAULT_PROBE_TRIALS,
};
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser, Debug)]
#[command(
    name = "cpu-cache-chat",
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
    Bench(ProbeArgs),
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
    #[arg(long, default_value = "5")]
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

fn main() {
    if let Err(e) = run() {
        eprintln!("cpu-cache-chat: {e}");
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
    if args.sim {
        cfg.sim_path = Some(
            args.sim_path
                .unwrap_or_else(|| "/tmp/cpu-cache-chat.sim".into()),
        );
    }
    cfg.validate()?;

    let stop = Arc::new(AtomicBool::new(false));
    {
        let stop = stop.clone();
        let _ = ctrlc::set_handler(move || {
            stop.store(true, Ordering::SeqCst);
        });
    }

    let (commands, updates, worker) = worker::start(cfg.clone(), stop.clone());
    let result = if cfg.plain {
        plain_loop(&cfg, stop.clone(), commands, updates)
    } else {
        ui::run(cfg, stop.clone(), commands, updates)
    };
    stop.store(true, Ordering::SeqCst);
    let _ = worker.join();
    result
}

fn plain_loop(
    cfg: &RuntimeConfig,
    stop: Arc<AtomicBool>,
    commands: std::sync::mpsc::Sender<String>,
    updates: std::sync::mpsc::Receiver<worker::Update>,
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
        for line in io::stdin().lock().lines() {
            match line {
                Ok(line) if line.trim() == "/q" => {
                    input_stop.store(true, Ordering::SeqCst);
                    break;
                }
                Ok(line) => {
                    println!("queued: {}", escape_controls(&line));
                    if commands.send(line).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
    let mut previous = String::new();
    while !stop.load(Ordering::SeqCst) {
        match updates.recv_timeout(Duration::from_millis(50)) {
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
            Ok(worker::Update::Accepted) => {}
            Ok(worker::Update::Delivery(seq, state)) => println!("seq {seq}: {state}"),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    stop.store(true, Ordering::SeqCst);
    println!("stopped");
    Ok(())
}

fn probe(args: ProbeArgs) -> Result<(), Error> {
    let mode = args.channel.mode(false);
    if !mode.is_physical() {
        return Err(Error::InvalidInput(
            "probe requires a physical channel".into(),
        ));
    }
    let stop = Arc::new(AtomicBool::new(false));
    let slot = Duration::from_millis(args.slot_ms);
    let role = Role::from(args.role);
    // Receiver-blind payload: sender chooses bytes the receiver binary does not embed.
    let payload = if role.is_initiator() {
        let mut p = vec![0u8; (args.bits as usize).div_ceil(8).min(32)];
        for (i, b) in p.iter_mut().enumerate() {
            *b = ((i as u8).wrapping_mul(37)).wrapping_add(0x5A);
        }
        println!(
            "sender payload (keep this off the receiver argv): {:02x?}",
            p
        );
        run_probe_sender(mode, args.bits, slot, stop, &p)?;
        println!(
            "{{\"role\":\"A\",\"mode\":\"{}\",\"sent\":{}}}",
            mode,
            p.len()
        );
        return Ok(());
    } else {
        // Receiver does not know the sender bytes unless the operator pastes them later.
        run_probe_receiver(mode, role, args.trials, args.bits, slot, stop, None)?
    };
    println!("{}", serde_json::to_string_pretty(&payload).unwrap());
    Ok(())
}

fn bench(args: ProbeArgs) -> Result<(), Error> {
    let mode = args.channel.mode(false);
    let slot = Duration::from_millis(args.slot_ms);
    let stop = Arc::new(AtomicBool::new(false));
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
