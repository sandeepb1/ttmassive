mod agentlog;
mod api;
mod assets;
mod auth;
mod chat;
mod cli;
mod config;
mod control;
mod daemon;
mod gitinfo;
mod limits;
mod modelmenu;
mod modes;
mod notify;
mod ops;
mod placeholder;
mod pty;
mod question;
mod resume;
mod screen;
mod scrollback;
mod server;
mod subagents;
mod sessions;
mod suggest;
mod tmux;
mod tunnel;
mod uploads;
mod usage;
mod ws;

use std::{io::IsTerminal, path::PathBuf};

use anyhow::Result;
use clap::{Parser, Subcommand};

pub struct AppState {
    pub cfg: config::Config,
    pub auth: auth::Auth,
    pub data_dir: PathBuf,
    /// Web Push: raw base64url VAPID private key and the subscribed devices.
    pub vapid_private: String,
    pub subscriptions: std::sync::Mutex<Vec<web_push::SubscriptionInfo>>,
    /// Secret that hook processes present to `/api/internal/notify`.
    pub notify_token: String,
    pub shim_dir: PathBuf,
    /// Set by the control socket to ask `serve` to shut down gracefully.
    pub shutdown: tokio::sync::Notify,
    pub started: std::time::Instant,
    /// Set once shutdown begins so vanishing sessions are kept, not forgotten.
    pub stopping: std::sync::atomic::AtomicBool,
    pub(crate) last_alert: dashmap::DashMap<(String, notify::Kind), std::time::Instant>,
    /// Everything remembered about each session (how it was started, agent ids, notes).
    /// tmux decides which of them are running; the rest show as "stopped".
    pub sessions: sessions::Registry,
    /// Plan-limit usage for Claude Code and Codex, and per-session token totals.
    pub usage: usage::Usage,
    /// Each session's conversation log, parsed for the chat view.
    pub chat: chat::ChatCache,
    /// Held while keys are being typed into an agent's menus, so two requests never interleave.
    pub driver: tokio::sync::Mutex<()>,
    /// The public address ssh gave us, when `--tunnel` is on.
    pub tunnel: std::sync::Mutex<tunnel::TunnelState>,
    /// What each Claude session last said about itself through its status line, by session uid.
    pub statusline: dashmap::DashMap<String, usage::StatusLive>,
}

#[derive(Parser)]
#[command(name = "ttm", version, about = "Remote terminal for AI coding agents")]
struct Cli {
    /// Config file (default: ~/.config/ttm/config.toml).
    #[arg(long, short, global = true, env = "TTM_CONFIG")]
    config: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

/// Whether to open a public HTTPS address for the web app (see `tunnel` in the config).
#[derive(clap::Args, Clone, Default)]
struct TunnelArgs {
    /// Reach ttm from anywhere through an ssh tunnel: `localhost.run` (default) or `pinggy`.
    #[arg(long, value_name = "SERVICE", num_args = 0..=1, default_missing_value = "localhost.run")]
    tunnel: Option<tunnel::Provider>,
    /// Do not open a tunnel, even if the config asks for one.
    #[arg(long, conflicts_with = "tunnel")]
    no_tunnel: bool,
}

impl TunnelArgs {
    /// The flags beat the config file.
    fn apply(&self, cfg: &mut config::Config) {
        if self.no_tunnel {
            cfg.tunnel = None;
        } else if self.tunnel.is_some() {
            cfg.tunnel = self.tunnel;
        }
    }

