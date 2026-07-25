//! CodeForge — terminal-native IDE.
//!
//! Client/server. A detached **server** (`forge --server`) owns all state —
//! windows, panes (each a child in its own PTY + vt100 emulator, `pane.rs`),
//! layout (`layout.rs`) — and outlives any client, so a disconnect/SSH drop
//! never loses the session. A thin **client** (`forge`) owns the real terminal:
//! it forwards keystrokes/resizes over a unix socket (`protocol.rs`) and paints
//! the bytes the server sends back. Bare `forge` starts a server (picking a
//! project) then attaches; a second `forge` attaches to the running one.
//!
//! Each window is a fixed editor (Neovim) + terminal + Claude CLI trio; there
//! are no splits. Hide/show panes to reshape it: editor left (full height) with
//! terminal + Claude stacked on the right; hide the editor and the other two go
//! side by side. The editor's IDE features (fuzzy find, grep, LSP) come from the
//! Neovim config in `config/nvim/`, loaded via NVIM_APPNAME=codeforge.
//!
//! Controls (Ctrl-a is the prefix):
//!   Ctrl-a e     show/hide the editor pane
//!   Ctrl-a t     show/hide the terminal pane
//!   Ctrl-a a     show/hide the Claude pane
//!   Ctrl-a hjkl  move focus left/down/up/right
//!   Ctrl-a o     cycle focus to the next pane
//!   Ctrl-a p     open the project picker (re-home the current window)
//!   Ctrl-a c     new window (its own editor/shell/AI for another project)
//!   Ctrl-a X     close the current window (kills its panes)
//!   Ctrl-a n     next window   ·   Ctrl-a 1..9  jump to window
//!   Ctrl-a d     detach (server keeps running; reattach with `forge`)
//!   Ctrl-a r     reload the server on the latest build (reopens same windows)
//!   Ctrl-a F     forget the saved session (next `forge` starts fresh)
//!   Ctrl-a ?     toggle the keybinding help overlay
//!   Ctrl-a q     quit CodeForge (ends the session)
//!   Ctrl-a Ctrl-a  send a literal Ctrl-a to the child
//! Click a pane to focus it; mouse events pass through to the child (nvim mouse
//! works). Every other key is forwarded to the focused pane's child unchanged.
//! A child that exits (Ctrl-D / `exit` / `:q`) is respawned in place — the
//! terminal restarts, the editor reopens, the Claude pane returns on
//! `claude --resume`. Panes are never destroyed, only hidden.

mod config;
mod layout;
mod pane;
mod picker;
mod protocol;

use std::io::{self, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, queue, terminal};
use portable_pty::CommandBuilder;
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;

use config::{Config, Keys};
use layout::{Dir, FocusDir, Layout, Rect};
use pane::{Pane, PaneRole};
use picker::{Picker, PickerAction};

/// A restorable description of one window: its project dir, the shell's working
/// directory, and the editor's open files.
#[derive(Clone)]
struct WindowSpec {
    dir: PathBuf,
    shell_cwd: Option<PathBuf>,
    files: Vec<PathBuf>,
}

impl WindowSpec {
    fn bare(dir: PathBuf) -> WindowSpec {
        WindowSpec {
            dir,
            shell_cwd: None,
            files: Vec::new(),
        }
    }
}

/// The nvim RPC socket for the editor pane with the given id, so we can query
/// its open files at capture time.
fn nvim_sock(id: usize) -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join(format!("codeforge-nvim-{id}.sock"))
}

/// Read a process's current working directory via /proc.
fn proc_cwd(pid: u32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/cwd")).ok()
}

/// Fetch the current temperature for `city` from wttr.in (best-effort, via curl).
fn fetch_weather(city: &str) -> Option<String> {
    let q = city.trim().replace(' ', "+");
    let url = format!("https://wttr.in/{q}?format=%t&u");
    let out = Command::new("curl")
        .arg("-s")
        .arg("--max-time")
        .arg("4")
        .arg(&url)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    // wttr.in returns e.g. "+72°F"; reject error pages / junk.
    if s.is_empty() || s.len() > 12 || s.contains("Unknown") || s.contains("html") {
        return None;
    }
    Some(s)
}

