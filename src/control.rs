//! The control socket: a private Unix socket the `ttm` CLI uses to drive a running
//! daemon. One JSON request per connection, one JSON response, newline-terminated.

use std::{path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};

use crate::{AppState, ops, scrollback, sessions, tmux};

const MAX_REQUEST: u64 = 64 * 1024;
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Longest we wait for a reply (restoring a session can take a moment).
const CALL_TIMEOUT: Duration = Duration::from_secs(60);
/// `sun_path` is 104 bytes on macOS and 108 on Linux; stay under both.
const MAX_SOCKET_PATH: usize = 100;

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Request {
    Ping,
    Status,
    Shutdown { kill_sessions: bool },
    List,
    Presets,
    Groups,
    New {
        preset: Option<String>,
        name: Option<String>,
        #[serde(default)]
        yolo: bool,
    },
    Kill { id: String },
    Rename { id: String, new: String },
    Restore { id: String, fresh: bool },
    Output { id: String, lines: usize },
    Screen { id: String },
    Send { id: String, text: Option<String>, keys: Vec<String>, enter: bool },
    Answer { id: String, choice: String, prompt_id: Option<String> },
    Meta { id: String, note: Option<String>, tags: Option<Vec<String>>, pinned: Option<bool> },
    GroupStart { name: String },
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Response {
    pub ok: bool,
    #[serde(default)]
    pub data: Value,
    #[serde(default)]
    pub error: Option<String>,
}

pub fn check_path(path: &Path) -> Result<()> {
    anyhow::ensure!(
        path.as_os_str().len() <= MAX_SOCKET_PATH,
        "control socket path is too long for a Unix socket ({} bytes, max {MAX_SOCKET_PATH}): {}\n\
         set `socket = \"/short/path/ttm.sock\"` in the config file",
        path.as_os_str().len(),
        path.display()
    );
    Ok(())
}

// ------------------------------------------------------------------ server

pub async fn serve(st: Arc<AppState>, path: std::path::PathBuf) -> Result<()> {
    check_path(&path)?;
    // A leftover file from a crashed run would make bind fail. The pidfile guard has
    // already established that no other daemon owns it.
    let _ = std::fs::remove_file(&path);
    let listener = UnixListener::bind(&path).with_context(|| format!("binding control socket {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    tracing::info!("control socket at {}", path.display());
    loop {
        let (stream, _) = listener.accept().await?;
        let st = st.clone();
        tokio::spawn(async move {
            if let Err(e) = handle(st, stream).await {
                tracing::debug!("control connection: {e:#}");
            }
        });
    }
}

async fn handle(st: Arc<AppState>, stream: UnixStream) -> Result<()> {
    let (r, mut w) = stream.into_split();
    let mut line = String::new();
    let mut reader = BufReader::new(r.take(MAX_REQUEST));
    tokio::time::timeout(READ_TIMEOUT, reader.read_line(&mut line)).await.context("request timed out")??;
    let resp = match serde_json::from_str::<Request>(&line) {
        Ok(req) => match dispatch(&st, req).await {
            Ok(data) => Response { ok: true, data, error: None },
            Err(e) => Response { ok: false, data: Value::Null, error: Some(format!("{e:#}")) },
        },
        Err(e) => Response { ok: false, data: Value::Null, error: Some(format!("bad request: {e}")) },
    };
    let mut out = serde_json::to_vec(&resp)?;
    out.push(b'\n');
    w.write_all(&out).await?;
    Ok(())
}

async fn dispatch(st: &Arc<AppState>, req: Request) -> Result<Value> {
    Ok(match req {
        Request::Ping => json!({ "pong": true }),
        Request::Status => status(st).await?,
        Request::Shutdown { kill_sessions } => {
            // Reply first, then stop: the caller is waiting on this connection.
            // Flag it now so sessions that vanish with tmux are kept, not forgotten.
            st.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
            let st = st.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                if kill_sessions {
                    scrollback::snapshot_all(&st).await;
                    let _ = tmux::kill_server().await;
                }
                st.shutdown.notify_one();
            });
            json!({ "stopping": true, "kill_sessions": kill_sessions })
        }
        Request::List => serde_json::to_value(sessions::snapshot(st).await?)?,
        Request::Presets => json!(
            st.cfg
                .presets
                .iter()
                .map(|p| json!({
                    "name": p.name, "command": p.command, "dir": p.dir, "restart": p.restart,
                    "agent": p.agent(), "autostart": p.autostart, "yolo": p.yolo,
                }))
                .collect::<Vec<_>>()
        ),
        Request::Groups => json!(st.cfg.groups.iter().map(|g| json!({ "name": g.name, "presets": g.presets })).collect::<Vec<_>>()),
        Request::New { preset, name, yolo } => {
            let req =
                sessions::NewSession { preset: preset.as_deref(), name: name.as_deref(), group: None, exact: false, yolo, dir: None };
            json!({ "id": sessions::create(st, req).await? })
        }
        Request::Kill { id } => {
            sessions::kill(st, &id).await?;
            json!({})
        }
        Request::Rename { id, new } => {
            sessions::rename(st, &id, &new).await?;
            json!({})
        }
        Request::Restore { id, fresh } => {
            sessions::restore(st, &id, fresh).await?;
            json!({})
        }
        Request::Output { id, lines } => json!({ "text": ops::output(st, &id, lines).await? }),
        Request::Screen { id } => serde_json::to_value(ops::screen(&id).await?)?,
        Request::Send { id, text, keys, enter } => {
            ops::send(&id, text.as_deref(), &keys, enter).await?;
            json!({})
        }
        Request::Answer { id, choice, prompt_id } => {
            json!({ "sent": ops::answer(&id, &choice, prompt_id.as_deref()).await? })
        }
        Request::Meta { id, note, tags, pinned } => {
            ops::set_meta(st, &id, ops::MetaPatch { note, tags, pinned }).await?;
            json!({})
        }
        Request::GroupStart { name } => json!({ "ids": ops::start_group(st, &name).await? }),
    })
}

async fn status(st: &Arc<AppState>) -> Result<Value> {
    let list = sessions::snapshot(st).await?;
    let count = |s: &str| list.iter().filter(|v| v.status == s).count();
    Ok(json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
        "uptime_secs": st.started.elapsed().as_secs(),
        "bind": st.cfg.bind.to_string(),
        "data_dir": st.data_dir,
        "managed": std::env::var_os("TTM_MANAGED").is_some(),
        "sessions": { "running": count("running"), "exited": count("exited"), "stopped": count("stopped") },
        "push_devices": st.subscriptions.lock().map_or(0, |g| g.len()),
        "tunnel": st.tunnel.lock().map_or(Value::Null, |t| t.to_json()),
    }))
}

