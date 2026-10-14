# ttm

ttm lets you use AI coding agents (Claude Code, Codex, or anything with a command line) from a
phone or tablet. It is one Rust binary that works as the daemon, the web app (a PWA) and the
command-line tool.

Your agents run inside tmux, so they keep working when your connection drops. ttm remembers how
each session was started and what it last showed, so after a reboot you can bring every session
back and resume its Claude or Codex conversation. It also reads the question an agent is asking
off the screen and puts answer buttons on your phone, in the app and in the notification.

On top of the terminal there is a chat view for Claude Code and Codex sessions, live 5-hour and
weekly plan usage, mode and model switching, and shortcuts to start an agent in a folder you
already use.

```
 phone / tablet / browser                         your machine
┌───────────────────────────┐   HTTPS (your proxy)   ┌───────────────────────────────┐
│ xterm.js + touch key bar  │ ─────────────────────▶ │ ttm daemon (Axum)             │
│ sessions drawer           │ ◀── WebSocket ───────▶ │  ├─ login, REST API           │
│ chat view, answer buttons │                        │  ├─ PTY per client ─▶ tmux    │
│ service worker (push)     │ ◀── Web Push (FCM etc) │  ├─ reconcile loop, alerts    │
└───────────────────────────┘                        │  └─ control socket ◀── `ttm` CLI
                                                     └──────────────┬────────────────┘
                                                              tmux -L ttm
                                                          claude · codex · shells
```

