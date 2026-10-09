//! One PTY running a `tmux attach` client. Dropping/killing it detaches the
//! client; the tmux session (and the agent inside it) keeps running.

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::{sync::mpsc, task::JoinHandle};

pub struct PtyClient {
    master: Arc<Mutex<Box<dyn MasterPty + Send>>>,
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    pub input: mpsc::Sender<Vec<u8>>,
    pub output: mpsc::Receiver<Vec<u8>>,
    /// Resolves with the exit code of the tmux client.
    pub exited: mpsc::Receiver<u32>,
    _tasks: Vec<JoinHandle<()>>,
}

fn size(rows: u16, cols: u16) -> PtySize {
    PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }
}

impl PtyClient {
    pub fn spawn(args: Vec<String>, rows: u16, cols: u16) -> Result<Self> {
        let pair = native_pty_system()
            .openpty(size(rows, cols))
            .context("opening pty")?;
        let mut cmd = CommandBuilder::new("tmux");
        cmd.args(args);
        cmd.env("TERM", "xterm-256color");
        cmd.env("COLORTERM", "truecolor");
        let mut child = pair.slave.spawn_command(cmd).context("spawning tmux")?;
        drop(pair.slave);

        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader()?;
        let mut writer = pair.master.take_writer()?;
        let master = Arc::new(Mutex::new(pair.master));

        // Bounded channels give the PTY natural backpressure.
        let (out_tx, output) = mpsc::channel::<Vec<u8>>(64);
        let (input, mut in_rx) = mpsc::channel::<Vec<u8>>(256);
        let (exit_tx, exited) = mpsc::channel::<u32>(1);

        let read_task = tokio::task::spawn_blocking(move || {
            use std::io::Read;
            let mut buf = [0u8; 16 * 1024];
            while let Ok(n) = reader.read(&mut buf) {
                if n == 0 || out_tx.blocking_send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        let write_task = tokio::task::spawn_blocking(move || {
            use std::io::Write;
            while let Some(bytes) = in_rx.blocking_recv() {
                if writer.write_all(&bytes).and_then(|()| writer.flush()).is_err() {
                    break;
                }
            }
        });
        let wait_task = tokio::task::spawn_blocking(move || {
            let code = child.wait().map(|s| s.exit_code()).unwrap_or(1);
            let _ = exit_tx.blocking_send(code);
        });

        Ok(Self {
            master,
            killer,
            input,
            output,
            exited,
            _tasks: vec![read_task, write_task, wait_task],
        })
    }

    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        let m = self.master.lock().map_err(|_| anyhow::anyhow!("pty lock poisoned"))?;
        m.resize(size(rows, cols)).context("pty resize")
    }
}

impl Drop for PtyClient {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}
