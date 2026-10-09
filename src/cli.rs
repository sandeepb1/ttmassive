//! Client-side commands: they talk to a running daemon over the control socket.

use std::{os::unix::process::CommandExt, path::Path, process::Command};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::{
    control::{Request, call},
    daemon::human_duration,
    screen,
};

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v[k].as_str().unwrap_or("")
}

fn table(rows: &[Vec<String>]) {
    let cols = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> =
        (0..cols).map(|c| rows.iter().map(|r| r.get(c).map_or(0, |s| s.chars().count())).max().unwrap_or(0)).collect();
    for r in rows {
        let line: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(i, s)| if i + 1 == r.len() { s.clone() } else { format!("{s:<w$}", w = widths[i]) })
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
}

pub async fn ls(sock: &Path, json: bool) -> Result<()> {
    let v = call(sock, &Request::List).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let list = v.as_array().context("unexpected reply")?;
    if list.is_empty() {
        println!("no sessions (start one with `ttm new <preset>`)");
        return Ok(());
    }
    let mut rows = vec![["", "SESSION", "STATE", "AGENT", "PRESET", "IDLE", "NOTE"].map(String::from).to_vec()];
    for s in list {
        let state = match str_of(s, "status") {
            "exited" => format!("exited({})", s["exit_code"].as_u64().map_or("?".into(), |c| c.to_string())),
            other => other.to_owned(),
        };
        let mut note = s["note"].as_str().unwrap_or("").to_owned();
        if let Some(tags) = s["tags"].as_array().filter(|t| !t.is_empty()) {
            let t: Vec<&str> = tags.iter().filter_map(Value::as_str).collect();
            note = format!("{note} [{}]", t.join(",")).trim().to_owned();
        }
        rows.push(vec![
            if s["pinned"].as_bool() == Some(true) { "*".into() } else { String::new() },
            str_of(s, "id").into(),
            state,
            s["agent"].as_str().unwrap_or("-").into(),
            s["preset"].as_str().unwrap_or("-").into(),
            human_duration(s["idle_secs"].as_u64().unwrap_or(0)),
            note,
        ]);
    }
    table(&rows);
    Ok(())
}

pub async fn presets(sock: &Path) -> Result<()> {
    let v = call(sock, &Request::Presets).await?;
    let list = v.as_array().context("unexpected reply")?;
    if list.is_empty() {
        println!("no presets configured (see config.example.toml)");
        return Ok(());
    }
    let mut rows = vec![["PRESET", "AGENT", "RESTART", "AUTOSTART", "COMMAND"].map(String::from).to_vec()];
    for p in list {
        rows.push(vec![
            str_of(p, "name").into(),
            str_of(p, "agent").into(),
            str_of(p, "restart").into(),
            if p["autostart"].as_bool() == Some(true) { "yes".into() } else { "no".into() },
            str_of(p, "command").into(),
        ]);
    }
    table(&rows);
    let g = call(sock, &Request::Groups).await?;
    for g in g.as_array().into_iter().flatten() {
        let members: Vec<&str> = g["presets"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
        println!("group {}: {}", str_of(g, "name"), members.join(", "));
    }
    Ok(())
}

pub async fn new_session(sock: &Path, target: &str, name: Option<String>, yolo: bool) -> Result<()> {
    let preset = (target != "shell").then(|| target.to_owned());
    let v = call(sock, &Request::New { preset, name, yolo }).await?;
    println!("started {}", str_of(&v, "id"));
    Ok(())
}

pub async fn kill(sock: &Path, ids: &[String]) -> Result<()> {
    for id in ids {
        call(sock, &Request::Kill { id: id.clone() }).await?;
        println!("removed {id}");
    }
    Ok(())
}

pub async fn rename(sock: &Path, id: &str, new: &str) -> Result<()> {
    call(sock, &Request::Rename { id: id.into(), new: new.into() }).await?;
    println!("renamed {id} -> {new}");
    Ok(())
}

pub async fn restore(sock: &Path, id: &str, fresh: bool) -> Result<()> {
    call(sock, &Request::Restore { id: id.into(), fresh }).await?;
    println!("restored {id}{}", if fresh { " (fresh start)" } else { "" });
    Ok(())
}

pub async fn output(sock: &Path, id: &str, lines: usize) -> Result<()> {
    let v = call(sock, &Request::Output { id: id.into(), lines }).await?;
    println!("{}", str_of(&v, "text"));
    Ok(())
}

pub async fn screen_cmd(sock: &Path, id: &str, json: bool) -> Result<()> {
    let v = call(sock, &Request::Screen { id: id.into() }).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }
    let a: screen::Analysis = serde_json::from_value(v)?;
    println!("activity: {:?}", a.activity);
    if let Some(p) = a.prompt {
        println!("\nquestion: {}", p.question);
        for c in &p.context {
            println!("  | {c}");
        }
        for c in &p.choices {
            let hot = c.hotkey.as_deref().map(|h| format!("  [{h}]")).unwrap_or_default();
            println!("  {} {}. {}{hot}", if c.selected { ">" } else { " " }, c.index, c.label);
        }
        println!("\nanswer with: ttm answer {id} approve | deny | <number>   (prompt id {})", p.id);
    } else {
        println!("\nno question on screen");
    }
    if !a.tail.is_empty() {
        println!("\nlast lines:");
        for l in a.tail {
            println!("  {l}");
        }
    }
    Ok(())
}

pub async fn send(sock: &Path, id: &str, text: Vec<String>, keys: Vec<String>, enter: bool) -> Result<()> {
    let text = (!text.is_empty()).then(|| text.join(" "));
    call(sock, &Request::Send { id: id.into(), text, keys, enter }).await?;
    Ok(())
}

pub async fn answer(sock: &Path, id: &str, choice: &str, prompt_id: Option<String>) -> Result<()> {
    let v = call(sock, &Request::Answer { id: id.into(), choice: choice.into(), prompt_id }).await?;
    let sent = str_of(&v, "sent");
    println!("{}", if sent.is_empty() { "sent".into() } else { format!("answered: {sent}") });
    Ok(())
}

pub async fn meta(
    sock: &Path,
    id: &str,
    note: Option<String>,
    tags: Vec<String>,
    clear_tags: bool,
    pin: bool,
    unpin: bool,
) -> Result<()> {
    let tags = if clear_tags { Some(Vec::new()) } else { (!tags.is_empty()).then_some(tags) };
    let pinned = if pin { Some(true) } else if unpin { Some(false) } else { None };
    if note.is_none() && tags.is_none() && pinned.is_none() {
        bail!("nothing to change: pass --note, --tag, --clear-tags, --pin or --unpin");
    }
    call(sock, &Request::Meta { id: id.into(), note, tags, pinned }).await?;
    Ok(())
}

pub async fn group_start(sock: &Path, name: &str) -> Result<()> {
    let v = call(sock, &Request::GroupStart { name: name.into() }).await?;
    let ids: Vec<&str> = v["ids"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    println!("started: {}", ids.join(", "));
    Ok(())
}

/// Attach this terminal to a session directly through tmux. Replaces the process.
pub fn attach(id: &str) -> Result<()> {
    let err = Command::new("tmux").args(["-u", "-L", crate::tmux::SOCKET, "attach-session", "-t", &format!("={id}")]).exec();
    Err(err).context("running tmux (is the session running? try `ttm restore`)")
}