/// Ask a running nvim (via its RPC socket) for its listed, named buffers.
fn query_nvim_files(sock: &Path) -> Vec<PathBuf> {
    if !sock.exists() {
        return Vec::new();
    }
    let expr =
        r#"join(filter(map(getbufinfo({"buflisted":1}), "v:val.name"), "v:val != \"\""), "\n")"#;
    let out = Command::new("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg(expr)
        .output();
    match out {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .collect(),
        _ => Vec::new(),
    }
}

/// Capture a restorable spec for every window (open files + shell cwd).
fn capture_specs(windows: &[Window]) -> Vec<WindowSpec> {
    windows
        .iter()
        .map(|w| {
            let files = w
                .panes
                .iter()
                .find(|p| p.role == PaneRole::Editor)
                .map(|p| query_nvim_files(&nvim_sock(p.id)))
                .unwrap_or_default();
            let shell_cwd = w
                .panes
                .iter()
                .find(|p| p.role == PaneRole::Shell)
                .and_then(|p| p.pid())
                .and_then(proc_cwd);
            WindowSpec {
                dir: w.dir.clone(),
                shell_cwd,
                files,
            }
        })
        .collect()
}

/// Messages funneled into the single-threaded event loop from producer threads.
pub enum Msg {
    /// Bytes emitted by pane `id`'s child.
    Output(usize, Vec<u8>),
    /// Bytes typed by the user, already stripped of prefix sequences.
    Input(Vec<u8>),
    /// A client attached: its write half + terminal size (rows, cols).
    Attach(UnixStream, u16, u16),
    /// The client detached (server keeps running).
    Detach,
    /// The client's terminal was resized (rows, cols).
    Resize(u16, u16),
    /// Show/hide a pane by role.
    Toggle(PaneRole),
    /// Move focus to the next pane (cycle order).
    FocusNext,
    /// Move focus to the neighbouring pane in a direction.
    Focus(FocusDir),
    /// A mouse event at 1-based screen coords, with the SGR button code and
    /// whether it's a press/drag (`M`) vs release (`m`).
    Mouse {
        x: u16,
        y: u16,
        cb: u16,
        press: bool,
    },
    /// Toggle the keybinding help overlay.
    ToggleHelp,
    /// Toggle the project picker (re-homes the current window).
    OpenPicker,
    /// Open the picker to create a new window.
    NewWindow,
    /// Close the current window (kills its panes).
    CloseWindow,
    /// Switch to window `n` (0-based).
    SelectWindow(usize),
    /// Periodic tick to refresh the status-bar clock.
    Tick,
    /// Reload: restart the server on the latest build, reopening the same dirs.
    Reload,
    /// Forget the saved session (next `forge` starts fresh).
    ClearSession,
    /// User asked to quit.
    Quit,
}

/// One workspace: a fixed editor + terminal + AI trio for a single directory.
/// The layout is derived from which panes are visible (no arbitrary splits): the
/// editor is a full-height left column; the terminal and AI stack on the right.
struct Window {
    panes: Vec<Pane>,
    layout: Layout,
    focus_id: usize,
    dir: PathBuf,
    title: String,
    show_editor: bool,
    show_shell: bool,
    show_ai: bool,
    /// The AI pane wants attention (bell rang while this window was unfocused).
    attention: bool,
    /// Last-seen AI-pane bell count, to detect new bells.
    last_ai_bell: usize,
}

/// Find the id of the pane with a given role (all three always exist).
fn role_id(panes: &[Pane], role: PaneRole) -> usize {
    panes
        .iter()
        .find(|p| p.role == role)
        .map(|p| p.id)
        .unwrap_or(0)
}

/// Derive the layout tree from which panes are visible. Editor (when shown) is a
/// full-height left column; terminal-over-AI stack on the right. With the editor
/// hidden, the remaining panes go side by side (AI left, terminal right). Returns
/// `None` if nothing is visible.
fn compute_layout(
    panes: &[Pane],
    show_editor: bool,
    show_shell: bool,
    show_ai: bool,
    editor_ratio: f32,
    right_ratio: f32,
) -> Option<Layout> {
    let e = role_id(panes, PaneRole::Editor);
    let s = role_id(panes, PaneRole::Shell);
    let a = role_id(panes, PaneRole::Ai);

    if show_editor {
        let mut right = Vec::new();
        if show_shell {
            right.push(s);
        }
        if show_ai {
            right.push(a);
        }
        Some(match right.len() {
            0 => Layout::Leaf(e),
            1 => Layout::Split {
                dir: Dir::Row,
                ratio: editor_ratio,
                a: Box::new(Layout::Leaf(e)),
                b: Box::new(Layout::Leaf(right[0])),
            },
            _ => Layout::Split {
                dir: Dir::Row,
                ratio: editor_ratio,
                a: Box::new(Layout::Leaf(e)),
                b: Box::new(Layout::Split {
                    dir: Dir::Col,
                    ratio: right_ratio,
                    a: Box::new(Layout::Leaf(right[0])),
                    b: Box::new(Layout::Leaf(right[1])),
                }),
            },
        })
    } else {
        // Editor hidden: AI on the left, terminal on the right.
        let mut cols = Vec::new();
        if show_ai {
            cols.push(a);
        }
        if show_shell {
            cols.push(s);
        }
        match cols.len() {
            0 => None,
            1 => Some(Layout::Leaf(cols[0])),
            _ => Some(Layout::Split {
                dir: Dir::Row,
                ratio: 0.5,
                a: Box::new(Layout::Leaf(cols[0])),
                b: Box::new(Layout::Leaf(cols[1])),
            }),
        }
    }
}

/// Recompute a window's layout from its visibility flags and fix focus if the
/// focused pane became hidden.
fn refresh_layout(w: &mut Window, editor_ratio: f32, right_ratio: f32) {
    if let Some(l) = compute_layout(
        &w.panes,
        w.show_editor,
        w.show_shell,
        w.show_ai,
        editor_ratio,
        right_ratio,
    ) {
        w.layout = l;
        if !leaf_exists(&w.layout, w.focus_id) {
            if let Some(f) = w.layout.first_leaf() {
                w.focus_id = f;
            }
        }
    }
}

/// The last path component, for the window's status-bar label.
fn dir_title(dir: &Path) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("~")
        .to_string()
}

/// Build a new window from a spec (dir + optional shell cwd + open files), with
/// pane ids from `base`. The editor is focused.
fn new_window(
    spec: &WindowSpec,
    cfg: &Config,
    shell: &str,
    base: usize,
    tx: &Sender<Msg>,
) -> Result<Window> {
    let panes = spawn_ide(spec, cfg, shell, base, tx)?;
    let layout = compute_layout(&panes, true, true, true, cfg.editor_ratio, cfg.right_ratio)
        .unwrap_or(Layout::Leaf(base));
    let title = dir_title(&spec.dir);
    Ok(Window {
        panes,
        layout,
        focus_id: base,
        dir: spec.dir.clone(),
        title,
        show_editor: true,
        show_shell: true,
        show_ai: true,
        attention: false,
        last_ai_bell: 0,
    })
}

/// Spawn the three panes (editor + terminal + AI) for `spec`, ids from `base`.
/// The editor opens `spec.files` (or the dir) and, if nvim, listens on an RPC
/// socket so its buffers can be captured. The shell starts in `spec.shell_cwd`.
fn spawn_ide(
    spec: &WindowSpec,
    cfg: &Config,
    shell: &str,
    base: usize,
    tx: &Sender<Msg>,
) -> Result<Vec<Pane>> {
    let dir = &spec.dir;

    let (editor, editor_title) = build_editor(cfg, dir, base, &spec.files);
    let shell_cwd = spec.shell_cwd.as_deref().unwrap_or(dir);
    let (ai, ai_title) = command_line(&cfg.ai, dir);

    Ok(vec![
        Pane::spawn(
            editor,
            editor_title,
            PaneRole::Editor,
            1,
            1,
            base,
            tx.clone(),
        )?,
        Pane::spawn(
            command(shell, shell_cwd),
            "shell".into(),
            PaneRole::Shell,
            1,
            1,
            base + 1,
            tx.clone(),
        )?,
        Pane::spawn(ai, ai_title, PaneRole::Ai, 1, 1, base + 2, tx.clone())?,
    ])
}

/// Build a command that inherits our environment, runs in `cwd`, and advertises
/// a 256-color terminal to the child.
fn command(program: &str, cwd: &Path) -> CommandBuilder {
    let mut c = CommandBuilder::new(program);
    c.cwd(cwd);
    c.env("TERM", "xterm-256color");
    c
}

/// Build the editor command for a pane (id `id`), opening `files` (or the dir).
/// nvim gets NVIM_APPNAME + an RPC socket so its buffers can be captured later.
fn build_editor(
    cfg: &Config,
    dir: &Path,
    id: usize,
    files: &[PathBuf],
) -> (CommandBuilder, String) {
    let mut toks = cfg.editor.split_whitespace();
    let prog = toks.next().unwrap_or("nvim");
    let is_nvim = prog.ends_with("nvim");
    let title = prog.rsplit('/').next().unwrap_or(prog).to_string();
    let mut c = command(prog, dir);
    for a in toks {
        if a != "." {
            c.arg(a);
        }
    }
    if is_nvim {
        c.env("NVIM_APPNAME", "codeforge");
        let sock = nvim_sock(id);
        let _ = std::fs::remove_file(&sock); // stale socket blocks --listen
        c.arg("--listen");
        c.arg(&sock);
    }
    if files.is_empty() {
        c.arg(".");
    } else {
        for f in files {
            c.arg(f);
        }
    }
    (c, title)
}

/// Respawn a pane's child after it exits (Ctrl-D / `exit` / `:q`), keeping the
/// pane and its id in place. The editor reopens the dir, the shell restarts, and
/// the AI pane comes back on the resume picker (`claude --resume`).
fn respawn_pane(
    role: PaneRole,
    dir: &Path,
    id: usize,
    cfg: &Config,
    shell: &str,
    tx: &Sender<Msg>,
) -> Result<Pane> {
    match role {
        PaneRole::Editor => {
            let (cmd, title) = build_editor(cfg, dir, id, &[]);
            Pane::spawn(cmd, title, role, 1, 1, id, tx.clone())
        }
        PaneRole::Shell => Pane::spawn(
            command(shell, dir),
            "shell".into(),
            role,
            1,
            1,
            id,
            tx.clone(),
        ),
        PaneRole::Ai => {
            // Bring claude back on its session-resume picker.
            let line = if cfg.ai.trim_start().starts_with("claude") {
                "claude --resume".to_string()
            } else {
                cfg.ai.clone()
            };
            let (cmd, title) = command_line(&line, dir);
            Pane::spawn(cmd, title, role, 1, 1, id, tx.clone())
        }
    }
}