// ------------------------------------------------------------------ client

/// Send one request to the daemon listening on `sock` and return its data.
pub async fn call(sock: &Path, req: &Request) -> Result<Value> {
    let work = async {
        let mut s = UnixStream::connect(sock).await.map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => {
                anyhow!("ttm is not running (start it with `ttm start`)")
            }
            _ => anyhow!(e).context(format!("connecting to {}", sock.display())),
        })?;
        let mut line = serde_json::to_vec(req)?;
        line.push(b'\n');
        s.write_all(&line).await?;
        s.shutdown().await?;
        let mut buf = String::new();
        BufReader::new(s).read_line(&mut buf).await?;
        anyhow::ensure!(!buf.is_empty(), "the daemon closed the connection without replying");
        let resp: Response = serde_json::from_str(&buf).context("unreadable reply from the daemon")?;
        if !resp.ok {
            bail!("{}", resp.error.unwrap_or_else(|| "request failed".into()));
        }
        Ok(resp.data)
    };
    tokio::time::timeout(CALL_TIMEOUT, work).await.context("the daemon did not reply in time")?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_as_tagged_json() {
        let r = Request::Send { id: "a".into(), text: Some("hi".into()), keys: vec!["Enter".into()], enter: false };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""cmd":"send""#), "{s}");
        let back: Request = serde_json::from_str(&s).unwrap();
        assert!(matches!(back, Request::Send { ref id, .. } if id == "a"));
        assert!(serde_json::from_str::<Request>(r#"{"cmd":"nope"}"#).is_err());
        assert!(matches!(serde_json::from_str::<Request>(r#"{"cmd":"ping"}"#).unwrap(), Request::Ping));
    }

    #[test]
    fn long_socket_paths_are_rejected_with_advice() {
        let long = format!("/{}/ttm.sock", "x".repeat(120));
        let e = check_path(Path::new(&long)).unwrap_err().to_string();
        assert!(e.contains("socket ="), "{e}");
        assert!(check_path(Path::new("/tmp/ttm.sock")).is_ok());
    }
}
