//! The long-running server: HTTP + WebSocket for the web app, the control socket for
//! the CLI, and the background reconcile loop. Runs in the foreground; `ttm start`
//! launches it detached.

use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{Context, Result};

use crate::{AppState, api, auth, config::Config, control, daemon, notify, scrollback, sessions, tmux};

/// Wait for Ctrl-C, SIGTERM, or a shutdown request over the control socket.
async fn shutdown_signal(st: &AppState) {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).expect("installing the SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => tracing::info!("interrupted"),
        _ = term.recv() => tracing::info!("terminated"),
        () = st.shutdown.notified() => tracing::info!("shutdown requested"),
    }
}

pub async fn serve(cfg: Config) -> Result<()> {
    tmux::check_installed().await?;
    let data_dir = cfg.data_dir()?;
    let sock = cfg.socket_path()?;
    control::check_path(&sock)?;
    // Refuse to run twice; removed again when `serve` returns.
    let _pid = daemon::PidGuard::acquire(&data_dir)?;

    crate::tunnel::check_config(&cfg)?;
    if let Some(p) = cfg.tunnel {
        crate::tunnel::check_program(p).await?;
    }
    let auth = auth::Auth::new(&cfg, &data_dir)?;
    let bind = cfg.bind;
    crate::uploads::cleanup(&data_dir, crate::uploads::KEEP);
    let (registry, subs) = sessions::load(&data_dir);
    let ttm_exe = std::env::current_exe().context("locating the ttm executable")?;
    let state = Arc::new(AppState {
        vapid_private: notify::load_or_create_vapid(&data_dir)?,
        subscriptions: std::sync::Mutex::new(subs),
        notify_token: notify::load_or_create_token(&data_dir)?,
        shim_dir: notify::install_shims(&data_dir, &ttm_exe)?,
        last_alert: dashmap::DashMap::new(),
        shutdown: tokio::sync::Notify::new(),
        started: std::time::Instant::now(),
        stopping: std::sync::atomic::AtomicBool::new(false),
        sessions: registry,
        usage: crate::usage::Usage::new(),
        chat: crate::chat::ChatCache::default(),
        driver: tokio::sync::Mutex::new(()),
        tunnel: std::sync::Mutex::new(crate::tunnel::TunnelState {
            provider: cfg.tunnel,
            ..Default::default()
        }),
        statusline: dashmap::DashMap::new(),
        data_dir,
        cfg,
        auth,
    });

    use axum::serve::ListenerExt;
    // Keystrokes and echoes are tiny writes: without TCP_NODELAY they wait to be batched with
    // the next one (Nagle), which adds up to tens of milliseconds to every key.
    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("binding {bind}"))?
        .tap_io(|tcp| {
            let _ = tcp.set_nodelay(true);
        });
    let app = api::router(state.clone());
    let (stop_http, stopped) = tokio::sync::oneshot::channel::<()>();
    let http = tokio::spawn(async move {
        axum::serve(listener, app.into_make_service_with_connect_info::<SocketAddr>())
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let mut control_task = tokio::spawn(control::serve(state.clone(), sock.clone()));
    tokio::spawn(sessions::reconcile_loop(state.clone()));
    tracing::info!("ttm {} listening on http://{bind}", env!("CARGO_PKG_VERSION"));
    let tunnel_task = state.cfg.tunnel.map(|p| {
        tokio::spawn(crate::tunnel::run(state.clone(), p, state.cfg.tunnel_token.clone()))
    });

    tokio::select! {
        () = shutdown_signal(&state) => {}
        r = &mut control_task => {
            // The socket server only returns if something is badly wrong.
            r.context("control socket task panicked")?.context("control socket stopped")?;
        }
    }

    state.stopping.store(true, std::sync::atomic::Ordering::Relaxed);
    tracing::info!("shutting down");
    let _ = stop_http.send(());
    // Open WebSockets would keep a graceful shutdown waiting forever; give them a moment.
    let _ = tokio::time::timeout(Duration::from_secs(3), http).await;
    control_task.abort();
    if let Some(t) = tunnel_task {
        t.abort(); // kill_on_drop ends the ssh child
    }
    // Leave a final copy of every session's screen, then persist state.
    scrollback::snapshot_all(&state).await;
    sessions::save(&state).await;
    let _ = std::fs::remove_file(&sock);
    Ok(())
}