/// Build a command from a whitespace-separated command line (program + args),
/// running in `cwd`. Returns the builder and a short title (the program name).
/// nvim gets NVIM_APPNAME so it loads CodeForge's isolated config.
fn command_line(cmdline: &str, cwd: &Path) -> (CommandBuilder, String) {
    let mut parts = cmdline.split_whitespace();
    let program = parts.next().unwrap_or("bash");
    let mut c = command(program, cwd);
    for arg in parts {
        c.arg(arg);
    }
    if program.ends_with("nvim") {
        c.env("NVIM_APPNAME", "codeforge");
    }
    (c, program.rsplit('/').next().unwrap_or(program).to_string())
}

/// The user's projects root: `$DDN_PROJECTS` if set (see the DDN bashrc), else
/// the first of `~/projects` / `~/work/projects` that exists, else `$HOME`.
fn projects_root() -> PathBuf {
    if let Ok(p) = std::env::var("DDN_PROJECTS") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let home = std::env::var("HOME").map(PathBuf::from).unwrap_or_default();
    for cand in ["projects", "work/projects"] {
        let d = home.join(cand);
        if d.is_dir() {
            return d;
        }
    }
    home
}

/// Resolve a CLI project argument: an absolute/relative path, or a bare name
/// resolved under `root`.
fn resolve_arg_dir(arg: &str, root: &Path) -> PathBuf {
    let p = PathBuf::from(arg);
    if p.is_dir() {
        return p.canonicalize().unwrap_or(p);
    }
    let under = root.join(arg);
    if under.is_dir() {
        return under;
    }
    p // Let the child surface the error (e.g. nvim on a new path).
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let sock = socket_path();

    // Server mode (spawned detached): `forge --server <dir>...` opens one window
    // per directory; `forge --server` (no dirs) restores the saved session.
    if args.get(1).map(|s| s == "--server").unwrap_or(false) {
        return run_server(&sock, args[2..].to_vec());
    }

    // A session already running? Attach to it.
    if UnixStream::connect(&sock).is_ok() {
        return run_client(&sock);
    }
    if args.get(1).map(|s| s == "attach").unwrap_or(false) {
        eprintln!("codeforge: no session running");
        return Ok(());
    }

    // No server yet: pick a project (CLI arg or startup picker), start a detached
    // server, then attach to it.
    let (cfg, warn) = Config::load();
    if let Some(w) = warn {
        eprintln!("codeforge: {w}");
    }
    for c in cfg.keys.conflicts() {
        eprintln!("codeforge: keybind warning: {c}");
    }
    let proot = cfg
        .projects_root
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(projects_root);

    // A CLI arg starts a fresh single-window session. Bare `forge` restores the
    // last saved session (resuming AI conversations); if none, shows the picker.
    match args.get(1) {
        Some(arg) => spawn_server(&[resolve_arg_dir(arg, &proot)])?,
        None => {
            if load_snapshot().is_empty() {
                let picked = choose_project_interactive(&proot)?;
                spawn_server(&[picked])?;
            } else {
                spawn_server(&[])?; // restore
            }
        }
    }

    wait_for_socket(&sock, Duration::from_secs(5))?;
    run_client(&sock)
}

/// Where the last session's window directories are saved, so a fresh `forge`
/// (after quit or reboot) can restore the layout: `$XDG_STATE_HOME/codeforge/session`.
fn snapshot_path() -> PathBuf {
    let base = std::env::var("XDG_STATE_HOME")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/state")
        });
    base.join("codeforge").join("session")
}

/// Save window specs, best-effort. One window per line, tab-separated:
/// `dir \t shell_cwd \t file1,file2,...`.
fn save_snapshot(specs: &[WindowSpec]) {
    let path = snapshot_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let body: String = specs
        .iter()
        .map(|s| {
            let dir = s.dir.to_string_lossy();
            let cwd = s
                .shell_cwd
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            let files = s
                .files
                .iter()
                .map(|f| f.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(",");
            format!("{dir}\t{cwd}\t{files}")
        })
        .collect::<Vec<_>>()
        .join("\n");
    let _ = std::fs::write(&path, body);
}

/// Load saved window specs whose project dir still exists.
fn load_snapshot() -> Vec<WindowSpec> {
    let text = match std::fs::read_to_string(snapshot_path()) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    let mut specs = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let dir = PathBuf::from(fields.next().unwrap_or("").trim());
        if !dir.is_dir() {
            continue;
        }
        let cwd = fields.next().unwrap_or("").trim();
        let shell_cwd = if cwd.is_empty() {
            None
        } else {
            let p = PathBuf::from(cwd);
            p.is_dir().then_some(p)
        };
        let files = fields
            .next()
            .unwrap_or("")
            .split(',')
            .map(str::trim)
            .filter(|f| !f.is_empty())
            .map(PathBuf::from)
            .collect();
        specs.push(WindowSpec {
            dir,
            shell_cwd,
            files,
        });
    }
    specs
}

/// Delete the saved session so the next `forge` starts fresh.
fn clear_snapshot() {
    let _ = std::fs::remove_file(snapshot_path());
}

/// Unix socket for the per-user server: `$XDG_RUNTIME_DIR/codeforge-<user>.sock`.
fn socket_path() -> PathBuf {
    let base = std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let user = std::env::var("USER").unwrap_or_else(|_| "user".to_string());
    base.join(format!("codeforge-{user}.sock"))
}

