mod ui;

use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ccc_app::{escape_controls, Session};
use ccc_channel::{run_probe_receiver, run_probe_sender};
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
    #[arg(long)]
    plain: bool,
    #[arg(long, default_value = "2")]
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
    #[arg(long, default_value = "2")]
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

    let mut session = Session::open(cfg.clone(), stop.clone())?;
    if cfg.plain {
        plain_loop(&mut session, &cfg, stop)
    } else {
        ui::run(session, cfg, stop)
    }
}

fn plain_loop(
    session: &mut Session<Box<dyn ccc_protocol::Wire>>,
    cfg: &RuntimeConfig,
    stop: Arc<AtomicBool>,
) -> Result<(), Error> {
    println!(
        "cpu-cache-chat / {} / {}  mode: {}",
        cfg.role,
        cfg.role.mark().glyph(),
        cfg.channel
    );
    if !cfg.channel.is_physical() {
        println!("WARNING: simulation transport — not a hardware channel");
    }
    println!("connecting…");
    io::stdout().flush().ok();
    for note in session.connect()? {
        println!("{note}");
    }
    if cfg.kind == AppKind::Game && cfg.role.is_initiator() {
        session.request_start()?;
    }
    while !stop.load(Ordering::SeqCst) {
        if cfg.kind == AppKind::Game && session.our_turn() {
            if cfg.agent {
                match session.try_agent_move() {
                    Ok(Some(sq)) => match session.queue_move(sq) {
                        Ok(_) => println!("agent move {sq}"),
                        Err(e) => println!("agent move rejected: {e}"),
                    },
                    Ok(None) => {}
                    Err(e) => println!("agent error: {e}"),
                }
            } else {
                println!("{}", session.snapshot().board.unwrap_or_default());
                print!("your move 1-9 (or q): ");
                io::stdout().flush().ok();
                if let Some(line) = try_read_line(Duration::from_millis(50)) {
                    if line == "q" {
                        break;
                    }
                    if let Ok(sq) = line.parse::<u8>() {
                        match session.queue_move(sq) {
                            Ok(_) => {}
                            Err(e) => println!("{e}"),
                        }
                    }
                }
            }
        }
        for note in session.tick(Duration::from_millis(80))? {
            println!("{note}");
        }
        if cfg.kind == AppKind::Chat {
            if let Some(line) = try_read_line(Duration::from_millis(10)) {
                if line == "/q" {
                    break;
                }
                match session.queue_chat(line) {
                    Ok(_) => {}
                    Err(e) => println!("{e}"),
                }
            }
        }
        let snap = session.snapshot();
        if let Some(end) = &snap.outcome {
            println!("{}", snap.board.unwrap_or_default());
            println!("{end}");
            break;
        }
    }
    session.interrupt();
    println!("stopped");
    Ok(())
}

fn try_read_line(timeout: Duration) -> Option<String> {
    use std::io::IsTerminal;
    if !io::stdin().is_terminal() {
        let mut s = String::new();
        return match io::stdin().read_line(&mut s) {
            Ok(0) => None,
            Ok(_) => Some(s.trim().to_string()),
            Err(_) => None,
        };
    }
    // Non-blocking stdin is awkward in portable Rust; the TUI path handles input.
    // In plain mode we poll with a short sleep only when piped input is absent.
    let _ = timeout;
    None
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