Contents: [Security](#security) · [Requirements](#requirements) · [Build and run](#build-and-run)
· [Using the app](#using-the-app) · [Chat view](#chat-view) · [Modes, toggles and launch
options](#modes-toggles-and-launch-options) · [Plan usage](#plan-usage) · [Command
line](#command-line) · [Configuration](#configuration) · [How it works](#how-it-works) ·
[Development](#development) · [Status](#status)

## Security

**Anyone who can log in to ttm has a shell as the user running it.** Treat it like SSH.

- ttm speaks plain HTTP and binds to `127.0.0.1` by default. Put a TLS reverse proxy in front of
  it before you expose it (examples are in `contrib/`). Without TLS, your password and terminal
  traffic are readable on the network, and browsers won't install the PWA or allow push
  notifications.
- Use a long, unique password. Failed logins are limited to 5 per minute per IP, which slows an
  attacker down but doesn't replace a strong password.
- Set `trust_proxy = true` only when ttm is reachable through your proxy and nothing else.
- `--tunnel` puts the login page on the public internet. It refuses to start unless the config has
  a `password_hash` (not a plaintext `password`), and `ttm hash-password` refuses passwords under
  12 characters. See [Public address](#public-address-from-anywhere).
- ttm is a single-user tool: one password, one Unix user, no accounts or roles.
- The CLI talks to the daemon over a Unix socket (`ttm.sock`, mode 0600, inside the owner-only
  data directory), so only your user can drive it that way.
- Saved scrollback can contain anything an agent printed, including secrets. It sits in the data
  directory with owner-only permissions.
- The file viewer only serves files inside the folder a session works in (its git repository, or the
  folder itself), after following symlinks, and never ttm's own config or data directory or a
  `.ssh`, `.gnupg`, `.aws` or `.kube` folder. That is a seatbelt against a stray path in some
  output, not a sandbox: anyone logged in can already read these files through the terminal.

## Requirements

- tmux 3.2 or newer on the machine that runs ttm (checked at startup).
- Rust (tested with 1.96) and a recent Node.js (tested with 26) to build.
- Linux or macOS. [Status](#status) lists what has and hasn't been tested.
- On Linux, the OpenSSL development headers (`libssl-dev` or `openssl-devel`), because the push
  client links the system TLS library.

Check what you have:

```sh
tmux -V && rustc --version && node --version
```

## Build and run

These eight steps take you from nothing to a terminal in your browser. Run them on the machine
that will host your agents.

### 1. Get the code and build it

```sh
git clone https://github.com/sandeepb1/ttmassive.git
cd ttmassive
make release
```

`make release` installs the web dependencies (`npm ci`), builds the web UI and compiles the Rust
binary with the UI embedded. The first build takes a few minutes. The result is one
self-contained file, `target/release/ttm`.

`make web` builds only the UI. A plain `cargo build` stops with a clear message if the UI hasn't
been built yet.

### 2. Install it

```sh
make install          # copies it to ~/.local/bin (PREFIX=/usr/local to change)
ttm --version         # "command not found"? add ~/.local/bin to your PATH
```

### 3. Sign in to your agents

ttm runs the real `claude` and `codex` on this machine, so sign in to each one here, once, the
way you normally would (`claude` then `/login`, and `codex login`). The
[plan usage](#plan-usage) display reads these same logins.

### 4. Write the config

```sh
mkdir -p ~/.config/ttm
cp config.example.toml ~/.config/ttm/config.toml
ttm hash-password                       # type a password; it prints a hash
$EDITOR ~/.config/ttm/config.toml
```

Paste the hash into `password_hash` and add a preset for each agent:

```toml
bind = "127.0.0.1:7681"
password_hash = "$argon2id$..."         # the output of ttm hash-password

[[presets]]
name = "claude"
command = "claude"
dir = "~/projects"

[[presets]]
name = "codex"
command = "codex"
dir = "~/projects"
```

You can set `TTM_PASSWORD_HASH` (or a plaintext `TTM_PASSWORD`) in the environment instead of
putting a hash in the file. That works for `ttm start` and `ttm serve` from your shell. A
background service does not inherit your shell's environment, so for a service keep
`password_hash` in the config file. `config.example.toml` lists every option.

### 5. Start it

```sh
ttm start             # runs in the background and waits until it answers
ttm status            # address, uptime, sessions
```

### 6. Open it

Go to <http://127.0.0.1:7681> and sign in. Tap ☰ and choose `claude`, or run `ttm new claude` in
a terminal. The strip along the top shows your plan usage.

### 7. Reach it from a phone or tablet

Pick one:

- Tailscale: `tailscale serve` puts ttm on an HTTPS address inside your tailnet.
- Android over USB or Wi-Fi debugging: run `adb reverse tcp:7681 tcp:7681`, then open
  `http://localhost:7681` on the device. Browsers treat localhost as secure, so installing the
  app and push notifications work.
- Your own domain: put a TLS reverse proxy in front (`contrib/Caddyfile.example` or
  `contrib/nginx.conf.example`) and set `trust_proxy = true`.
- Anywhere, with nothing to set up: `ttm start --tunnel` opens an ssh tunnel to
  [localhost.run](https://localhost.run) and prints a public `https://…lhr.life` address (see
  [Public address](#public-address-from-anywhere)).

Without HTTPS or localhost, the browser won't install the app or allow push notifications, and
plain HTTP over a network exposes your password and terminal (see [Security](#security)).

### 8. Keep it running (optional)

Without a service, `ttm start` survives closing the terminal but not a reboot. Your agents
survive a daemon restart either way, because they live in tmux. To start ttm at login:

```sh
make install                  # the service should run the installed copy
ttm install-service --now     # a systemd (Linux) or launchd (macOS) user service
```

Three things to know:

- Run the installed binary, not one inside `~/Documents` or `~/Desktop`. On macOS, launchd can
  hang while starting a freshly built binary from a protected folder.
- The service needs `password_hash` in the config file, as step 4 explains.
- A service has no locale. ttm passes `-u` to tmux and sets `LANG=en_US.UTF-8` for new sessions
  when none is set, so Unicode draws correctly.

After you rebuild and run `make install`, restart the service:

```sh
launchctl kickstart -k gui/$(id -u)/com.ttm.server    # macOS
systemctl --user restart ttm                          # Linux
```

On Linux, run `loginctl enable-linger $USER` so the service keeps running after you log out.

### If something goes wrong

| You see | What to do |
|---|---|
| `web/dist/index.html not found` while building | Run `make web` first, or use `make release`, which does it for you |
| `no password configured` | Set `password_hash` in the config, or `TTM_PASSWORD_HASH` in the environment (not for a service) |
| `control socket path is too long` | Set `socket = "/tmp/ttm.sock"` in the config (Unix socket paths are limited to about 100 bytes) |
| `ttm is already running` | Run `ttm status`, then `ttm stop` or `ttm restart` |
| `binding 127.0.0.1:7681` fails | Something else uses the port. Change `bind` |
| The app can't be installed, or push is greyed out | You're on plain HTTP over a network. Use step 7 |
| Every session shows as stopped under a service | An old build ran without a locale. Rebuild, install and restart the service |
| Anything else | `ttm logs -n 100`, or `ttm serve` to run in the foreground and watch it |

## Using the app

### Sessions drawer

Tap ☰ to open the drawer.

- New session: start a session from a preset, a group (shown as `▶ name`) or a plain shell.
- Start in a folder: the drawer lists folders you have used recently in tmux, meaning where your
  panes are now, then where saved sessions started. Each folder has a button per Claude or Codex
  preset. Tap one and the agent starts there, in a session named like `claude-myproject`. A
  plain shell with no folder starts in your home folder. The list comes from `GET /api/dirs`, and
  `POST /api/sessions` accepts `{"preset", "dir"}`.
- Skip permissions: a switch above the preset buttons. See
  [Launch options](#launch-options-skip-permissions).
- Session rows show the state (green: output just now, amber: idle, red: exited, hollow:
  stopped), the agent, idle time, notes and tags. ★ pins a session to the top, ✎ edits its name,
  note and tags, ✕ removes it, and Resume or Restore appears on anything that isn't running.

The URL hash (`/#name`) is the current session, so you can bookmark one.

### Answer buttons

While a session is open, ttm reads its screen every 2 seconds. When an agent asks a question, a
bar appears above the key bar with the question and one button per answer. While an agent is
working, the bar has an Interrupt button. [Reading the screen](#reading-the-screen) explains how
questions are recognised.

### Key bar

The key bar has Esc, Ctrl, Alt, Tab and the arrows.

- Ctrl and Alt are sticky. Tap once to apply to the next key, including keys from your on-screen
  keyboard (so Ctrl then `c` sends Ctrl-C). Double-tap to lock, and tap again to release.
- Arrows and paging keys repeat while held.
- Swipe up on the bar for `^C ⇧Tab | ~ / - PgUp PgDn Home End`.
- A− and A+ at the top right change the font size. ttm remembers it.

The terminal refits when the screen rotates or the on-screen keyboard opens, and tmux follows the
device you used most recently (`window-size latest`).

### Feeling like a native terminal

A few things the browser would otherwise get wrong when you run Claude Code or Codex:

- **Shift+Enter** adds a new line in the prompt instead of sending it (Claude gets `ESC Enter`,
  Codex gets `Ctrl+J`). Alt/Option+Enter does the same. A plain shell is left alone.
- **Option as Meta.** With a hardware keyboard, Option+B / F jump by word and Option+Enter adds a new
  line. It is on by default; turn it off in Settings if you type characters like é with Option.
  Both toggles in Settings are per device.
- **Paste or drop a file** onto the terminal to attach it. The file is uploaded to the host and its
  path is pasted into the prompt, quoted, without pressing Enter. Claude and Codex attach an image
  given its path. Images are shrunk first; other files up to 10 MB are kept as they are.
- **Titles and the bell.** A program's window title shows after the activity text in the pane
  header. A bell from a pane you are not looking at puts a dot on the pane and its row in the list,
  plays a soft sound and buzzes (Android), at most once every ten seconds. The sound can be turned
  off in Settings.
- **Copying on a touch screen.** The terminal is drawn on a canvas, so there is no text to
  long-press. Touch and hold it for half a second to open **Select text**: the screen and the last
  2,000 lines as real text, where your device's own selection handles and Copy menu work.

### The command bar

`Cmd+K` (Mac, iPad) or `Ctrl+K` opens a bar over the terminal, even while it has the keyboard; the
⌘ button in the pane header does the same on a touch screen. Type to filter, arrows and Enter to
choose, Escape to go back. It can start a shell, Claude or Codex in the current pane's folder,
**Open folder…** (type a path inside your home folder, Tab completes it, then pick what to start
there), show another session in a new pane, switch sessions, close a pane, change an agent's model or
mode, search the history, select text, open a file by path, and flip the toggles above. New sessions
take the next free tile, or replace the next pane when they are all in use. Commands you use
recently come first. Press `Ctrl+K` a second time to close the bar and send the terminal its own
`Ctrl+K` (delete to the end of the line).

### Searching the history

The terminal only shows the screen; tmux keeps the rest (the last 50,000 lines). Tap the magnifier
in the corner of the terminal, or press `Ctrl+Alt+F`, and type: every match in the whole history
is listed, newest first, with how far up it is. Tap one, or step with the up and down arrows (or
Enter and Shift+Enter), and tmux scrolls to it in copy-mode with the matches highlighted and the
current one in a different colour. The list tucks away after a jump so you can see the match; tap
the counter to bring it back. Leave with the close button, Escape in the search box, or `q` in the
terminal. Closing the tab after a jump also leaves copy-mode.

**Your messages.** With the box empty, the bar lists what you typed to Claude or Codex in this
session, newest first, with how long ago and how far up each is. Tap one, or use the arrows, to jump
to it in the terminal history. Each of your messages that is on the terminal screen also gets a thin
bar in the margin, so they stand out while you scroll back. `Ctrl+Alt+[` and `Ctrl+Alt+]` step to the
previous and next message without opening the bar. These need the terminal view; an agent session
opens in the chat view, so switch with the Chat/Terminal toggle.

A message is found by its first line, and counts as yours when only a few symbols come before it on
its line (`> `, `❯ `, `│ > `), so the agent quoting you is not mistaken for you. One that the agent
draws differently, such as a long paste shown as `[Pasted text]`, or that has scrolled out of the
50,000 lines, is greyed out as "not on screen". It does not shift the others, because messages are
matched from the newest end.

Matching is literal, not a regular expression. Case is ignored unless the search has a capital
letter, as in tmux. Up to 1000 matches are counted and 200 are listed. It searches running
sessions only; a stopped session keeps just its saved output (see `ttm output`).

### The activity chip

A small pill in the corner of each pane, under its header, says what the agent is doing and for how
long, in the chat view and the terminal view alike: `Working · Running the tests · 3m 12s`, or
`Waiting for you · 40s` in amber when it is stuck on a question. It fades out when the agent is
idle, and a tap goes straight through it. In tiled view each pane has its own, so one glance shows
which agents are busy and which need you.

- *What* it is doing comes from the conversation: the to-do the agent is on, else the tool it is
  running. A plain shell has no chip.
- *How long* is timed on the page from the moment it sees the change. If you open a pane while the
  agent is already mid-turn, it counts from when you sent the message instead, using the server's
  clock (the `Date` header of the screen reading) so a device with a wrong clock still agrees.
- The state is the same reading that drives the header's "working…" and "waiting for you": the
  screen is checked every two seconds. If the screen cannot be read for 10 seconds the chip goes
  away rather than sticking on "Working".

### Opening files from the terminal

A file path in the terminal output becomes tappable: `src/main.rs:42:7` from a compiler,
`./lib/a.ts(12,3)`, `/etc/hosts`, `~/notes.md`. Tap it to open a read-only viewer with line numbers
and light syntax colours, scrolled to the line the path named, which is marked. **Copy path** copies
the real path. The viewer shows the file as it is now, so it reflects what an agent just wrote.

- Paths are looked up from the folder the session is in, then from the root of its git repository,
  so a path printed relative to the repository root works from a subfolder.
- Only files inside that folder open. A path that leaves it, directly or through a symlink, is
  refused with a reason, as are ttm's own files, credentials folders, directories and binary files.
- A file over 1 MB is cut at 1 MB on a whole line and says so. Colours need no setup and cover Rust,
  TypeScript and JavaScript, Python, Go, shell, JSON, TOML, YAML, CSS, C-family languages and
  Java/Kotlin; other files are shown plain.
- Text is recognised as a path only when it looks like one: a folder and a file name, an explicit
  `./`, `../`, `~/` or `/` location, or a name with a code extension. URLs, version numbers,
  `and/or`, and slash commands such as `/model` are left alone. A bare `node.js` in prose does
  look like a file, and opens "no such file" if tapped.

### When an agent exits

When the agent in a preset session ends (you press Ctrl+C out of Claude, or run `/exit`), the
pane becomes your shell, so you can keep working in the same session. Presets with
`restart = "on_failure"` keep the old behaviour, because they need the exit to restart.

### Notifications

Open the drawer, choose Notifications, tap Turn on, allow the browser prompt, then tap Send test.

| Agent | You are alerted when |
|---|---|
| Claude Code | its `Notification` hook fires (permission prompt, waiting for input) or `Stop` (finished) |
| Codex | its `notify` program fires: turn finished, or approval requested |
| anything else | output goes quiet for `idle_alert_secs` after activity |
| any preset | the process exits (with its exit code) |

For "needs input" alerts, ttm reads the screen first, so the notification shows the actual
question and up to two answer buttons, using the screen's own wording ("Yes", "No, and tell…").
Tapping a button answers without opening the app and posts a confirmation. Tapping the body
opens the session. No popup appears if you're already looking at that session.

Hooks work by putting small `claude` and `codex` wrapper scripts first on the `PATH` of preset
sessions. The wrappers add the hook settings and then run the real binary. This has three
consequences:

- Start agents from presets. In a plain-shell session your shell's startup files rearrange
  `PATH` and may alias `claude`, which bypasses the wrapper.
- For Codex, the wrapper passes `-c notify=[...]`, which replaces any global `notify` setting
  from `~/.codex/config.toml` inside ttm sessions.
- iOS only delivers web push to a PWA added to the Home Screen (iOS 16.4 and later).

### Surviving restarts and reboots

| What happens | What ttm does |
|---|---|
| Connection drops, tab closed | Nothing is lost. Reopening redraws the live screen |
| `ttm` restarts or is upgraded | Sessions keep running in tmux and the daemon picks them up again, with notes, tags, pins and agent ids |
| Reboot, `tmux kill-server`, `ttm stop --kill-sessions` | Each session stays in the list as stopped, with its last screen saved. Restore starts it again |
| A preset has `autostart = true` | On daemon start it is created, or restored (resuming) if it was stopped |

ttm keeps its memory in the data directory: `state.json` (how each session was started, agent
conversation ids, notes, tags, pins, push subscriptions) and `scrollback/` (the last ~2000 lines
of each session, refreshed while it's active and on shutdown). A plain shell restores into the
directory it had `cd`'d to.

A shell you close yourself (`exit`) is forgotten, because that was deliberate, as long as other
sessions are still running. If it was the last one, ttm keeps it, since tmux itself may have
died. Anything lost to a shutdown or reboot is kept.

### Resuming Claude Code and Codex

When you start `claude` or `codex` from a preset, ttm learns the agent's own conversation id from
its hooks (Claude's `SessionStart`, `Notification` and `Stop`, and Codex's `notify`). Resume then
runs `claude --resume <id>` or `codex resume <id>` in the same directory. If no id was learned,
it falls back to `claude --continue` or `codex resume --last`. Start fresh skips all of that.

Resume is offered after an exit, a kill or a reboot. Presets with `restart = "on_failure"` also
resume on automatic restarts (set `resume_on_restart = false` to turn that off). Only the first
agent a session starts resumes, never agents that the agent spawns itself.

## Chat view

For a Claude Code or Codex session, a Chat | Terminal toggle appears at the top. Chat shows the
conversation as messages instead of a terminal. It is the same session underneath: what you type
goes into the agent's own prompt, and the terminal is one tap away. New agent sessions open in
Chat, and each session remembers the view you last used.

Chat is built from the conversation log the agent already writes. It shows what the agent has
finished, one message or tool call at a time, and not text arriving token by token (the terminal
is still the place for that). It shows:

- your messages, and the agent's replies as formatted text: headings, lists, tables, links and
  code blocks with a Copy button. Anything the agent writes is rendered as text, never as markup;
- tool calls as one collapsed line each (`$ Run the tests ✓`). Tap one to see the command and
  its output, or for an edit, the diff;
- the agent's thinking, collapsed, and a Working… indicator while it runs;
- slash commands you ran (`/compact`) and their confirmations.

### Composer

Type and press the arrow to send. With a physical keyboard, Enter sends and Shift+Enter starts a
new line. Messages can run to several lines. Empty messages can't be sent, because Claude keeps a
greyed-out suggestion in its input box and an empty Enter might accept it. The ■ button
interrupts the agent.

- `/` commands: start a message with `/` for suggestions. They include the agent's built-in
  commands plus your own commands and skills (`~/.claude/commands`, `~/.claude/skills`, the
  project's `.claude/` folders and `~/.codex/prompts`). Commands that come from plugins aren't
  suggested, but you can still type them.
- `@` files: type `@` and part of a file name to search the session's folder. The search follows
  `.gitignore`, skips hidden files and never leaves that folder.
- Images: the + button opens the camera or your files. Photos are shrunk to 2048 px in the
  browser, saved on the host under `uploads/` in the data directory (files older than 7 days are
  removed whenever the daemon starts) and referenced as `@/path/to/image.jpg`, which both Claude
  and Codex read. They show as thumbnails in the chat.
- Model and effort: the chip next to the composer shows the model and effort. Tap it to switch.
  The list is the agent's own `/model` menu, read live, and your choice applies to this session
  only. Codex asks for a reasoning level after the model, so its picker has a second step.
  ttm never takes the menus' "make this the default" path. In Claude that path is Enter or a
  number. In Codex it is Enter on the level screen, which rewrites `model` and
  `model_reasoning_effort` in `config.toml` (confirmed in a copy of the Codex folder). Your
  defaults for new sessions are never changed from the chat. The chip updates as soon as you
  switch. If you switch inside the terminal view, Claude's chip follows from its log straight
  away, and Codex's follows when it next replies.
- `/resume`: typing `/resume` on its own opens a picker instead of sending a message, because in
  both agents it opens an interactive list of earlier conversations. ttm opens the agent's own
  list, reads the rows off the screen, and shows them in a sheet. Tap a row to resume it. The
  sheet has a search box, Newer and Older buttons for paging, and (for Claude) a button that
  switches between this project and all projects. Rows show when the conversation was last used
  and, for Claude, its branch, size and folder. Picking a row only resumes it and changes no
  setting. `/resume <id>` with an argument is sent as typed. Codex's list shows only the
  current folder's conversations, and ttm doesn't switch its filter. The chat follows the new
  conversation as soon as Claude reports it. Codex's follows when its next turn ends.
- Suggested replies (Claude): after some turns Claude shows a greyed-out suggestion in its empty
  input box, such as "go ahead with step 2". The chat shows it as a chip above the composer. Tap
  it to put the text in the box, then edit it or send it. ttm reads the suggestion from the
  screen's faint text, so it only appears while the input line is empty. Claude's fixed "Try …"
  hint is ignored, and Codex's placeholder isn't offered.

### Questions from the agent

When Claude asks you something with its question tool, the chat shows a card. Tap an option (or
several, for multiple choice), or type your own answer and tap Send, or tap Dismiss. A single
question with one answer is sent the moment you tap. ttm answers by pressing the keys Claude's own
form expects and checking the screen between presses, so if the question has gone away it stops
instead of typing into the wrong place. Permission prompts show as buttons above the composer, as
they do in the terminal view.

### Limits of the chat view

- Switching the model and answering the question form work by pressing keys in Claude's own
  menus. If a Claude Code update changes those menus, the buttons may stop working (the error
  says so). The terminal view keeps working.
- If you've half-typed something in the terminal view, a chat message is added after it.
- Codex supports messages, tool calls, image attach, approvals (shown as buttons, with the
  command and its reason) and model switching. It has no equivalent of Claude's question card.
  Its model menu has a "More reasoning…" row (Max and Ultra) that opens another menu, and the
  picker leaves that row out.
- A new Codex session gets its Chat toggle after its first reply. Codex only reports its
  conversation id when a turn ends, so type the first message in the terminal view. The toggle
  appears, and the view switches to Chat, when the reply is done. Claude sessions have the toggle
  from the start. Codex also runs a background thread that names each conversation, and that
  thread fires the same hook with its own id. ttm only accepts a Codex conversation id that has a
  log file, so the helper's id is ignored.
- The chat keeps the last 2,000 items of a conversation.

## Session list and details panel

The left sidebar groups sessions under **Pinned**, then one heading per working folder (tap a
heading to fold it; folds are remembered). Each row shows a dot for what the agent is doing
(pulsing blue: working, pulsing amber and a **Needs you** tag: waiting on a question, green:
ready for the next instruction, grey: idle or stopped), the first lines of its latest message,
the git branch of its folder, and the usual agent and idle time. The row's `⋯` holds pin, edit and
remove. Rows for sessions that were not started from a preset only show the folder and branch:
what an agent is doing and its latest message come from the agent's own log, which ttm only knows
about for sessions it started.

The right sidebar (the panel button in the top bar, or a swipe from the right edge) has search and
four tabs:

- **Overview**: the model, mode and conversation actions, and plan usage.
- **Files**: every file the agent edited, wrote or deleted in the conversation, newest first, with
  `+`/`−` line counts and the combined diff. Claude's `Edit`, `MultiEdit` and `Write` and Codex's
  patches are both covered.
- **Plan**: Claude's to-do list (`TodoWrite`) and the plan from plan mode (`ExitPlanMode`), or a
  plan Codex proposed, as formatted text. Codex's own to-do updates are not shown: it records them
  as code inside a tool call, which is not a stable format to read.
- **Images**: pictures you attached in the chat, kept for a week.

Search covers the messages loaded in the chat (not the whole log). Tap a result to jump to the
message, which flashes once.

## Helper agents

When Claude starts subagents (its `Agent` tool) or Codex starts helper threads, you can watch them.

- In the chat, the call becomes a **card**: the agent type, what it was asked, a clock while it
  runs, how many tool calls it has made, and the last thing it did. Tap the card for the task and
  the helper's report, and **Open transcript** for everything it did.
- A transcript opens beside the conversation when the screen has room for panes (in another helper's
  pane if one is open, otherwise a new pane or the next one; the conversation you were reading keeps
  focus), or in a tall sheet that
  drags down to close. It is read-only and live while the helper runs, and a helper that started
  helpers of its own shows them as cards in turn.
- The details panel's **Overview** lists every helper of the conversation as a tree, newest state
  first, including ones whose card has scrolled out of the chat. A session in the left list shows
  a count chip while helpers run; tap it to list them under the row.

Claude's helpers are read from `<conversation>/subagents/`, and a helper counts as running while its
log is still being written (about 90 seconds of quiet means it finished). Codex's are found from
the start and end events in the parent log, and each helper's own rollout file. Neither agent
tells ttm when a helper is *interrupted*, so one that stopped early shows as finished. The chat only
reads the last 8 MB of a long log, so a card may be missing for an old helper: the Overview list still
has it.

## Tiled panes

Show several sessions at once, as many as **Settings → Panes → Most panes** allows (up to 6).
Tap the columns button in a pane's header to open another session in a new pane, or the grid
button to pick a layout. Each pane has its own Chat/Terminal toggle, key bar and agent colour
(orange for Claude, teal for Codex, set by the theme), and its own close button.

**Grids.** One pane, two columns, two rows, three columns, one big and two small, 2×2 and 3×2.
Opening panes picks the smallest grid with room, starting from the grid you set in Settings (one
for big screens, one for tablets). Picking a grid by hand keeps it, even with empty tiles: an
empty tile lists the sessions that are not on screen. Drag the line between two tiles to resize
them; the sizes are remembered for each grid.

**Small screens.** A grid only appears if every tile gets room for a usable terminal. A portrait
tablet gets stacked rows, a phone gets one pane. Panes that do not fit stay open behind a strip
of tabs; tap a tab to bring that pane on screen.

**Focus.** The pane you touched last is the focused one: its header is underlined, the details
panel describes it, and the address bar names it. `Ctrl+Alt` with an arrow key moves to the
neighbouring tile, and `Ctrl+Alt+1` to `6` jumps to a tile. In tiled view the session list shrinks
to a rail of buttons (two letters per session, with the status dot). A button opens that session
in the focused pane, or jumps to the pane that already shows it, and the first button widens the
rail back to the full list until you pick something.

**Workspaces.** In the layout sheet, name the current grid and its sessions and save it, for
example "Claude + Codex + shell". Workspaces and the Panes settings are stored on the server, so
one saved on your Mac opens on your tablet. A session that is no longer running is skipped.

Open panes are remembered across reloads on each device. At most four panes draw with the GPU
(the focused one and the most recently used); the rest use the slower text renderer, so a
tablet's few WebGL contexts are never used up.

## Modes, toggles and launch options

### Mode chip

Next to the model chip, a chip shows the agent's current mode. Claude has Auto, Manual, Accept
edits, Plan and Bypass. Codex has Default and Plan. Tap the chip to open a sheet with two big
buttons, Plan and Build, and every mode the agent offers below them.

Build means the mode you were in before you went to Plan, so going to Plan and back takes two
taps. ttm presses Shift+Tab for you and checks the footer after each press. It stops with an
error if the mode you asked for isn't reachable in that session. Codex raises its reasoning level
while planning, and the sheet says so.

### More (⋯)

- Reasoning − and + step the level for this session only. Claude uses its `/effort` slider, and
  Codex uses Alt+. and Alt+,. The level shown is read from the agent's footer, or from Claude's
  status line, so it is current before the agent's next reply.
- "What Codex may do…" lists Codex's permission presets: Ask for approval, Approve for me, Read
  Only and Full Access. A choice applies to this session and doesn't write `config.toml`. Full
  Access is red and asks for a second confirmation first. ttm refuses to enable it without that
  confirmation, and cancelling returns you to the list.
- Commands: Compact for both agents, Context and Clear for Claude, and New conversation for
  Codex. Clear and New ask first. Only these commands can be sent this way.
- Edit last message puts your last message in the box so you can change it and send it again. It
  doesn't use Claude's Esc Esc rewind, which rewinds files and conversation and leaves no marker
  in Codex's log, so the chat couldn't follow it.

### Launch options ("skip permissions")

The sessions drawer has a Skip permissions switch above the preset buttons. With it on, a
`claude` preset starts with `IS_SANDBOX=1` and `--dangerously-skip-permissions`, and a `codex`
preset starts with `--yolo` (any `-a`, `-s` or `--full-auto` on the preset is dropped). The
switch applies to the folder buttons too, and it resets after each launch, so it's always a
per-session choice.

Such sessions are tagged "⚡ permissions skipped" and show a red Bypass chip. The flag is stored
with the session, so Restore and Resume keep it. The same options exist as `yolo = true` on a
preset in `config.toml` and as `ttm new <preset> --yolo`. The default is off.

## Plan usage

The strip along the top of the app shows how much of your 5-hour and weekly limits you've used:

```
CC 5h 29% · 7d 31% │ CX 5h 98% · 7d 15%
```

`CC` is Claude Code and `CX` is Codex. Numbers turn amber from 75% and red from 90%, dim when
they are more than 30 minutes old, and show `⚠` when a tool's numbers can't be read.

Tap the strip for details: a bar and a reset countdown for each window, per-model weekly limits
when your plan has them (Claude's Opus and Sonnet caps), your plan name, how old the numbers are
and where they came from, why a refresh failed, and the tokens each Claude or Codex session has
used. The sessions drawer shows the same token line on every row (`18k in · 8 out · 68k cached`:
fresh and cache-written input, output, and cache reads).

| Tool | Where the numbers come from |
|---|---|
| Claude Code | Anthropic's usage endpoint, using the login Claude Code already holds. Claude sessions started from presets also report fresh numbers through their status line while you work |
| Codex | OpenAI's usage endpoint, using the login in `~/.codex/auth.json`. If that can't be reached, the newest limit reading in Codex's own session log |

- Nothing to configure. Sign in to each tool on the host (step 3 of
  [Build and run](#build-and-run)) and it works. A tool you aren't signed in to shows `⚠`, with
  the reason in the panel.
- ttm only reads your logins and never refreshes them. Refreshing would rotate the token and
  could sign the real tool out. If the panel says a login has expired, use that tool once.
- Claude's login is read from `~/.claude/.credentials.json` (or under `CLAUDE_CONFIG_DIR`) when
  that file exists, and otherwise from the macOS Keychain item `Claude Code-credentials`. The
  first Keychain read can show a macOS prompt: choose Always Allow. Do this once with `ttm serve`
  in a terminal before relying on a background service, which has nobody there to click it.
  Codex's folder can be moved with `CODEX_HOME`.
- The app asks for usage once a minute, and ttm asks the provider at most every five minutes.
  Opening the panel or tapping Refresh now accepts numbers up to a minute old.
- With Notifications on, you get one push per tool and window when 5-hour or weekly use reaches
  90%. Tapping it opens the panel. A new window can warn again.
- API-key logins have no plan limits, so a tool signed in that way shows `⚠` and says why.
- In preset sessions, ttm sets Claude's status line, which prints `5h 29% (4h19m) · 7d 30%` in
  Claude's footer. If you have your own `statusLine`, expect ttm's to replace it inside ttm (not
  tested with a custom one).
- These are the tools' own endpoints, not documented public APIs, so they can change. When one
  breaks, the panel shows the error and keeps the last numbers it had.

## Public address from anywhere

`ttm start --tunnel` (or `tunnel = "localhost.run"` in the config) runs your system's `ssh` as a
reverse tunnel and gives ttm a public HTTPS address. No account, no extra program and no extra
Rust crates: OpenSSH is the only requirement.

```
$ ttm start --tunnel
ttm started (pid 4242), listening on http://127.0.0.1:7681
public  https://b78e2f4d1619ec.lhr.life  (localhost.run)
```

| | |
|---|---|
| `--tunnel` / `--tunnel localhost.run` | localhost.run: no account, no time limit. The address can change whenever the tunnel reconnects |
| `--tunnel pinggy` | pinggy.io over port 443. The free tier closes after an hour and the address changes. `tunnel_token` (or `TTM_TUNNEL_TOKEN`) gives a tunnel that lasts |
| `--tunnel cloudflare` | Your own [Cloudflare Tunnel](#your-own-domain-with-cloudflare): a domain you pick, no time limit, needs `cloudflared` |
| `--no-tunnel` | Turn off a tunnel the config asks for |

Works on `serve`, `start` and `restart`. The flag beats the config.

- **Where you see the address:** `ttm start` waits up to 15 seconds for it, `ttm status` shows it
  with the uptime and reconnect count, and Settings in the web app has it with a Copy button.
  When it changes you get a push notification that opens the new address.
- **Reconnecting:** ssh runs with a keepalive. If it drops (sleep, Wi-Fi change), ttm reconnects
  with a growing pause, up to a minute. ssh can also look connected after the Mac sleeps while the
  service has forgotten the tunnel (its address then shows "no tunnel"), so ttm checks the public
  address from outside every minute and rebuilds the tunnel after two misses in a row. If the
  tunnel can't come up at all, ttm keeps serving on
  localhost and `start` warns instead of failing; `ttm status` shows the last error. Only a
  missing `ssh` stops the start.
- **ssh runs isolated:** with `-F none`, no agent, no keys from `~/.ssh`, and its own
  `known_hosts` in the data directory (`accept-new`, so a changed host key is still refused).
- **Login protection:** the tunnel refuses a plaintext password (see [Security](#security)).
  pinggy tells ttm each visitor's address, so the 5-per-minute limit applies per visitor.
  localhost.run does not, so every visitor looks like `127.0.0.1`; ttm then lets one login
  attempt through every 2 seconds for everyone instead, so a stranger can't lock you out.
  While the tunnel is on, ttm believes `X-Forwarded-*` headers only from connections made by
  this machine, and marks the session cookie `Secure` for the public address.
### Your own domain with Cloudflare

For an address you choose and that never changes, use a Cloudflare Tunnel. ttm runs
`cloudflared` with your tunnel token and reports your domain as the public address.

1. In Cloudflare Zero Trust, create a tunnel (Networks > Tunnels) and copy its token.
2. Under the tunnel's Public Hostname, add your domain (`ttm.example.com`) with service
   `HTTP` and URL `localhost:7681` (your `bind` port). Cloudflare creates the DNS record.
3. Put this in the config, then restart the service:

```toml
tunnel = "cloudflare"
tunnel_token = "eyJh..."          # or TTM_TUNNEL_TOKEN
tunnel_domain = "ttm.example.com" # or TTM_TUNNEL_DOMAIN
```

- ttm refuses to start the tunnel without the token, the domain, and a `password_hash`.
- The token is passed to `cloudflared` in its environment, not on the command line. Keep
  the config file private (`chmod 600`).
- The address shows as up once `cloudflared` registers a connection. If the hostname is not
  routed to the tunnel in the dashboard, ttm still shows it as up but the domain will not
  load: check step 2. `cloudflared` reconnects on its own; ttm restarts it if it exits.

- **Both services are free third-party services.** Everything you do in ttm passes through their
  servers (over TLS that they terminate), and they can rate-limit or drop you. For something you
  depend on, use Tailscale or your own domain and proxy.

## Command line

These commands talk to the running daemon, so run `ttm start` first.

### The daemon

| Command | What it does |
|---|---|
| `ttm start [--tunnel [SERVICE]] [--no-tunnel]` | Start the server in the background (new session, pidfile, log file). Waits until it answers and prints the log tail if it fails. `--tunnel` also opens a [public address](#public-address-from-anywhere) |
| `ttm stop` | Stop the daemon. Sessions keep running in tmux. `--kill-sessions` also stops tmux and every agent, after saving their screens |
| `ttm restart [--tunnel [SERVICE]] [--no-tunnel]` | `stop`, then `start` |
| `ttm status` | Version, uptime, address, session counts, push devices, and whether a service manager owns it. Exit status 3 when not running |
| `ttm logs [-n N] [-f]` | Show or follow the log |
| `ttm serve [--tunnel [SERVICE]] [--no-tunnel]` | Run in the foreground (what service managers run) |
| `ttm install-service [--now]` | Write a systemd (Linux) or launchd (macOS) user service that runs `ttm serve`, using this shell's `PATH` so your agents are found. `--now` also starts it |
| `ttm uninstall-service` | Remove it. Sessions keep running |

Only one daemon runs at a time. When a service manager owns the daemon, `ttm stop` refuses (it
would just be restarted) and prints the `systemctl` or `launchctl` command to use. `--force`
overrides that. `contrib/` also has hand-editable `ttm.service` and `com.ttm.server.plist`
examples, plus `Caddyfile.example` and `nginx.conf.example` for TLS.

### Sessions

```sh
ttm ls                       # sessions: state, agent, preset, idle time, notes, tags (--json)
ttm presets                  # presets and groups
ttm new claude [--name api]  # start a preset; `ttm new shell` for a plain shell
ttm new claude --yolo        # start Claude or Codex with permission checks skipped
ttm group start dev          # start every preset in a group
ttm restore api [--fresh]    # bring a stopped or exited session back (alias: resume)
ttm attach api               # attach this terminal through tmux
ttm rename api backend
ttm kill api other           # remove sessions and their saved output
ttm meta api --note "fixing auth" --tag backend --pin     # also --unpin, --clear-tags
```

### Reading the screen

```sh
ttm output api -n 100        # live screen + scrollback, or the saved copy if stopped
ttm screen api               # what it's doing, and the question if it is asking one
ttm send api "run the tests" --enter          # type into a session without attaching
ttm send api --key Esc --key Up               # named keys: Enter Esc Tab Up Down Left Right
                                              #   Home End PgUp PgDn Backspace C-c C-d C-z C-l
ttm answer api approve       # or: deny | interrupt | <choice number>
```

`ttm screen` reports `working`, `waiting`, `idle` or `unknown`. For a question it also gives the
text, the lines above it (the command or file), each choice with its hotkey, and a prompt id.
ttm recognises numbered selector menus ("❯ 1. Yes / 2. … / 3. No (esc)", as Claude Code and Codex
draw them) and inline `[y/N]` questions. A menu only counts as live when it is the last thing on
screen, so an old answered question never lingers.

`approve` and `deny` pick the choice that reads like yes or no. Keys are sent through tmux, so
they work without anyone attached. For a menu with a visible cursor, ttm walks to the choice with
the arrow keys and presses Enter (a "(esc)" choice presses Esc). For `[y/N]` it types the letter
and Enter. The web app and notifications send the prompt id along, and the server refuses (409)
if the question on screen has changed, so a stale button can never answer a new question. From
the CLI, `--prompt-id` does the same.

## Configuration

`config.example.toml` documents every option, and a test keeps it valid. Preset keys:

| Key | Meaning |
|---|---|
| `command`, `dir`, `env` | What to run (through `sh -c`), where, and extra environment |
| `restart`, `max_restarts`, `backoff_secs` | `on_failure` respawns after a non-zero exit or a kill, with doubling backoff. After `max_restarts` it stays dead so you can read why |
| `autostart`, `resume_on_restart` | Start (or restore) it with the daemon, and resume the conversation on automatic restarts |
| `yolo` | Always start Claude or Codex with permission checks skipped |
| `memory_mb`, `cpu_percent` | Resource limits, Linux only (systemd cgroups) |
| `agent`, `idle_alert_secs` | Override the agent guess (`claude`, `codex` or `other`), and set the quiet-time threshold for `other` |

Groups (`[[groups]]`, with `name` and `presets = [...]`) start several presets together. Other
settings are `bind`, `password_hash`, `trust_proxy`, `tunnel`, `tunnel_token`, `tunnel_domain`, `session_ttl_hours`, `data_dir`, `socket`,
`vapid_subject` and `notify_url`.

## How it works

- tmux is the source of truth for what is running. Each session is a tmux session on a dedicated
  server (`tmux -L ttm`). Every browser connection runs its own `tmux new-session -A` client in a
  PTY, so disconnecting only kills that client. You can also run `tmux -L ttm attach` over SSH.
- The daemon remembers the rest. A 2-second reconcile loop adopts sessions, keeps vanished ones
  as stopped, applies restart policies, raises idle and exit alerts and snapshots scrollback.
- The control socket is `ttm.sock` in the data directory (or `socket =` in the config; Unix
  socket paths are limited to about 100 bytes). It takes one JSON request per connection. The CLI
  and the HTTP API call the same operations.
- The WebSocket protocol (`GET /ws?session=<id>&rows=&cols=`) uses binary frames for raw terminal
  bytes in both directions. Text frames are JSON control messages: `resize` and `ping` from the
  client, and `pong`, `stopped` (the session exists only as saved output), `exit` and `error`
  from the server. Resize calls `master.resize(...)` on the PTY.
- Auth is an Argon2 password check followed by an HMAC-signed `HttpOnly`, `SameSite=Strict`
  cookie (`Secure` when the proxy says HTTPS). The WebSocket also checks that `Origin` matches
  `Host`.
- Hooks: agents run `ttm notify`, which posts to the server on loopback with a shared token.
  Sessions are identified by a hidden id that survives renames.
- Usage: `GET /api/usage` (login cookie; `?refresh=1` accepts data a minute old instead of five)
  returns both tools' windows, plan and source, plus tokens per session. Claude's status line
  runs `ttm statusline`, which prints the footer and posts to `/api/internal/usage` the same
  loopback way. Token totals come from Claude's transcript and Codex's session log, found by the
  conversation id the hooks reported, and are read incrementally.
- Files: `GET /api/sessions/{id}/file?path=` returns `{path, rel, name, size, truncated, lines,
  text}` for a file inside the session's project folder (404 not found, 403 outside it or private,
  415 a directory or binary file). `path` is absolute, `~/…`, or relative to the session's folder,
  then to its repository root. Text over 1 MB is cut on a line.
- Messages: `GET /api/sessions/{id}/turns` returns `{turns: [{id, ts, preview, snippet, n, text, up}]}`,
  your messages to the agent from its transcript, newest first (at most 40), each placed on the
  terminal. `n` is null when the message is not found there. To go to one, `POST .../search/jump`
  with `{q: snippet, n, text}`. A plain shell returns an empty list.
- Search: `GET /api/sessions/{id}/search?q=` reads the whole tmux history and returns
  `{total, capped, hits: [{n, up, text, at, len}]}`, newest first, where `n` counts matches from the
  newest. `POST /api/sessions/{id}/search/jump` with `{q, n, text?}` puts the session in tmux's
  copy-mode on that match using tmux's own search, so the count and the jump agree; `text` is the
  line that was tapped, so the jump still lands on it if new output renumbered the matches.
  `POST .../search/close` leaves copy-mode. The query is passed to tmux as an escaped literal.
- Chat: `GET /api/sessions/{id}/chat?epoch=&after=` returns the conversation items added or
  changed since the client's last sequence number (a new `epoch` means start over), together with
  the screen analysis, the current mode and any suggested reply. Other routes under the same
  prefix:
  - `POST .../message` types a message (multi-line text goes in as a bracketed paste).
  - `POST .../question` answers or dismisses the agent's question form.
  - `GET .../suggest?kind=slash|file&q=` feeds the composer.
  - `POST .../upload` saves an attached image, and `GET .../attachments` lists the session's
    images, newest first.
  - `POST .../model/open`, `.../model/choose`, `.../model/level`, `.../model/close` and
    `POST .../effort` drive the agent's model menu.
  - `POST .../resume/open`, `.../resume/search`, `.../resume/scope`, `.../resume/page` and
    `.../resume/choose` drive the agent's `/resume` list. The agent's list closes with
    `.../model/close`.
  - `POST .../mode`, `.../reasoning`, `.../command`, `.../permissions/open` and
    `.../permissions/choose` drive modes, reasoning, quick commands and Codex permissions.

  `GET .../agents` lists a conversation's helper agents (also in the chat response as `agents`),
  and `GET .../agents/{agent}/chat?epoch=&after=` returns one helper's transcript in the same
  shape as the chat. `GET /api/sessions` adds `running_subagents`.

  `GET /api/sessions` also returns each session's `cwd` and `branch` (read from `.git/HEAD`, no
  `git` process), and for Claude and Codex sessions `activity` and a one-line `preview`. The chat
  response carries the agent's `todos` and `plan`.

  The log parsing is in `src/chat.rs`. The key sequences for the question form, the model menu
  and the mode switches are in `src/question.rs`, `src/modelmenu.rs` and `src/modes.rs`, each
  with tests built on screens captured from the real agents.

## Development

```sh
cargo run -- serve --config ./dev.toml        # backend on :7681
cd web && npm run dev                          # Vite dev server, proxies /api and /ws
make test                                      # cargo test + web unit tests
make check                                     # clippy -D warnings + tsc
```

To see how the chat parser reads a real conversation log:

```sh
TTM_TEST_LOG=~/.claude/projects/<project>/<session>.jsonl TTM_TEST_AGENT=claude \
  cargo test real_log -- --ignored --nocapture      # or TTM_TEST_AGENT=codex
```

ttm uses the shared tmux server `tmux -L ttm`, so sessions from a throwaway test instance also
show up in a real ttm running on the same machine. When you test anything that drives an agent's
own menus, set `CODEX_HOME` (or `CLAUDE_CONFIG_DIR`) to a throwaway copy for the server process,
and delete the copy afterwards. Those menus have a "make this the default" key one press away
from the safe one, and a copy of the config folder holds your login. Keep the copy's path short: a
long one made a test session die at startup.

In debug builds the server reads `web/dist` from disk, so rebuilding the UI doesn't need a Rust
rebuild. Release builds embed it. On macOS the default data directory is
`~/Library/Application Support/ttm` (`XDG_DATA_HOME` is ignored). Set `data_dir` in a dev config
to keep experiments out of it.

## Status

ttm was built and exercised on macOS with a Samsung Galaxy Tab S9 Ultra (Samsung Internet). That
covered:

- login, typing, the key bar, rotation, sessions, presets and restarts;
- push delivery through FCM, tap-to-open, answer buttons in the app and in the notification;
- stopped sessions with Resume, the daemon commands, the control socket and the service-file
  generator;
- survival across daemon restarts and a simulated reboot (`ttm stop --kill-sessions`);
- plan usage against real Claude Code and Codex logins, with a live Claude session inside ttm
  feeding the strip, the panel and the per-session token line.

The chat view was checked on the tablet against a live Claude Code session: messages and tool
calls, a two-question form answered by tapping (one single choice, one multiple choice), a typed
answer, a dismissed question, `/` and `@` suggestions, attaching a photo through Android's picker
(Claude described it correctly), switching model and effort, a suggested reply sent from its
chip, and the `/resume` picker (search, all projects, paging and picking a conversation). After
the model and effort switches, `~/.claude/settings.json` was byte-for-byte unchanged. A Claude
session started from the folder shortcut opened straight into chat.

A live Codex session was checked the same way: messages and shell commands in the chat, an
approval answered with the button (the file was then created), a slash and `@` suggestion, an
image Codex read and described, the mode sheet (Default to Plan and back), the More sheet, a
cancelled Full Access confirmation, the `/resume` picker (search and picking a conversation, through the API), and
switching Codex's model and reasoning level (through the real picker on the tablet, and through
the API for every level). That was done with `CODEX_HOME` pointed at a throwaway copy of the Codex
folder, so a bug couldn't have touched the real config. Afterwards the copy was deleted and
`~/.codex/config.toml` was byte-for-byte unchanged. The model Codex then ran its turn with matched
the switch.

The launchd service was run from `~/.local/bin`, which exposed and fixed two bugs: a binary in
`~/Documents` that hung at startup, and sessions that all showed as stopped because a service
has no locale. After Ctrl+C out of Claude, the pane dropped into a working shell.

### Not verified

- The tunnel in a browser. It was driven with curl over the real localhost.run and pinggy: start,
  address, login limits, the `Secure` cookie, reconnecting, bad token, no `ssh`. A full browser
  session through it (WebSocket terminal, installing the PWA, push on the new address) was not.
- Real agent screens. Question detection is tuned for the menu layouts Claude Code and Codex
  draw, but it was tested against fixtures and stand-in programs that print the same shapes, not
  live permission prompts from the real tools. If a prompt isn't recognised there are no answer
  buttons (the terminal still works). `ttm screen <session>` shows what ttm sees.
- Resume. The `--resume` and `resume <id>` handoff was tested with stand-ins that run exactly
  what ttm injects, not with a real agent. The hook payloads themselves (`session_id`,
  `thread-id`) have since been seen from live Claude and Codex runs.
- Linux. The code is portable, and the systemd unit and `systemd-run` limits are written, but
  none of it has been run on Linux. `install-service --now` was only tested for generating files
  there.
- Usage warnings. The 90% push logic is unit-tested and the alert was seen being raised, but a
  warning was never delivered to a subscribed device, and tapping one (which opens the panel)
  hasn't been tried. Also untried: a Claude login on Linux (read from
  `~/.claude/.credentials.json`), a plan with per-model weekly limits (they were empty on the Pro
  plan tested), the strip on a phone-width screen, and the Keychain prompt under launchd.
- Modes and launch options. Claude's mode cycle (Auto, Manual, Accept edits, Plan) and the Claude
  reasoning stepper were driven through the API but not all tapped on the tablet. Codex Full
  Access was never enabled. The Skip permissions switch launching a new session was seen only
  through sessions created the same way via the API.
- Codex beyond approvals. Any other kind of question Codex might ask, its "More reasoning…"
  levels (Max, Ultra), and how its model list looks on an account with different models.
- Chat edge cases. A multiple-choice question that is the only question in its form (the review
  page is assumed to follow, as it does for a form of several), typing your own answer in the
  first question of a several-question form, a phone-width screen, long conversations near the
  2,000-item cap, and Claude Code versions other than the one tested (2.1.295).
- iOS and Safari, Chrome on Android, and the nginx example.

### Known limitations

- Single user.
- Resource limits are Linux-only. A `setrlimit` fallback was rejected because Node-based agents
  crash under address-space limits.
- Hook alerts, resume and answer buttons in notifications need agents started from presets.
- The reconcile loop runs every 2 seconds.
- Scrollback is plain text with no colours, and is saved at most every 5 seconds.