/// Spawn a server detached from the controlling terminal (via `setsid`) so it
/// survives the client exiting / the SSH drop. With directories, it opens one
/// fresh window each; with none, it restores the saved session (resuming AI
/// conversations).
fn spawn_server(dirs: &[PathBuf]) -> Result<()> {
    let exe = std::env::current_exe().context("finding own executable")?;
    let mut cmd = Command::new("setsid");
    cmd.arg(exe).arg("--server");
    for d in dirs {
        cmd.arg(d);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("spawning detached server via setsid")?;
    Ok(())
}

/// Wait for the server's socket to become connectable.
fn wait_for_socket(sock: &Path, timeout: Duration) -> Result<()> {
    let start = Instant::now();
    loop {
        if UnixStream::connect(sock).is_ok() {
            return Ok(());
        }
        if start.elapsed() > timeout {
            bail!("timed out waiting for the server to start");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// Client-side startup project picker (bare `forge`).
fn choose_project_interactive(proot: &Path) -> Result<PathBuf> {
    let mut out = io::stdout();
    enable_raw_mode().context("enabling raw mode")?;
    queue!(out, EnterAlternateScreen, cursor::Hide)?;
    out.flush()?;
    let guard = TerminalGuard;
    let chosen = Picker::new(proot.to_path_buf()).run_blocking(&mut out)?;
    drop(guard);
    Ok(chosen.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))))
}

/// The persistent server: owns all windows/PTYs, renders to whatever client is
/// attached, and keeps running across detaches/disconnects.
fn run_server(sock: &Path, dirs: Vec<String>) -> Result<()> {
    let _ = std::fs::remove_file(sock); // clear any stale socket
    let listener = UnixListener::bind(sock).context("binding server socket")?;

    let (mut cfg, _) = Config::load();
    let prefix = cfg.prefix_byte();
    let keys = cfg.keys;
    let shell = cfg
        .shell
        .clone()
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));
    let proot = cfg
        .projects_root
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(projects_root);

    // With dirs: one fresh window each. Without: restore the saved session and
    // resume each AI conversation.
    let specs: Vec<WindowSpec> = if dirs.is_empty() {
        let saved = load_snapshot();
        cfg.ai = format!("{} --continue", cfg.ai.trim());
        if saved.is_empty() {
            vec![WindowSpec::bare(
                std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            )]
        } else {
            saved
        }
    } else {
        dirs.iter()
            .map(|d| WindowSpec::bare(resolve_arg_dir(d, &proot)))
            .collect()
    };

    let (tx, rx) = mpsc::channel::<Msg>();

    // Accept clients. Each connection gets a reader thread that turns its frames
    // into messages (input is parsed for prefix commands / mouse).
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let stream = match conn {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let tx = tx.clone();
                thread::spawn(move || {
                    let mut rd = match stream.try_clone() {
                        Ok(r) => r,
                        Err(_) => return,
                    };
                    let mut parser = InputParser::new(prefix, keys);
                    loop {
                        match protocol::read_frame(&mut rd) {
                            Ok((protocol::ATTACH, p)) => {
                                if let (Some((r, c)), Ok(wr)) =
                                    (protocol::parse_size(&p), stream.try_clone())
                                {
                                    if tx.send(Msg::Attach(wr, r, c)).is_err() {
                                        return;
                                    }
                                }
                            }
                            Ok((protocol::INPUT, p)) => {
                                let mut msgs = Vec::new();
                                parser.feed(&p, &mut msgs);
                                for m in msgs {
                                    if tx.send(m).is_err() {
                                        return;
                                    }
                                }
                            }
                            Ok((protocol::RESIZE, p)) => {
                                if let Some((r, c)) = protocol::parse_size(&p) {
                                    if tx.send(Msg::Resize(r, c)).is_err() {
                                        return;
                                    }
                                }
                            }
                            Ok((protocol::DETACH, _)) | Err(_) => {
                                let _ = tx.send(Msg::Detach);
                                return;
                            }
                            Ok(_) => {}
                        }
                    }
                });
            }
        });
    }

    // Status-bar clock tick.
    {
        let tx = tx.clone();
        thread::spawn(move || loop {
            thread::sleep(Duration::from_secs(15));
            if tx.send(Msg::Tick).is_err() {
                break;
            }
        });
    }

    // Status-bar weather (best-effort, refreshed every 10 minutes).
    let weather = Arc::new(Mutex::new(String::new()));
    if !cfg.weather.trim().is_empty() {
        let weather = weather.clone();
        let city = cfg.weather.clone();
        let tx = tx.clone();
        thread::spawn(move || loop {
            if let Some(t) = fetch_weather(&city) {
                *weather.lock().unwrap() = t;
                let _ = tx.send(Msg::Tick);
            }
            thread::sleep(Duration::from_secs(600));
        });
    }

    let mut windows: Vec<Window> = Vec::new();
    let mut next_id: usize = 0;
    for spec in &specs {
        windows.push(new_window(spec, &cfg, &shell, next_id, &tx)?);
        next_id += 3;
    }
    let mut cur = 0usize;
    let mut show_help = false;
    let mut picker: Option<Picker> = None;
    let mut picker_new_window = false;
    // (cols, rows) of the attached client; updated on attach/resize.
    let mut size = (80u16, 24u16);
    let mut client: Option<UnixStream> = None;
    let mut framebuf: Vec<u8> = Vec::new();
    // Whether to keep persisting the session (Ctrl-a fresh turns this off).
    let mut save_enabled = true;

    // Persist the session so a fresh `forge` can restore it. Re-captured when the
    // window set changes and on teardown (reload/quit).
    let mut last_dirs: Vec<PathBuf> = windows.iter().map(|w| w.dir.clone()).collect();
    save_snapshot(&capture_specs(&windows));

    {
        let w = &mut windows[cur];
        relayout(&mut w.panes, &w.layout, size.0, size.1.saturating_sub(1))?;
    }

    while let Ok(first) = rx.recv() {
        let mut batch = vec![first];
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }

        let mut dirty = false;
        let mut quit = false;
        let mut needs_clear = false;
        for msg in batch {
            match msg {
                Msg::Output(id, bytes) => {
                    for (wi, w) in windows.iter_mut().enumerate() {
                        if let Some(p) = w.panes.iter_mut().find(|p| p.id == id) {
                            p.feed(&bytes);
                            if wi == cur {
                                dirty = true;
                            }
                            break;
                        }
                    }
                }
                Msg::Input(bytes) => {
                    if let Some(pk) = picker.as_mut() {
                        match pk.feed_bytes(&bytes) {
                            PickerAction::None => {}
                            PickerAction::Cancel => {
                                picker = None;
                                needs_clear = true;
                            }
                            PickerAction::Chosen(dir) => {
                                picker = None;
                                needs_clear = true;
                                let (c, r) = size;
                                let area = r.saturating_sub(1);
                                let base = next_id;
                                next_id = base + 3;
                                let spec = WindowSpec::bare(dir);
                                if picker_new_window {
                                    windows.push(new_window(&spec, &cfg, &shell, base, &tx)?);
                                    cur = windows.len() - 1;
                                } else {
                                    for p in &mut windows[cur].panes {
                                        p.kill();
                                    }
                                    windows[cur] = new_window(&spec, &cfg, &shell, base, &tx)?;
                                }
                                let w = &mut windows[cur];
                                relayout(&mut w.panes, &w.layout, c, area)?;
                            }
                        }
                        dirty = true;
                    } else {
                        let w = &mut windows[cur];
                        if let Some(p) = w.panes.iter_mut().find(|p| p.id == w.focus_id) {
                            p.write_input(&bytes)?;
                            p.scroll_to_bottom();
                        }
                    }
                }
                Msg::Attach(stream, r, c) => {
                    size = (c, r);
                    let area = size.1.saturating_sub(1);
                    for w in &mut windows {
                        relayout(&mut w.panes, &w.layout, size.0, area)?;
                    }
                    client = Some(stream);
                    dirty = true;
                    needs_clear = true;
                }
                Msg::Detach => {
                    if let Some(mut cl) = client.take() {
                        let _ = protocol::write_frame(&mut cl, protocol::DETACH, &[]);
                    }
                }
                Msg::Resize(r, c) => {
                    size = (c, r);
                    let area = size.1.saturating_sub(1);
                    for w in &mut windows {
                        relayout(&mut w.panes, &w.layout, size.0, area)?;
                    }
                    dirty = true;
                    needs_clear = true;
                }
                Msg::Toggle(role) => {
                    let (c, r) = size;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    // Flip the pane's visibility, but never hide the last one.
                    let (flag, others) = match role {
                        PaneRole::Editor => (&mut w.show_editor, w.show_shell || w.show_ai),
                        PaneRole::Shell => (&mut w.show_shell, w.show_editor || w.show_ai),
                        PaneRole::Ai => (&mut w.show_ai, w.show_editor || w.show_shell),
                    };
                    if !*flag || others {
                        let now_shown = !*flag;
                        *flag = !*flag;
                        // Showing a pane focuses it.
                        if now_shown {
                            w.focus_id = role_id(&w.panes, role);
                        }
                        refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                        relayout(&mut w.panes, &w.layout, c, area)?;
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::FocusNext => {
                    let w = &mut windows[cur];
                    let mut ls = Vec::new();
                    w.layout.leaves(&mut ls);
                    if let Some(pos) = ls.iter().position(|&x| x == w.focus_id) {
                        w.focus_id = ls[(pos + 1) % ls.len()];
                    }
                    dirty = true;
                }
                Msg::Mouse { x, y, cb, press } => {
                    let (c, r) = size;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    let mut rects = Vec::new();
                    w.layout.rects(
                        Rect {
                            x: 0,
                            y: 0,
                            w: c,
                            h: area,
                        },
                        &mut rects,
                    );
                    let (px, py) = (x.saturating_sub(1), y.saturating_sub(1));
                    let hit = rects
                        .iter()
                        .find(|(_, rc)| {
                            px >= rc.x && px < rc.x + rc.w && py >= rc.y && py < rc.y + rc.h
                        })
                        .copied();
                    if let Some((id, rect)) = hit {
                        let is_wheel = cb == 64 || cb == 65;
                        if press && !is_wheel && w.focus_id != id {
                            w.focus_id = id;
                            dirty = true;
                        }
                        let wants_mouse = w
                            .panes
                            .iter()
                            .find(|p| p.id == id)
                            .is_some_and(|p| p.wants_mouse());
                        if wants_mouse {
                            if let Some(inner) = rect.inner() {
                                if px >= inner.x
                                    && px < inner.x + inner.w
                                    && py >= inner.y
                                    && py < inner.y + inner.h
                                {
                                    let lx = px - inner.x + 1;
                                    let ly = py - inner.y + 1;
                                    let seq = format!(
                                        "\x1b[<{};{};{}{}",
                                        cb,
                                        lx,
                                        ly,
                                        if press { 'M' } else { 'm' }
                                    );
                                    if let Some(p) = w.panes.iter_mut().find(|p| p.id == id) {
                                        p.write_input(seq.as_bytes())?;
                                    }
                                }
                            }
                        } else if is_wheel {
                            if let Some(p) = w.panes.iter_mut().find(|p| p.id == id) {
                                p.scroll(if cb == 64 { 3 } else { -3 });
                            }
                            dirty = true;
                        }
                    }
                }
                Msg::Focus(dir) => {
                    let (c, r) = size;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    let mut rects = Vec::new();
                    w.layout.rects(
                        Rect {
                            x: 0,
                            y: 0,
                            w: c,
                            h: area,
                        },
                        &mut rects,
                    );
                    if let Some(id) = layout::neighbor(&rects, w.focus_id, dir) {
                        w.focus_id = id;
                    }
                    dirty = true;
                }
                Msg::ToggleHelp => {
                    show_help = !show_help;
                    dirty = true;
                    needs_clear = true;
                }
                Msg::OpenPicker => {
                    picker = if picker.is_some() {
                        None
                    } else {
                        picker_new_window = false;
                        Some(Picker::new(proot.clone()))
                    };
                    dirty = true;
                    needs_clear = true;
                }
                Msg::NewWindow => {
                    picker = Some(Picker::new(proot.clone()));
                    picker_new_window = true;
                    dirty = true;
                    needs_clear = true;
                }
                Msg::CloseWindow => {
                    for p in &mut windows[cur].panes {
                        p.kill();
                    }
                    windows.remove(cur);
                    needs_clear = true;
                    dirty = true;
                    if windows.is_empty() {
                        quit = true;
                        break;
                    }
                    if cur >= windows.len() {
                        cur = windows.len() - 1;
                    }
                    let (c, r) = size;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    relayout(&mut w.panes, &w.layout, c, area)?;
                }
                Msg::SelectWindow(n) => {
                    if n < windows.len() && n != cur {
                        cur = n;
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::Tick => {
                    // Just refresh the status-bar clock.
                    dirty = true;
                }
                Msg::Reload => {
                    // Restart the server on the latest build, restoring the same
                    // windows (open files, shell cwd, resumed AI). Capture state,
                    // tell the client to reconnect, tear down, and hand off to a
                    // fresh detached server that restores from the snapshot.
                    save_snapshot(&capture_specs(&windows));
                    if let Some(cl) = client.as_mut() {
                        let _ = protocol::write_frame(cl, protocol::RECONNECT, &[]);
                    }
                    for w in &mut windows {
                        for p in &mut w.panes {
                            p.kill();
                        }
                    }
                    let _ = std::fs::remove_file(sock);
                    spawn_server(&[])?; // restore from the snapshot just saved
                    return Ok(());
                }
                Msg::ClearSession => {
                    clear_snapshot();
                    save_enabled = false;
                }
                Msg::Quit => {
                    quit = true;
                    break;
                }
            }
        }

        if quit {
            // Capture a rich snapshot so a later `forge` restores files + cwd.
            if save_enabled {
                save_snapshot(&capture_specs(&windows));
            }
            if let Some(mut cl) = client.take() {
                let _ = protocol::write_frame(&mut cl, protocol::DETACH, &[]);
            }
            break;
        }

        // A child that exited (Ctrl-D / `exit` / `:q`) is respawned in place so
        // the pane never vanishes. A child that keeps dying fast is left dead for
        // now (crash-loop guard) and retried once it has aged out.
        let (c, r) = size;
        let area = r.saturating_sub(1);
        for w in windows.iter_mut() {
            let mut respawn: Vec<(usize, PaneRole, u32)> = Vec::new();
            for p in w.panes.iter_mut() {
                if p.is_dead() {
                    let fast = p.age() < Duration::from_millis(1500);
                    let fails = if fast { p.respawns() + 1 } else { 0 };
                    if fails < 4 {
                        respawn.push((p.id, p.role, fails));
                    }
                }
            }
            if respawn.is_empty() {
                continue;
            }
            let dir = w.dir.clone();
            for (id, role, fails) in respawn {
                let mut newp = respawn_pane(role, &dir, id, &cfg, &shell, &tx)?;
                newp.set_respawns(fails);
                if let Some(slot) = w.panes.iter_mut().find(|p| p.id == id) {
                    *slot = newp;
                }
            }
            relayout(&mut w.panes, &w.layout, c, area)?;
            dirty = true;
            needs_clear = true;
        }

        // Re-capture the snapshot when the set of window directories changes
        // (window added/removed) — cheap enough at that cadence.
        let now_dirs: Vec<PathBuf> = windows.iter().map(|w| w.dir.clone()).collect();
        if now_dirs != last_dirs {
            if save_enabled {
                save_snapshot(&capture_specs(&windows));
            }
            last_dirs = now_dirs;
        }

        // Flag a background window whose Claude pane rang the bell (wants input).
        for (idx, w) in windows.iter_mut().enumerate() {
            if let Some(ai) = w.panes.iter().find(|p| p.role == PaneRole::Ai) {
                let bc = ai.bell_count();
                if bc > w.last_ai_bell {
                    w.last_ai_bell = bc;
                    if idx != cur {
                        w.attention = true;
                        dirty = true;
                    }
                }
            }
        }
        // Viewing a window clears its attention flag.
        if let Some(w) = windows.get_mut(cur) {
            if w.attention {
                w.attention = false;
                dirty = true;
            }
        }

        if dirty {
            if let Some(cl) = client.as_mut() {
                let now = chrono::Local::now();
                let temp = weather.lock().unwrap().clone();
                let right_info = if temp.is_empty() {
                    now.format("%a %b %-d  %H:%M").to_string()
                } else {
                    format!("{}  {temp}", now.format("%a %b %-d  %H:%M"))
                };
                framebuf.clear();
                render(
                    &mut framebuf,
                    &windows,
                    cur,
                    size.0,
                    size.1,
                    needs_clear,
                    show_help,
                    picker.as_ref(),
                    &right_info,
                    keys,
                )?;
                if protocol::write_frame(cl, protocol::OUTPUT, &framebuf).is_err() {
                    client = None;
                }
            }
        }
    }

    for w in &mut windows {
        for p in &mut w.panes {
            p.kill();
        }
    }
    let _ = std::fs::remove_file(sock);
    Ok(())
}

/// Connect to the socket, retrying until it succeeds or `timeout` elapses.
fn connect_retry(sock: &Path, timeout: Duration) -> Option<UnixStream> {
    let start = Instant::now();
    loop {
        if let Ok(s) = UnixStream::connect(sock) {
            return Some(s);
        }
        if start.elapsed() > timeout {
            return None;
        }
        thread::sleep(Duration::from_millis(50));
    }
}

/// The thin client: owns the real terminal, forwards keystrokes and resizes to
/// the server, and writes whatever the server sends back to stdout. Survives a
/// server reload by reconnecting; exits on detach/quit.
fn run_client(sock: &Path) -> Result<()> {
    let mut out = io::stdout();
    enable_raw_mode().context("enabling raw mode")?;
    queue!(out, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;
    out.flush()?;
    let _guard = TerminalGuard;

    // Current write half; swapped out during a reconnect. The input/resize
    // threads below hold this and write only while a connection is present.
    let write: Arc<Mutex<Option<UnixStream>>> = Arc::new(Mutex::new(None));

    // stdin -> server (raw bytes).
    {
        let write = write.clone();
        thread::spawn(move || {
            let mut stdin = io::stdin();
            let mut buf = [0u8; 4096];
            loop {
                match stdin.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let mut g = write.lock().unwrap();
                        if let Some(s) = g.as_mut() {
                            let _ = protocol::write_frame(s, protocol::INPUT, &buf[..n]);
                        }
                    }
                }
            }
        });
    }

    // SIGWINCH -> resize frame.
    {
        let write = write.clone();
        let mut signals = Signals::new([SIGWINCH]).context("installing SIGWINCH handler")?;
        thread::spawn(move || {
            for _ in signals.forever() {
                if let Ok((c, r)) = terminal::size() {
                    let mut g = write.lock().unwrap();
                    if let Some(s) = g.as_mut() {
                        let _ = protocol::write_frame(
                            s,
                            protocol::RESIZE,
                            &protocol::size_payload(r, c),
                        );
                    }
                }
            }
        });
    }

    // Connect / reconnect loop.
    loop {
        let stream = match connect_retry(sock, Duration::from_secs(5)) {
            Some(s) => s,
            None => break, // server gone for good
        };
        {
            let (c, r) = terminal::size().unwrap_or((80, 24));
            let mut g = write.lock().unwrap();
            *g = Some(stream.try_clone().context("cloning socket")?);
            if let Some(s) = g.as_mut() {
                let _ = protocol::write_frame(s, protocol::ATTACH, &protocol::size_payload(r, c));
            }
        }

        let mut rd = stream;
        let reconnect;
        loop {
            match protocol::read_frame(&mut rd) {
                Ok((protocol::OUTPUT, p)) => {
                    out.write_all(&p)?;
                    out.flush()?;
                }
                Ok((protocol::RECONNECT, _)) => {
                    reconnect = true;
                    break;
                }
                Ok((protocol::DETACH, _)) => {
                    *write.lock().unwrap() = None;
                    return Ok(()); // clean detach/quit
                }
                Ok(_) => {}
                Err(_) => {
                    reconnect = true; // unexpected drop: try to reconnect briefly
                    break;
                }
            }
        }
        *write.lock().unwrap() = None;
        if !reconnect {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

/// Does `id` still appear as a leaf in the tree?
fn leaf_exists(layout: &Layout, id: usize) -> bool {
    let mut ls = Vec::new();
    layout.leaves(&mut ls);
    ls.contains(&id)
}

/// Resize every pane's PTY to match its current rectangle.
fn relayout(panes: &mut [Pane], layout: &Layout, cols: u16, rows: u16) -> Result<()> {
    let mut rects = Vec::new();
    layout.rects(
        Rect {
            x: 0,
            y: 0,
            w: cols,
            h: rows,
        },
        &mut rects,
    );
    for (id, rect) in rects {
        if let (Some(inner), Some(p)) = (rect.inner(), panes.iter_mut().find(|p| p.id == id)) {
            p.resize(inner.h, inner.w)?;
        }
    }
    Ok(())
}

/// Reader state: what we're in the middle of parsing on the stdin byte stream.
enum InState {
    /// Normal keystrokes.
    Normal,
    /// Saw the `Ctrl-a` prefix; next byte is a command.
    Prefix,
    /// Saw `ESC`; might be the start of a CSI/mouse sequence.
    Esc,
    /// Saw `ESC [`; a `<` next means an SGR mouse sequence.
    Csi,
    /// Inside an SGR mouse sequence (`ESC [ < ...`), collecting `cb;x;y`.
    Mouse,
}

/// Parses the client's raw byte stream into `Msg`s: intercepts prefix commands
/// and SGR mouse events, and passes everything else through as `Msg::Input`.
///
/// Mouse events arrive as `ESC [ < cb;x;y M` (press/drag) or `... m` (release);
/// non-mouse escape sequences (arrows, etc.) pass straight through.
struct InputParser {
    state: InState,
    mouse: Vec<u8>,
    passthrough: Vec<u8>,
    prefix: u8,
    keys: Keys,
}

impl InputParser {
    fn new(prefix: u8, keys: Keys) -> InputParser {
        InputParser {
            state: InState::Normal,
            mouse: Vec::new(),
            passthrough: Vec::new(),
            prefix: prefix.max(1),
            keys,
        }
    }

    /// Flush buffered passthrough as a `Msg::Input` into `out`.
    fn flush(&mut self, out: &mut Vec<Msg>) {
        if !self.passthrough.is_empty() {
            out.push(Msg::Input(std::mem::take(&mut self.passthrough)));
        }
    }

    /// Feed a chunk of input bytes, appending resulting messages to `out`.
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<Msg>) {
        for &b in bytes {
            match self.state {
                InState::Normal => match b {
                    _ if b == self.prefix => self.state = InState::Prefix,
                    0x1b => self.state = InState::Esc,
                    _ => self.passthrough.push(b),
                },
                InState::Prefix => {
                    self.state = InState::Normal;
                    let k = self.keys;
                    let c = b as char;
                    let cmd = if c == k.quit {
                        Some(Msg::Quit)
                    } else if c == k.cycle {
                        Some(Msg::FocusNext)
                    } else if c == k.toggle_editor {
                        Some(Msg::Toggle(PaneRole::Editor))
                    } else if c == k.toggle_shell {
                        Some(Msg::Toggle(PaneRole::Shell))
                    } else if c == k.toggle_ai {
                        Some(Msg::Toggle(PaneRole::Ai))
                    } else if c == k.focus_left {
                        Some(Msg::Focus(FocusDir::Left))
                    } else if c == k.focus_down {
                        Some(Msg::Focus(FocusDir::Down))
                    } else if c == k.focus_up {
                        Some(Msg::Focus(FocusDir::Up))
                    } else if c == k.focus_right {
                        Some(Msg::Focus(FocusDir::Right))
                    } else if c == k.help {
                        Some(Msg::ToggleHelp)
                    } else if c == k.picker {
                        Some(Msg::OpenPicker)
                    } else if c == k.win_new {
                        Some(Msg::NewWindow)
                    } else if c == k.win_close {
                        Some(Msg::CloseWindow)
                    } else if c == k.detach {
                        Some(Msg::Detach)
                    } else if c == k.reload {
                        Some(Msg::Reload)
                    } else if c == k.fresh {
                        Some(Msg::ClearSession)
                    } else if b.is_ascii_digit() && b != b'0' {
                        Some(Msg::SelectWindow((b - b'1') as usize))
                    } else if b == self.prefix {
                        // prefix prefix -> a literal prefix byte to the child.
                        self.passthrough.push(self.prefix);
                        None
                    } else {
                        None
                    };
                    if let Some(m) = cmd {
                        self.flush(out);
                        out.push(m);
                    }
                }
                InState::Esc => match b {
                    b'[' => self.state = InState::Csi,
                    _ => {
                        self.passthrough.push(0x1b);
                        if b == self.prefix {
                            self.state = InState::Prefix;
                        } else if b == 0x1b {
                            self.state = InState::Esc;
                        } else {
                            self.passthrough.push(b);
                            self.state = InState::Normal;
                        }
                    }
                },
                InState::Csi => {
                    if b == b'<' {
                        self.state = InState::Mouse;
                        self.mouse.clear();
                    } else {
                        self.passthrough.extend_from_slice(b"\x1b[");
                        self.passthrough.push(b);
                        self.state = InState::Normal;
                    }
                }
                InState::Mouse => {
                    if b == b'M' || b == b'm' {
                        self.state = InState::Normal;
                        if let Some(ev) = parse_mouse(&self.mouse, b == b'M') {
                            self.flush(out);
                            out.push(ev);
                        }
                    } else {
                        self.mouse.push(b);
                    }
                }
            }
        }
        // Trailing passthrough (no command pending) goes out too.
        self.flush(out);
    }
}

/// Parse the `cb;x;y` body of an SGR mouse sequence into a `Msg::Mouse`.
fn parse_mouse(body: &[u8], press: bool) -> Option<Msg> {
    let s = std::str::from_utf8(body).ok()?;
    let mut parts = s.split(';');
    let cb: u16 = parts.next()?.parse().ok()?;
    let x: u16 = parts.next()?.parse().ok()?;
    let y: u16 = parts.next()?.parse().ok()?;
    Some(Msg::Mouse { x, y, cb, press })
}

/// Paint the current window's panes, the status bar, and any overlay, then
/// place the hardware cursor in the focused pane. Per-cell so panes can tile.
/// The layout occupies `rows - 1`; the last row is the status bar.
#[allow(clippy::too_many_arguments)]
fn render(
    out: &mut Vec<u8>,
    windows: &[Window],
    cur: usize,
    cols: u16,
    rows: u16,
    clear: bool,
    show_help: bool,
    picker: Option<&Picker>,
    right_info: &str,
    keys: Keys,
) -> Result<()> {
    let w = &windows[cur];
    let area = rows.saturating_sub(1);
    let mut rects = Vec::new();
    w.layout.rects(
        Rect {
            x: 0,
            y: 0,
            w: cols,
            h: area,
        },
        &mut rects,
    );

    // The panes tile the whole area and we repaint every cell, so a clear is
    // only needed when the geometry shrank (and could leave stale cells).
    queue!(out, cursor::Hide, ResetColor)?;
    if clear {
        queue!(out, terminal::Clear(terminal::ClearType::All))?;
    }

    for (id, rect) in &rects {
        if let Some(p) = w.panes.iter().find(|p| p.id == *id) {
            draw_border(out, rect, &p.title, *id == w.focus_id)?;
            if let Some(inner) = rect.inner() {
                blit_pane(out, p.screen(), inner)?;
            }
        }
    }

    draw_status(out, cols, rows, windows, cur, right_info)?;

    // Position the real cursor inside the focused pane.
    if let Some((_, rect)) = rects.iter().find(|(id, _)| *id == w.focus_id) {
        if let (Some(inner), Some(p)) = (rect.inner(), w.panes.iter().find(|p| p.id == w.focus_id))
        {
            let screen = p.screen();
            let (crow, ccol) = screen.cursor_position();
            if !screen.hide_cursor() && crow < inner.h && ccol < inner.w {
                queue!(
                    out,
                    cursor::MoveTo(inner.x + ccol, inner.y + crow),
                    cursor::Show
                )?;
            }
        }
    }

    if show_help {
        draw_help(out, cols, rows, keys)?;
    }
    if let Some(pk) = picker {
        pk.render(out, cols, rows)?;
    }
    out.flush()?;
    Ok(())
}

/// Draw the bottom status bar: window tabs on the left (current highlighted,
/// orange when a window wants attention), and the date/clock/temperature on the
/// right. Keybinds live in the `Ctrl-a ?` overlay, not here.
fn draw_status(
    out: &mut Vec<u8>,
    cols: u16,
    rows: u16,
    windows: &[Window],
    cur: usize,
    right_info: &str,
) -> Result<()> {
    let y = rows.saturating_sub(1);
    // Fill the row so old content is covered.
    queue!(
        out,
        cursor::MoveTo(0, y),
        SetBackgroundColor(Color::DarkGrey),
        SetForegroundColor(Color::White),
        Print(" ".repeat(cols as usize)),
        cursor::MoveTo(0, y),
        Print(" forge "),
    )?;
    for (i, w) in windows.iter().enumerate() {
        let (bg, fg) = if i == cur {
            (Color::Cyan, Color::Black)
        } else if w.attention {
            (
                Color::Rgb {
                    r: 230,
                    g: 130,
                    b: 20,
                },
                Color::Black,
            ) // orange: wants input
        } else {
            (Color::DarkGrey, Color::White)
        };
        queue!(out, SetBackgroundColor(bg), SetForegroundColor(fg))?;
        queue!(out, Print(format!(" {}:{} ", i + 1, w.title)))?;
    }

    // Right-aligned date / clock / temperature.
    if !right_info.is_empty() {
        let text = format!(" {right_info} ");
        let w = text.chars().count() as u16;
        if w + 1 < cols {
            queue!(
                out,
                cursor::MoveTo(cols - w, y),
                SetBackgroundColor(Color::DarkGrey),
                SetForegroundColor(Color::White),
                Print(text),
            )?;
        }
    }
    queue!(out, ResetColor)?;
    Ok(())
}

/// Draw a centered overlay listing the CodeForge keybindings, built from the
/// live config so it reflects any customizations.
fn draw_help(out: &mut Vec<u8>, cols: u16, rows: u16, k: Keys) -> Result<()> {
    // (key display, label). Ctrl-a is the prefix for all of these.
    let binds: [(String, &str); 15] = [
        (k.toggle_editor.to_string(), "show/hide editor"),
        (k.toggle_shell.to_string(), "show/hide terminal"),
        (k.toggle_ai.to_string(), "show/hide Claude"),
        ("hjkl".into(), "move focus"),
        (k.cycle.to_string(), "cycle focus"),
        (k.picker.to_string(), "switch project"),
        (k.win_new.to_string(), "new window"),
        (k.win_close.to_string(), "close window"),
        ("1..9".into(), "jump to window"),
        (k.detach.to_string(), "detach (stays alive)"),
        (k.reload.to_string(), "reload (new build)"),
        (k.fresh.to_string(), "forget saved session"),
        (k.quit.to_string(), "quit (ends session)"),
        (k.help.to_string(), "toggle this help"),
        ("click".into(), "focus / wheel scrolls"),
    ];
    let mut lines: Vec<String> = Vec::new();
    lines.push("  CodeForge — keys (prefix Ctrl-a)  ".to_string());
    lines.push(String::new());
    for (key, label) in &binds {
        lines.push(format!("  Ctrl-a {key:<5}  {label}"));
    }
    lines.push(String::new());
    lines.push("  edit ~/.config/codeforge/config.toml  ".to_string());

    let w = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16 + 2;
    let h = lines.len() as u16 + 2;
    if w > cols || h > rows {
        return Ok(());
    }
    let x = (cols - w) / 2;
    let y = (rows - h) / 2;

    queue!(
        out,
        cursor::Hide,
        SetForegroundColor(Color::Black),
        SetBackgroundColor(Color::Cyan)
    )?;
    // Top/bottom borders and body.
    queue!(
        out,
        cursor::MoveTo(x, y),
        Print(format!("┌{}┐", "─".repeat((w - 2) as usize)))
    )?;
    for (i, line) in lines.iter().enumerate() {
        let padded = format!("{line:<width$}", width = (w - 2) as usize);
        queue!(
            out,
            cursor::MoveTo(x, y + 1 + i as u16),
            Print(format!("│{padded}│"))
        )?;
    }
    queue!(
        out,
        cursor::MoveTo(x, y + h - 1),
        Print(format!("└{}┘", "─".repeat((w - 2) as usize)))
    )?;
    queue!(out, ResetColor, SetAttribute(Attribute::Reset))?;
    Ok(())
}

/// Blit one emulator screen into `inner`, cell by cell, preserving attributes.
fn blit_pane(out: &mut Vec<u8>, screen: &vt100::Screen, inner: Rect) -> Result<()> {
    for row in 0..inner.h {
        queue!(out, cursor::MoveTo(inner.x, inner.y + row))?;
        for col in 0..inner.w {
            let cell = screen.cell(row, col);
            queue!(out, SetAttribute(Attribute::Reset))?;
            if let Some(cell) = cell {
                if let Some(c) = conv_color(cell.fgcolor()) {
                    queue!(out, SetForegroundColor(c))?;
                }
                if let Some(c) = conv_color(cell.bgcolor()) {
                    queue!(out, SetBackgroundColor(c))?;
                }
                if cell.bold() {
                    queue!(out, SetAttribute(Attribute::Bold))?;
                }
                if cell.italic() {
                    queue!(out, SetAttribute(Attribute::Italic))?;
                }
                if cell.underline() {
                    queue!(out, SetAttribute(Attribute::Underlined))?;
                }
                if cell.inverse() {
                    queue!(out, SetAttribute(Attribute::Reverse))?;
                }
                let contents = cell.contents();
                if contents.is_empty() {
                    queue!(out, Print(' '))?;
                } else {
                    queue!(out, Print(contents))?;
                }
            } else {
                queue!(out, Print(' '))?;
            }
        }
    }
    queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
    Ok(())
}

/// Draw a box border around `rect`, with `title` on the top edge. The focused
/// pane's border is bright; others are dim.
fn draw_border(out: &mut Vec<u8>, rect: &Rect, title: &str, focused: bool) -> Result<()> {
    if rect.w < 2 || rect.h < 2 {
        return Ok(());
    }
    let color = if focused {
        Color::Cyan
    } else {
        Color::DarkGrey
    };
    queue!(out, SetForegroundColor(color))?;
    if focused {
        queue!(out, SetAttribute(Attribute::Bold))?;
    }

    let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
    let right = x + w - 1;
    let bottom = y + h - 1;

    // Top edge with an embedded title: ┌─ title ─...─┐
    let inner_w = (w - 2) as usize;
    let label = format!(" {title} ");
    let label: String = label.chars().take(inner_w).collect();
    let dashes = inner_w.saturating_sub(label.chars().count());
    let top = format!("┌{}{}┐", label, "─".repeat(dashes));
    queue!(out, cursor::MoveTo(x, y), Print(top))?;

    // Bottom edge.
    let bot = format!("└{}┘", "─".repeat(inner_w));
    queue!(out, cursor::MoveTo(x, bottom), Print(bot))?;

    // Side edges.
    for row in (y + 1)..bottom {
        queue!(out, cursor::MoveTo(x, row), Print('│'))?;
        queue!(out, cursor::MoveTo(right, row), Print('│'))?;
    }

    queue!(out, SetAttribute(Attribute::Reset), ResetColor)?;
    Ok(())
}

/// Map a vt100 color to a crossterm color. `Default` returns `None` so the
/// terminal's own default is used.
fn conv_color(c: vt100::Color) -> Option<Color> {
    match c {
        vt100::Color::Default => None,
        vt100::Color::Idx(i) => Some(Color::AnsiValue(i)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb { r, g, b }),
    }
}

/// Restores the terminal on drop, so panics and `?` early-returns don't leave
/// the user's shell in raw-mode/alt-screen limbo.
struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let mut out = io::stdout();
        let _ = queue!(out, cursor::Show, DisableMouseCapture, LeaveAlternateScreen);
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sgr_mouse() {
        // ESC [ < 0 ; 12 ; 5 M  -> left button press at (12,5)
        match parse_mouse(b"0;12;5", true) {
            Some(Msg::Mouse { x, y, cb, press }) => {
                assert_eq!((x, y, cb, press), (12, 5, 0, true));
            }
            _ => panic!("expected a mouse event"),
        }
        assert!(parse_mouse(b"garbage", true).is_none());
    }
}