    /// The same choice as arguments for the `serve` process that `start` launches.
    fn to_args(&self) -> Vec<String> {
        if self.no_tunnel {
            vec!["--no-tunnel".into()]
        } else if let Some(p) = self.tunnel {
            vec![format!("--tunnel={}", p.name())]
        } else {
            Vec::new()
        }
    }
}

#[derive(Subcommand)]
enum Cmd {
    // ---- the daemon
    /// Run the server in the foreground (what service managers run).
    Serve(TunnelArgs),
    /// Start the server in the background.
    Start(TunnelArgs),
    /// Stop the background server. Sessions keep running in tmux unless --kill-sessions.
    Stop {
        /// Also stop tmux and every agent in it.
        #[arg(long)]
        kill_sessions: bool,
        /// Stop even if a service manager will restart it.
        #[arg(long)]
        force: bool,
    },
    /// Stop and start the background server.
    Restart(TunnelArgs),
    /// Show whether the server is running.
    Status,
    /// Show the server log.
    Logs {
        #[arg(short = 'n', long, default_value_t = 50)]
        lines: usize,
        /// Keep printing new lines.
        #[arg(short, long)]
        follow: bool,
    },
    /// Install a systemd (Linux) or launchd (macOS) user service for the server.
    InstallService {
        /// Also start it now.
        #[arg(long)]
        now: bool,
    },
    /// Remove the service installed by `install-service`.
    UninstallService,
    /// Print an Argon2 hash for the config file (reads the password from stdin).
    HashPassword,

    // ---- sessions
    /// List sessions.
    #[command(alias = "list")]
    Ls {
        #[arg(long)]
        json: bool,
    },
    /// List presets and groups.
    Presets,
    /// Start a session from a preset (or `shell`).
    New {
        /// Preset name, or `shell` for a plain shell.
        target: String,
        #[arg(long)]
        name: Option<String>,
        /// Start Claude Code or Codex with all permission checks skipped (Claude: `IS_SANDBOX=1` and
        /// `--dangerously-skip-permissions`; Codex: `--yolo`). Only for machines where that is safe.
        #[arg(long)]
        yolo: bool,
    },
    /// Remove sessions and everything remembered about them.
    Kill { ids: Vec<String> },
    /// Rename a session.
    Rename { id: String, new: String },
    /// Bring a stopped or exited session back, resuming Claude/Codex conversations.
    #[command(alias = "resume")]
    Restore {
        id: String,
        /// Start a fresh conversation instead of resuming.
        #[arg(long)]
        fresh: bool,
    },
    /// Attach this terminal to a running session.
    Attach { id: String },

    // ---- control
    /// Print a session's current screen (or its saved output if stopped).
    Output {
        id: String,
        #[arg(short = 'n', long, default_value_t = 200)]
        lines: usize,
    },
    /// Show what a session is doing and any question it is asking.
    Screen {
        id: String,
        #[arg(long)]
        json: bool,
    },
    /// Type into a session without attaching.
    Send {
        id: String,
        /// Text to type.
        text: Vec<String>,
        /// Named keys to press after the text (Enter, Esc, Tab, Up, Down, C-c, ...). Repeatable.
        #[arg(long = "key", short = 'k')]
        keys: Vec<String>,
        /// Press Enter at the end.
        #[arg(long)]
        enter: bool,
    },
    /// Answer the question on a session's screen.
    Answer {
        id: String,
        /// `approve`, `deny`, `interrupt`, or a choice number.
        choice: String,
        /// Only answer if the question still has this id (see `ttm screen`).
        #[arg(long)]
        prompt_id: Option<String>,
    },
    /// Set a note, tags or the pin on a session.
    Meta {
        id: String,
        #[arg(long)]
        note: Option<String>,
        #[arg(long = "tag")]
        tags: Vec<String>,
        #[arg(long)]
        clear_tags: bool,
        #[arg(long, conflicts_with = "unpin")]
        pin: bool,
        #[arg(long)]
        unpin: bool,
    },
    /// Work with groups of presets.
    Group {
        #[command(subcommand)]
        cmd: GroupCmd,
    },

    /// Called by agent hooks inside a ttm session; reports to the server.
    #[command(hide = true)]
    Notify {
        #[arg(long, value_enum)]
        kind: notify::HookKind,
        /// Read the hook's JSON payload from stdin (Claude Code).
        #[arg(long)]
        stdin: bool,
        /// Payload passed as the last argument (Codex).
        payload: Option<String>,
    },

    /// Claude Code's status line: prints plan-limit usage and reports it to the server.
    #[command(hide = true)]
    Statusline,
}

#[derive(Subcommand)]
enum GroupCmd {
    /// Start every preset in a group.
    Start { name: String },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs go to stderr so CLI output on stdout stays clean; no colour codes in log files.
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .init();
    let cli = Cli::parse();
    let cfg_path = cli.config.as_deref();

    // Commands that need no config or daemon.
    match &cli.cmd {
        Cmd::HashPassword => {
            let pw = read_password()?;
            anyhow::ensure!(
                pw.chars().count() >= auth::MIN_PASSWORD_LEN,
                "use at least {} characters: with `--tunnel` this password is all that stands between the internet and your terminals",
                auth::MIN_PASSWORD_LEN
            );
            println!("{}", auth::hash_password(&pw)?);
            return Ok(());
        }
        Cmd::Notify { kind, stdin, payload } => {
            notify::run_notify_cli(*kind, *stdin, payload.clone());
            return Ok(());
        }
        Cmd::Statusline => {
            usage::run_statusline_cli();
            return Ok(());
        }
        Cmd::Attach { id } => return cli::attach(id),
        Cmd::UninstallService => return daemon::uninstall_service().await,
        _ => {}
    }

    // Commands that run or manage the server need a full config (with a password).
    match cli.cmd {
        Cmd::Serve(t) => {
            let mut cfg = config::Config::load(cfg_path)?;
            t.apply(&mut cfg);
            return server::serve(cfg).await;
        }
        Cmd::Start(t) => {
            let mut cfg = config::Config::load(cfg_path)?;
            t.apply(&mut cfg);
            tunnel::check_config(&cfg)?;
            return daemon::start(cfg_path, &cfg, &t.to_args()).await;
        }
        Cmd::Restart(t) => {
            let mut cfg = config::Config::load(cfg_path)?;
            t.apply(&mut cfg);
            tunnel::check_config(&cfg)?;
            daemon::stop(&cfg, false, false).await?;
            return daemon::start(cfg_path, &cfg, &t.to_args()).await;
        }
        Cmd::InstallService { now } => {
            return daemon::install_service(cfg_path, &config::Config::load(cfg_path)?, now).await;
        }
        _ => {}
    }

    // Everything else talks to a running daemon.
    let cfg = config::Config::load_for_cli(cfg_path)?;
    let sock = cfg.socket_path()?;
    match cli.cmd {
        Cmd::Stop { kill_sessions, force } => daemon::stop(&cfg, kill_sessions, force).await?,
        Cmd::Status => {
            if !daemon::status(&cfg).await? {
                std::process::exit(3);
            }
        }
        Cmd::Logs { lines, follow } => daemon::logs(&cfg, lines, follow).await?,
        Cmd::Ls { json } => cli::ls(&sock, json).await?,
        Cmd::Presets => cli::presets(&sock).await?,
        Cmd::New { target, name, yolo } => cli::new_session(&sock, &target, name, yolo).await?,
        Cmd::Kill { ids } => cli::kill(&sock, &ids).await?,
        Cmd::Rename { id, new } => cli::rename(&sock, &id, &new).await?,
        Cmd::Restore { id, fresh } => cli::restore(&sock, &id, fresh).await?,
        Cmd::Output { id, lines } => cli::output(&sock, &id, lines).await?,
        Cmd::Screen { id, json } => cli::screen_cmd(&sock, &id, json).await?,
        Cmd::Send { id, text, keys, enter } => cli::send(&sock, &id, text, keys, enter).await?,
        Cmd::Answer { id, choice, prompt_id } => cli::answer(&sock, &id, &choice, prompt_id).await?,
        Cmd::Meta { id, note, tags, clear_tags, pin, unpin } => {
            cli::meta(&sock, &id, note, tags, clear_tags, pin, unpin).await?;
        }
        Cmd::Group { cmd: GroupCmd::Start { name } } => cli::group_start(&sock, &name).await?,
        _ => unreachable!("handled above"),
    }
    Ok(())
}

fn read_password() -> Result<String> {
    eprint!("Password: ");
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    let s = s.trim_end_matches(['\r', '\n']).to_owned();
    anyhow::ensure!(!s.is_empty(), "empty password");
    Ok(s)
}
