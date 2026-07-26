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
/// Read the aggregate CPU counters from `/proc/stat` as `(idle, total)` jiffies.
fn read_cpu() -> Option<(u64, u64)> {
    let stat = std::fs::read_to_string("/proc/stat").ok()?;
    let line = stat.lines().next()?; // "cpu  user nice system idle iowait irq ..."
    let vals: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|v| v.parse().ok())
        .collect();
    if vals.len() < 4 {
        return None;
    }
    let idle = vals[3] + vals.get(4).copied().unwrap_or(0); // idle + iowait
    let total: u64 = vals.iter().sum();
    Some((idle, total))
}

/// CPU busy percentage between two `/proc/stat` samples.
fn cpu_percent(prev: (u64, u64), cur: (u64, u64)) -> u8 {
    let dt = cur.1.saturating_sub(prev.1);
    let di = cur.0.saturating_sub(prev.0);
    if dt == 0 {
        return 0;
    }
    (((dt - di) as f64 / dt as f64) * 100.0).round() as u8
}

/// Used-memory percentage from `/proc/meminfo`.
fn mem_percent() -> Option<u8> {
    let info = std::fs::read_to_string("/proc/meminfo").ok()?;
    let mut total = 0u64;
    let mut avail = 0u64;
    for line in info.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("MemTotal:") => total = it.next()?.parse().ok()?,
            Some("MemAvailable:") => avail = it.next()?.parse().ok()?,
            _ => {}
        }
    }
    if total == 0 {
        return None;
    }
    Some((((total - avail) as f64 / total as f64) * 100.0).round() as u8)
}

/// Used-disk percentage for the filesystem holding `path`, via `df`.
fn disk_percent(path: &Path) -> Option<u8> {
    let out = Command::new("df").arg("-P").arg(path).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8_lossy(&out.stdout);
    // Second line, 5th column is the use percentage ("63%").
    let line = s.lines().nth(1)?;
    let pct = line.split_whitespace().nth(4)?.trim_end_matches('%');
    pct.parse().ok()
}

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
/// Bounded by `timeout` so a busy/starting nvim can never hang the server.
fn query_nvim_files(sock: &Path) -> Vec<PathBuf> {
    if !sock.exists() {
        return Vec::new();
    }
    let expr =
        r#"join(filter(map(getbufinfo({"buflisted":1}), "v:val.name"), "v:val != \"\""), "\n")"#;
    let out = Command::new("timeout")
        .arg("2")
        .arg("nvim")
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

/// Close nvim's current buffer (an editor "tab") over its RPC socket, keeping
/// the window/nvim alive. `bp|bd#` switches to the previous buffer then deletes
/// the one we left; `silent!` swallows the last-buffer case. Best-effort.
fn nvim_close_buffer(sock: &Path) {
    if !sock.exists() {
        return;
    }
    let _ = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg("execute('silent! bp|bd#')")
        .output();
}

/// Cycle the editor's open buffers over RPC, so the same prev/next-tab keys that
/// stack the terminal/Claude slots also flip through nvim's buffers (its "tabs")
/// when the editor is focused. Uses bufferline's cycle so the order matches the
/// visible tab bar; `silent!` keeps it quiet if bufferline isn't loaded.
fn nvim_cycle_buffer(sock: &Path, delta: i32) {
    if !sock.exists() {
        return;
    }
    let cmd = if delta >= 0 {
        "BufferLineCycleNext"
    } else {
        "BufferLineCyclePrev"
    };
    let _ = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg(format!("execute('silent! {cmd}')"))
        .output();
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
    /// Enter copy/scroll mode on the focused pane.
    CopyMode,
    /// New child (tab) in the focused slot.
    TabNew,
    /// Cycle the focused slot's active child (+1 next, -1 prev).
    TabCycle(i32),
    /// Close the focused slot's active child.
    TabClose,
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
    /// All child panes across the three slots. A slot (terminal or Claude) can
    /// hold several children (tabs, #12); only its active one is laid out.
    panes: Vec<Pane>,
    layout: Layout,
    focus_id: usize,
    /// Active child id per slot, indexed by `role_index` (editor, shell, ai).
    active: [usize; 3],
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

/// Slot index for a role: editor 0, shell 1, ai 2. Used to index `Window.active`.
fn role_index(role: PaneRole) -> usize {
    match role {
        PaneRole::Editor => 0,
        PaneRole::Shell => 1,
        PaneRole::Ai => 2,
    }
}

impl Window {
    /// Ids of this slot's children, in tab order.
    fn slot_ids(&self, role: PaneRole) -> Vec<usize> {
        self.panes
            .iter()
            .filter(|p| p.role == role)
            .map(|p| p.id)
            .collect()
    }

    /// (position of the active child, total children) for a slot.
    fn slot_pos(&self, role: PaneRole) -> (usize, usize) {
        let ids = self.slot_ids(role);
        let active = self.active[role_index(role)];
        let pos = ids.iter().position(|&i| i == active).unwrap_or(0);
        (pos, ids.len())
    }

    /// The role of the currently focused child.
    fn focus_role(&self) -> Option<PaneRole> {
        self.panes
            .iter()
            .find(|p| p.id == self.focus_id)
            .map(|p| p.role)
    }
}

/// Derive the layout tree from which panes are visible. Editor (when shown) is a
/// full-height left column; terminal-over-AI stack on the right. With the editor
/// hidden, the remaining panes go side by side (AI left, terminal right). Returns
/// `None` if nothing is visible.
#[allow(clippy::too_many_arguments)]
fn compute_layout(
    e: usize,
    s: usize,
    a: usize,
    show_editor: bool,
    show_shell: bool,
    show_ai: bool,
    editor_ratio: f32,
    right_ratio: f32,
) -> Option<Layout> {
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
        w.active[0],
        w.active[1],
        w.active[2],
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
    // spawn_ide assigns editor=base, shell=base+1, ai=base+2. All three always
    // exist; config only chooses which start visible (#17). Never zero visible.
    let active = [base, base + 1, base + 2];
    let (mut se, ss, sa) = (cfg.start_editor, cfg.start_terminal, cfg.start_ai);
    if !se && !ss && !sa {
        se = true;
    }
    let layout = compute_layout(
        active[0],
        active[1],
        active[2],
        se,
        ss,
        sa,
        cfg.editor_ratio,
        cfg.right_ratio,
    )
    .unwrap_or(Layout::Leaf(base));
    // Focus the first visible pane.
    let focus_id = layout.first_leaf().unwrap_or(base);
    let title = dir_title(&spec.dir);
    Ok(Window {
        panes,
        layout,
        focus_id,
        active,
        dir: spec.dir.clone(),
        title,
        show_editor: se,
        show_shell: ss,
        show_ai: sa,
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
        if !cfg.wrap {
            c.arg("-c");
            c.arg("set nowrap");
        }
        // init.lua autosaves unless this global is 0 (#19).
        if !cfg.autosave {
            c.arg("-c");
            c.arg("let g:codeforge_autosave=0");
        }
    }
    if !files.is_empty() {
        for f in files {
            c.arg(f);
        }
    } else if !is_nvim {
        // A generic editor opens the project dir.
        c.arg(".");
    }
    // nvim with no files: start on an empty buffer (its cwd is the project dir),
    // so a directory explorer isn't opened over which Ctrl-P would stack (#14).
    // Ctrl-P / live-grep still work because telescope searches the cwd.
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
    // Shared so the `Ctrl-a ?` editor can rebind keys live: reader-thread
    // parsers and the event loop all see the same mapping.
    let keys = Arc::new(Mutex::new(cfg.keys));
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
        // Resume the previous AI conversation on restore. `--continue` is
        // claude-specific; other AI CLIs (e.g. augment, #13) just relaunch.
        if cfg.ai.trim_start().starts_with("claude") {
            cfg.ai = format!("{} --continue", cfg.ai.trim());
        }
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
        let keys = keys.clone();
        thread::spawn(move || {
            for conn in listener.incoming() {
                let stream = match conn {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let tx = tx.clone();
                let keys = keys.clone();
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

    // Status-bar system metrics (cpu / ram / disk), refreshed every few seconds.
    let metrics = Arc::new(Mutex::new(String::new()));
    if cfg.status_metrics {
        let metrics = metrics.clone();
        let tx = tx.clone();
        let disk_path = proot.clone();
        thread::spawn(move || {
            let mut prev = read_cpu().unwrap_or((0, 0));
            loop {
                thread::sleep(Duration::from_secs(3));
                let cur = read_cpu().unwrap_or(prev);
                let cpu = cpu_percent(prev, cur);
                prev = cur;
                let mut parts = vec![format!("cpu {cpu}%")];
                if let Some(m) = mem_percent() {
                    parts.push(format!("ram {m}%"));
                }
                if let Some(d) = disk_percent(&disk_path) {
                    parts.push(format!("dsk {d}%"));
                }
                *metrics.lock().unwrap() = parts.join(" ");
                let _ = tx.send(Msg::Tick);
            }
        });
    }

    let mut windows: Vec<Window> = Vec::new();
    let mut next_id: usize = 0;
    for spec in &specs {
        windows.push(new_window(spec, &cfg, &shell, next_id, &tx)?);
        next_id += 3;
    }
    let mut cur = 0usize;
    let mut help: Option<HelpState> = None;
    let mut copy: Option<CopyMode> = None;
    let mut msel: Option<MouseSel> = None;
    let mut picker: Option<Picker> = None;
    let mut picker_new_window = false;
    // (cols, rows) of the attached client; updated on attach/resize.
    let mut size = (80u16, 24u16);
    let mut client: Option<UnixStream> = None;
    let mut framebuf: Vec<u8> = Vec::new();
    // Whether to keep persisting the session (Ctrl-a fresh turns this off).
    let mut save_enabled = true;

    // Persist the session so a fresh `forge` can restore it. Save the specs we
    // started from (don't query the just-launched nvim yet — it's still loading,
    // and its RPC would block); richer capture happens on change / teardown.
    let mut last_dirs: Vec<PathBuf> = windows.iter().map(|w| w.dir.clone()).collect();
    save_snapshot(&specs);

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
                    if let Some(cm) = copy.as_mut() {
                        // Copy/scroll mode is modal on its pane.
                        let w = &mut windows[cur];
                        let mut done = false;
                        if let Some(p) = w.panes.iter_mut().find(|p| p.id == cm.pane_id) {
                            let (exit, clip) = copy_input(cm, p, &bytes);
                            if let Some(text) = clip {
                                if !text.is_empty() {
                                    if let Some(cl) = client.as_mut() {
                                        let _ = protocol::write_frame(
                                            cl,
                                            protocol::OUTPUT,
                                            &osc52(&text),
                                        );
                                    }
                                }
                            }
                            if exit {
                                p.unfreeze();
                                done = true;
                            }
                        } else {
                            done = true; // pane vanished
                        }
                        if done {
                            copy = None;
                        }
                        dirty = true;
                        needs_clear = true;
                    } else if let Some(h) = help.as_mut() {
                        // The help overlay is modal: it captures keystrokes for
                        // navigation / live rebinding instead of the focused pane.
                        if help_input(h, &bytes, &keys) {
                            help = None;
                            needs_clear = true;
                        }
                        dirty = true;
                    } else if let Some(pk) = picker.as_mut() {
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
                        // Showing a pane focuses its active child.
                        if now_shown {
                            w.focus_id = w.active[role_index(role)];
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
                        // SGR cb bits: low 2 = button (0=left), bit5 (32) = motion
                        // (drag), bit6 (64) = wheel.
                        let is_wheel = cb & 64 != 0;
                        let is_motion = cb & 32 != 0;
                        let button = cb & 3;
                        if press && !is_wheel && !is_motion && w.focus_id != id {
                            w.focus_id = id;
                            dirty = true;
                        }
                        let wants_mouse = w
                            .panes
                            .iter()
                            .find(|p| p.id == id)
                            .is_some_and(|p| p.wants_mouse());
                        let inner = rect.inner();
                        // Cell under the pointer within the pane's content rect.
                        let cell = inner.and_then(|i| {
                            (px >= i.x && px < i.x + i.w && py >= i.y && py < i.y + i.h)
                                .then(|| (py - i.y, px - i.x))
                        });

                        let forward = |w: &mut Window, cb: u16, press: bool| -> Result<()> {
                            if let Some(i) = inner {
                                if px >= i.x && px < i.x + i.w && py >= i.y && py < i.y + i.h {
                                    let seq = format!(
                                        "\x1b[<{};{};{}{}",
                                        cb,
                                        px - i.x + 1,
                                        py - i.y + 1,
                                        if press { 'M' } else { 'm' }
                                    );
                                    if let Some(p) = w.panes.iter_mut().find(|p| p.id == id) {
                                        p.write_input(seq.as_bytes())?;
                                    }
                                }
                            }
                            Ok(())
                        };

                        if is_wheel {
                            // Wheel: forward to mouse-aware apps, else scroll our
                            // own scrollback.
                            if wants_mouse {
                                forward(w, cb, press)?;
                            } else if let Some(p) = w.panes.iter_mut().find(|p| p.id == id) {
                                p.scroll(if cb & 1 == 0 { 3 } else { -3 });
                                dirty = true;
                            }
                        } else if button == 0 && press && !is_motion {
                            // Left press: begin a selection at this cell (don't
                            // forward yet — a click without drag is forwarded on
                            // release so app buttons still work).
                            if let Some(c) = cell {
                                msel = Some(MouseSel {
                                    pane_id: id,
                                    anchor: c,
                                    cur: c,
                                    moved: false,
                                });
                            }
                        } else if is_motion && button == 0 {
                            // Left drag: extend the selection.
                            if let (Some(ms), Some(c)) = (msel.as_mut(), cell) {
                                if ms.pane_id == id && c != ms.cur {
                                    ms.cur = c;
                                    ms.moved = true;
                                    dirty = true;
                                }
                            }
                        } else if !press {
                            // Release: finish selection. A real drag copies via
                            // OSC 52; a plain click is forwarded to the app.
                            match msel.take() {
                                Some(ms) if ms.moved => {
                                    let (s, e) = ms.range();
                                    let text = w
                                        .panes
                                        .iter()
                                        .find(|p| p.id == ms.pane_id)
                                        .map(|p| extract_range(p.screen(), s, e))
                                        .unwrap_or_default();
                                    if !text.is_empty() {
                                        if let Some(cl) = client.as_mut() {
                                            let _ = protocol::write_frame(
                                                cl,
                                                protocol::OUTPUT,
                                                &osc52(&text),
                                            );
                                        }
                                    }
                                    dirty = true;
                                }
                                _ if wants_mouse => {
                                    // A click (no drag): send press then release
                                    // so the app registers a full click.
                                    forward(w, button, true)?;
                                    forward(w, button, false)?;
                                }
                                _ => {}
                            }
                        } else if wants_mouse {
                            // Other buttons (right/middle) go to the app as-is.
                            forward(w, cb, press)?;
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
                    exit_copy(&mut copy, &mut windows);
                    help = if help.is_some() {
                        None
                    } else {
                        Some(HelpState::new())
                    };
                    dirty = true;
                    needs_clear = true;
                }
                Msg::CopyMode => {
                    // Enter copy/scroll mode on the focused pane (no overlays up).
                    if help.is_none() && picker.is_none() {
                        let w = &mut windows[cur];
                        let id = w.focus_id;
                        if let Some(p) = w.panes.iter_mut().find(|p| p.id == id) {
                            p.freeze(); // hold live output so the view stays put
                        }
                        copy = Some(CopyMode::new(id));
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::OpenPicker => {
                    exit_copy(&mut copy, &mut windows);
                    help = None;
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
                    exit_copy(&mut copy, &mut windows);
                    help = None;
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
                Msg::TabNew => {
                    // Add another child to the focused slot. The editor stays a
                    // single nvim (use its native buffers/tabs), so this only
                    // applies to the terminal and Claude slots.
                    let (c, r) = size;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    if let Some(role) = w.focus_role() {
                        if role != PaneRole::Editor {
                            let id = next_id;
                            next_id += 1;
                            let dir = w.dir.clone();
                            let pane = if role == PaneRole::Shell {
                                Pane::spawn(
                                    command(&shell, &dir),
                                    "shell".into(),
                                    PaneRole::Shell,
                                    1,
                                    1,
                                    id,
                                    tx.clone(),
                                )?
                            } else {
                                let (cmd, title) = command_line(&cfg.ai, &dir);
                                Pane::spawn(cmd, title, PaneRole::Ai, 1, 1, id, tx.clone())?
                            };
                            w.panes.push(pane);
                            w.active[role_index(role)] = id;
                            w.focus_id = id;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            relayout(&mut w.panes, &w.layout, c, area)?;
                            dirty = true;
                            needs_clear = true;
                        }
                    }
                }
                Msg::TabCycle(delta) => {
                    let (c, r) = size;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    if let Some(PaneRole::Editor) = w.focus_role() {
                        // Editor is a single nvim: cycle its buffers (its tabs)
                        // instead of a slot stack.
                        nvim_cycle_buffer(&nvim_sock(w.focus_id), delta);
                        dirty = true;
                    } else if let Some(role) = w.focus_role() {
                        let ids = w.slot_ids(role);
                        if ids.len() > 1 {
                            let (pos, n) = w.slot_pos(role);
                            let next = (pos as i32 + delta).rem_euclid(n as i32) as usize;
                            let nid = ids[next];
                            w.active[role_index(role)] = nid;
                            w.focus_id = nid;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            relayout(&mut w.panes, &w.layout, c, area)?;
                            dirty = true;
                            needs_clear = true;
                        }
                    }
                }
                Msg::TabClose => {
                    let (c, r) = size;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    if let Some(PaneRole::Editor) = w.focus_role() {
                        // Editor is a single nvim: close the current buffer (tab)
                        // over RPC, leaving nvim alive.
                        nvim_close_buffer(&nvim_sock(w.focus_id));
                    } else if let Some(role) = w.focus_role() {
                        let ids = w.slot_ids(role);
                        if ids.len() > 1 {
                            // Kill + drop the active child, activate a neighbour.
                            let active = w.active[role_index(role)];
                            let (pos, _) = w.slot_pos(role);
                            if let Some(idx) = w.panes.iter().position(|p| p.id == active) {
                                w.panes[idx].kill();
                                w.panes.remove(idx);
                            }
                            let remaining = w.slot_ids(role);
                            let nid = remaining[pos.min(remaining.len() - 1)];
                            w.active[role_index(role)] = nid;
                            w.focus_id = nid;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            relayout(&mut w.panes, &w.layout, c, area)?;
                            dirty = true;
                            needs_clear = true;
                        } else {
                            // Last child of the slot: kill it and let the respawn
                            // loop bring a fresh one back (like typing `exit`).
                            let active = w.active[role_index(role)];
                            if let Some(p) = w.panes.iter_mut().find(|p| p.id == active) {
                                p.kill();
                            }
                        }
                    }
                }
                Msg::SelectWindow(n) => {
                    if n < windows.len() && n != cur {
                        exit_copy(&mut copy, &mut windows);
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
                // Right side, left-to-right: metrics, weather, date, clock — each
                // gated by its config toggle (#16).
                let mut segs: Vec<String> = Vec::new();
                if cfg.status_metrics {
                    let m = metrics.lock().unwrap().clone();
                    if !m.is_empty() {
                        segs.push(m);
                    }
                }
                if cfg.status_weather {
                    let temp = weather
                        .lock()
                        .unwrap()
                        .trim_start_matches('+')
                        .trim()
                        .to_string();
                    if !temp.is_empty() {
                        segs.push(temp);
                    }
                }
                if cfg.status_date {
                    segs.push(now.format("%a %b %-d").to_string());
                }
                if cfg.status_clock {
                    segs.push(now.format("%H:%M").to_string());
                }
                let right_info = segs.join("  ");
                framebuf.clear();
                let ksnap = *keys.lock().unwrap();
                render(
                    &mut framebuf,
                    &windows,
                    cur,
                    size.0,
                    size.1,
                    needs_clear,
                    help.as_ref(),
                    copy.as_ref(),
                    picker.as_ref(),
                    &right_info,
                    ksnap,
                    msel.as_ref().map(|m| {
                        let (s, e) = m.range();
                        (m.pane_id, s, e)
                    }),
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
    /// Shared with the event loop so live rebinds (the `Ctrl-a ?` editor) take
    /// effect immediately: the parser reads the current mapping on each prefix.
    keys: Arc<Mutex<Keys>>,
}

impl InputParser {
    fn new(prefix: u8, keys: Arc<Mutex<Keys>>) -> InputParser {
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
        let n = bytes.len();
        for (i, &b) in bytes.iter().enumerate() {
            let last = i + 1 == n;
            match self.state {
                InState::Normal => match b {
                    _ if b == self.prefix => self.state = InState::Prefix,
                    // A bare ESC (last byte of the chunk) is a real Esc keypress:
                    // pass it straight through. Terminals deliver escape
                    // sequences (arrows, mouse) atomically, so an ESC that isn't
                    // the last byte begins a sequence -> collect it instead. This
                    // both fixes the previously-swallowed lone ESC and lets the
                    // help overlay close on Esc.
                    0x1b if last => self.passthrough.push(0x1b),
                    0x1b => self.state = InState::Esc,
                    _ => self.passthrough.push(b),
                },
                InState::Prefix => {
                    self.state = InState::Normal;
                    let k = *self.keys.lock().unwrap();
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
                    } else if c == k.tab_new {
                        Some(Msg::TabNew)
                    } else if c == k.tab_next {
                        Some(Msg::TabCycle(1))
                    } else if c == k.tab_prev {
                        Some(Msg::TabCycle(-1))
                    } else if c == k.tab_close {
                        Some(Msg::TabClose)
                    } else if c == k.copy {
                        Some(Msg::CopyMode)
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
    help: Option<&HelpState>,
    copy: Option<&CopyMode>,
    picker: Option<&Picker>,
    right_info: &str,
    keys: Keys,
    msel: Option<Sel>,
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

    // Synchronized Output (DEC private mode 2026): tell the terminal to buffer
    // this whole frame and present it atomically, instead of drawing cell-by-cell
    // as the bytes arrive. This is what kills the scroll/redraw flicker — we
    // repaint every cell each frame, and without this the terminal shows those
    // intermediate states (tearing). Terminals that don't support the mode just
    // ignore the private-mode set/reset, so it's safe everywhere.
    out.extend_from_slice(b"\x1b[?2026h");
    // The panes tile the whole area and we repaint every cell, so a clear is
    // only needed when the geometry shrank (and could leave stale cells).
    queue!(out, cursor::Hide, ResetColor)?;
    if clear {
        queue!(out, terminal::Clear(terminal::ClearType::All))?;
    }

    for (id, rect) in &rects {
        if let Some(p) = w.panes.iter().find(|p| p.id == *id) {
            // Show a tab counter in the border when a slot has more than one
            // child (e.g. "shell 2/3"), so stacked terminals/Claude are visible.
            let (pos, n) = w.slot_pos(p.role);
            let mut title = if n > 1 {
                format!("{} {}/{}", p.title, pos + 1, n)
            } else {
                p.title.clone()
            };
            if copy.is_some_and(|cm| cm.pane_id == *id) {
                title.push_str("  COPY: j/k scroll · v select · y copy · Esc exit");
            }
            draw_border(out, rect, &title, *id == w.focus_id)?;
            if let Some(inner) = rect.inner() {
                blit_pane(out, p.screen(), inner)?;
                // Copy/scroll-mode selection + cursor on this pane.
                if let Some(cm) = copy.filter(|cm| cm.pane_id == *id) {
                    draw_copy_overlay(out, p.screen(), inner, cm)?;
                }
                // Live mouse drag-selection highlight on this pane (#21).
                if let Some((mid, s, e)) = msel {
                    if mid == *id {
                        draw_selection(out, p.screen(), inner, s, e)?;
                    }
                }
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

    if let Some(h) = help {
        draw_help(out, cols, rows, keys, h)?;
    }
    if let Some(pk) = picker {
        pk.render(out, cols, rows)?;
    }
    // End the synchronized frame: present everything queued above at once.
    out.extend_from_slice(b"\x1b[?2026l");
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

/// State of the `Ctrl-a ?` overlay. It starts as a read-only key reference and
/// can flip into an inline editor that rebinds keys live (Story #10).
struct HelpState {
    /// In the editor (rows selectable) vs. just viewing the reference.
    editing: bool,
    /// Selected row, an index into `config::EDITABLE`.
    sel: usize,
    /// Waiting to capture the next printable keypress as the new binding.
    capturing: bool,
    /// Transient status / error line shown at the bottom.
    msg: String,
}

impl HelpState {
    fn new() -> HelpState {
        HelpState {
            editing: false,
            sel: 0,
            capturing: false,
            msg: String::new(),
        }
    }
}

/// A decoded overlay keypress (raw bytes -> intent).
enum HelpKey {
    Up,
    Down,
    Enter,
    Esc,
    Char(char),
}

/// Decode raw input bytes into overlay keypresses, resolving arrow escape
/// sequences (`ESC [ A/B`) and a bare `ESC`.
fn decode_help_keys(bytes: &[u8]) -> Vec<HelpKey> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            if i + 2 < bytes.len() && bytes[i + 1] == b'[' {
                match bytes[i + 2] {
                    b'A' => v.push(HelpKey::Up),
                    b'B' => v.push(HelpKey::Down),
                    _ => {}
                }
                i += 3;
                continue;
            }
            v.push(HelpKey::Esc);
            i += 1;
            continue;
        }
        match b {
            b'\r' | b'\n' => v.push(HelpKey::Enter),
            0x20..=0x7e => v.push(HelpKey::Char(b as char)),
            _ => {}
        }
        i += 1;
    }
    v
}

/// Drive the help overlay from user input. Mutates `h`, applies committed
/// rebinds to the shared `keys` (effective immediately) and persists them.
/// Returns true when the overlay should close.
fn help_input(h: &mut HelpState, bytes: &[u8], keys: &Arc<Mutex<Keys>>) -> bool {
    for ev in decode_help_keys(bytes) {
        if h.capturing {
            match ev {
                HelpKey::Esc => {
                    h.capturing = false;
                    h.msg = "cancelled".into();
                }
                HelpKey::Char(c) => {
                    let field = config::EDITABLE[h.sel].0;
                    let cur = *keys.lock().unwrap();
                    match cur.with_bind(field, c) {
                        Ok(nk) => {
                            *keys.lock().unwrap() = nk; // live: reader threads see it
                            config::persist_keybind(field, c); // survive restart
                            h.capturing = false;
                            h.msg = format!("bound {field} to '{c}'");
                        }
                        Err(e) => h.msg = e, // stay capturing so they can retry
                    }
                }
                _ => h.msg = "press a printable key (Esc to cancel)".into(),
            }
            continue;
        }
        if !h.editing {
            match ev {
                HelpKey::Esc => return true,
                HelpKey::Char('e') | HelpKey::Char('E') => {
                    h.editing = true;
                    h.msg.clear();
                }
                HelpKey::Char(c) => {
                    // 'q' or the (current) help key also closes.
                    if c == 'q' || c == keys.lock().unwrap().help {
                        return true;
                    }
                }
                _ => {}
            }
        } else {
            let n = config::EDITABLE.len();
            match ev {
                HelpKey::Esc => {
                    h.editing = false;
                    h.msg.clear();
                }
                HelpKey::Up | HelpKey::Char('k') => h.sel = (h.sel + n - 1) % n,
                HelpKey::Down | HelpKey::Char('j') => h.sel = (h.sel + 1) % n,
                HelpKey::Enter => {
                    h.capturing = true;
                    h.msg = format!("press new key for {}", config::EDITABLE[h.sel].1);
                }
                HelpKey::Char('q') => return true,
                _ => {}
            }
        }
    }
    false
}

/// Keyboard copy/scroll mode for one pane (#21/#22): scroll the pane's
/// scrollback and select text to copy over OSC 52. Coordinates are visible
/// cells (0-based) in the pane's inner area; scrolling shifts the vt100 view.
struct CopyMode {
    pane_id: usize,
    row: u16,
    col: u16,
    /// Selection start, if a selection is active.
    anchor: Option<(u16, u16)>,
}

/// A live mouse drag-selection (#21). CodeForge captures the mouse globally, so
/// the outer terminal can't do its own selection — we render the highlight and
/// copy the text (OSC 52) ourselves on button release. Works in any pane,
/// including alt-screen ones like Claude where the terminal's own copy is dead
/// over SSH. Coordinates are (row, col) within the pane's inner content rect.
struct MouseSel {
    pane_id: usize,
    anchor: (u16, u16),
    cur: (u16, u16),
    /// True once the pointer moved off the anchor cell — distinguishes a
    /// select-drag from a plain click (which we still forward to the app).
    moved: bool,
}

/// A resolved selection to highlight: (pane id, start cell, end cell), in
/// reading order. Passed to `render` for the mouse drag-selection overlay.
type Sel = (usize, (u16, u16), (u16, u16));

impl MouseSel {
    /// Normalized (start, end) in reading order.
    fn range(&self) -> ((u16, u16), (u16, u16)) {
        if self.anchor <= self.cur {
            (self.anchor, self.cur)
        } else {
            (self.cur, self.anchor)
        }
    }
}

impl CopyMode {
    fn new(pane_id: usize) -> CopyMode {
        CopyMode {
            pane_id,
            row: 0,
            col: 0,
            anchor: None,
        }
    }

    /// Normalized (start, end) of the selection in reading order.
    fn selection(&self) -> Option<((u16, u16), (u16, u16))> {
        let a = self.anchor?;
        let b = (self.row, self.col);
        Some(if a <= b { (a, b) } else { (b, a) })
    }
}

enum CopyKey {
    Up,
    Down,
    Left,
    Right,
    PageUp,
    PageDown,
    Top,
    Bottom,
    Select,
    Yank,
    Esc,
}

/// Decode input bytes into copy-mode keys (hjkl + arrows, C-u/C-d + PageUp/Dn,
/// g/G, v/space to select, y/Enter to yank, Esc/q to exit).
fn decode_copy_keys(bytes: &[u8]) -> Vec<CopyKey> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];
        if b == 0x1b {
            if i + 2 < bytes.len() && bytes[i + 1] == b'[' {
                match bytes[i + 2] {
                    b'A' => v.push(CopyKey::Up),
                    b'B' => v.push(CopyKey::Down),
                    b'C' => v.push(CopyKey::Right),
                    b'D' => v.push(CopyKey::Left),
                    b'5' => v.push(CopyKey::PageUp),
                    b'6' => v.push(CopyKey::PageDown),
                    _ => {}
                }
                // Swallow a trailing '~' (ESC [ 5 ~ / 6 ~).
                if i + 3 < bytes.len() && bytes[i + 3] == b'~' {
                    i += 1;
                }
                i += 3;
                continue;
            }
            v.push(CopyKey::Esc);
            i += 1;
            continue;
        }
        match b {
            b'\r' | b'\n' | b'y' => v.push(CopyKey::Yank),
            0x15 => v.push(CopyKey::PageUp),
            0x04 => v.push(CopyKey::PageDown),
            b'k' => v.push(CopyKey::Up),
            b'j' => v.push(CopyKey::Down),
            b'h' => v.push(CopyKey::Left),
            b'l' => v.push(CopyKey::Right),
            b'g' => v.push(CopyKey::Top),
            b'G' => v.push(CopyKey::Bottom),
            b'v' | b' ' => v.push(CopyKey::Select),
            b'q' => v.push(CopyKey::Esc),
            _ => {}
        }
        i += 1;
    }
    v
}

/// Leave copy mode: unfreeze the pane (apply buffered output) and clear state.
fn exit_copy(copy: &mut Option<CopyMode>, windows: &mut [Window]) {
    if let Some(cm) = copy.take() {
        for w in windows.iter_mut() {
            if let Some(p) = w.panes.iter_mut().find(|p| p.id == cm.pane_id) {
                p.unfreeze();
            }
        }
    }
}

/// Drive copy mode on pane `p`. Returns `(should_exit, text_to_copy)`.
fn copy_input(cm: &mut CopyMode, p: &mut Pane, bytes: &[u8]) -> (bool, Option<String>) {
    let (rows, cols) = p.screen().size();
    let maxr = rows.saturating_sub(1);
    let maxc = cols.saturating_sub(1);
    // Alt-screen apps (Claude/vim) have no line scrollback — only move the
    // cursor over the visible screen, don't try to scroll history.
    let alt = p.alternate_screen();
    for ev in decode_copy_keys(bytes) {
        match ev {
            CopyKey::Esc => {
                p.scroll_to_bottom();
                return (true, None);
            }
            CopyKey::Up => {
                if cm.row > 0 {
                    cm.row -= 1;
                } else if !alt {
                    p.scroll(1);
                }
            }
            CopyKey::Down => {
                if cm.row < maxr {
                    cm.row += 1;
                } else if !alt {
                    p.scroll(-1);
                }
            }
            CopyKey::Left => cm.col = cm.col.saturating_sub(1),
            CopyKey::Right => cm.col = (cm.col + 1).min(maxc),
            CopyKey::PageUp => {
                if alt {
                    cm.row = 0;
                } else {
                    p.scroll((rows / 2).max(1) as i32);
                }
            }
            CopyKey::PageDown => {
                if alt {
                    cm.row = maxr;
                } else {
                    p.scroll(-((rows / 2).max(1) as i32));
                }
            }
            CopyKey::Top => {
                if !alt {
                    p.scroll(1_000_000);
                }
                cm.row = 0;
            }
            CopyKey::Bottom => {
                if !alt {
                    p.scroll_to_bottom();
                }
                cm.row = maxr;
            }
            CopyKey::Select => {
                cm.anchor = if cm.anchor.is_some() {
                    None
                } else {
                    Some((cm.row, cm.col))
                };
            }
            CopyKey::Yank => {
                let text = extract_selection(p.screen(), cm);
                p.scroll_to_bottom();
                return (true, Some(text));
            }
        }
    }
    (false, None)
}

/// Extract the selected text (or the cursor's whole line if nothing selected)
/// from the currently-visible screen, trimming trailing spaces per line.
fn extract_selection(screen: &vt100::Screen, cm: &CopyMode) -> String {
    let (_, cols) = screen.size();
    let last = cols.saturating_sub(1);
    let (start, end) = cm.selection().unwrap_or(((cm.row, 0), (cm.row, last)));
    extract_range(screen, start, end)
}

/// Extract text between two (row, col) cell coordinates (inclusive, reading
/// order) from the visible screen, trimming trailing spaces per line. Shared by
/// keyboard copy mode and mouse drag-selection.
fn extract_range(screen: &vt100::Screen, start: (u16, u16), end: (u16, u16)) -> String {
    let (_, cols) = screen.size();
    let last = cols.saturating_sub(1);
    let ((sr, sc), (er, ec)) = (start, end);
    let mut out = String::new();
    for r in sr..=er {
        let cstart = if r == sr { sc } else { 0 };
        let cend = (if r == er { ec } else { last }).min(last);
        let mut line = String::new();
        for c in cstart..=cend {
            match screen.cell(r, c).map(|cell| cell.contents()) {
                Some(s) if !s.is_empty() => line.push_str(&s),
                _ => line.push(' '),
            }
        }
        while line.ends_with(' ') {
            line.pop();
        }
        out.push_str(&line);
        if r < er {
            out.push('\n');
        }
    }
    out
}

/// Reverse-highlight a plain (start, end) cell range on an already-blitted pane
/// — the live feedback for a mouse drag-selection (#21). `inner` is the pane's
/// content rect; `s`/`e` are (row, col) in reading order.
fn draw_selection(
    out: &mut Vec<u8>,
    screen: &vt100::Screen,
    inner: Rect,
    s: (u16, u16),
    e: (u16, u16),
) -> Result<()> {
    let (rows, cols) = screen.size();
    let (sr, sc) = s;
    let (er, ec) = e;
    for r in 0..rows.min(inner.h) {
        if r < sr || r > er {
            continue;
        }
        let cstart = if r == sr { sc } else { 0 };
        let cend = if r == er { ec } else { cols.saturating_sub(1) };
        for c in cstart..=cend.min(cols.min(inner.w).saturating_sub(1)) {
            let ch = screen
                .cell(r, c)
                .map(|cell| cell.contents())
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| " ".to_string());
            queue!(
                out,
                cursor::MoveTo(inner.x + c, inner.y + r),
                SetAttribute(Attribute::Reverse),
                Print(ch),
                SetAttribute(Attribute::Reset)
            )?;
        }
    }
    Ok(())
}

/// Minimal standard base64 (no deps), for OSC 52 clipboard payloads.
fn base64(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for ch in data.chunks(3) {
        let n = ((ch[0] as u32) << 16)
            | ((*ch.get(1).unwrap_or(&0) as u32) << 8)
            | (*ch.get(2).unwrap_or(&0) as u32);
        out.push(T[((n >> 18) & 63) as usize] as char);
        out.push(T[((n >> 12) & 63) as usize] as char);
        out.push(if ch.len() > 1 {
            T[((n >> 6) & 63) as usize] as char
        } else {
            '='
        });
        out.push(if ch.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// An OSC 52 sequence that sets the system clipboard (works over SSH on
/// terminals that support it).
fn osc52(text: &str) -> Vec<u8> {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes())).into_bytes()
}

/// Overlay the copy-mode cursor and selection highlight on an already-blitted
/// pane. `inner` is the pane's content rect.
fn draw_copy_overlay(
    out: &mut Vec<u8>,
    screen: &vt100::Screen,
    inner: Rect,
    cm: &CopyMode,
) -> Result<()> {
    let sel = cm.selection();
    let (rows, cols) = screen.size();
    for r in 0..rows.min(inner.h) {
        // Highlight the whole current line so scrolling/motion is visible even
        // before the cursor reaches an edge.
        let on_cur_row = r == cm.row;
        for c in 0..cols.min(inner.w) {
            let selected = sel.is_some_and(|((sr, sc), (er, ec))| {
                (r > sr || (r == sr && c >= sc)) && (r < er || (r == er && c <= ec))
            });
            if !selected && !on_cur_row {
                continue;
            }
            let ch = screen
                .cell(r, c)
                .map(|cell| cell.contents())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| " ".to_string());
            queue!(
                out,
                cursor::MoveTo(inner.x + c, inner.y + r),
                SetAttribute(Attribute::Reverse),
                Print(ch),
                SetAttribute(Attribute::Reset)
            )?;
        }
    }
    Ok(())
}

/// Draw the centered `Ctrl-a ?` overlay — a key reference, or the inline key
/// editor when `h.editing`. Keys come from the live (possibly just-rebound)
/// config so the display always reflects the current mapping.
fn draw_help(out: &mut Vec<u8>, cols: u16, rows: u16, k: Keys, h: &HelpState) -> Result<()> {
    // (text, is-the-selected-editor-row).
    let mut lines: Vec<(String, bool)> = Vec::new();
    let title = if h.editing {
        "  CodeForge — edit keys (prefix Ctrl-a)"
    } else {
        "  CodeForge — keys (prefix Ctrl-a)"
    };
    lines.push((title.to_string(), false));
    lines.push((String::new(), false));
    for (i, (field, label)) in config::EDITABLE.iter().enumerate() {
        let key = k.get(field).map(|c| c.to_string()).unwrap_or_default();
        let marker = if h.editing && i == h.sel { "›" } else { " " };
        let selected = h.editing && i == h.sel;
        lines.push((format!(" {marker} Ctrl-a {key:<3}  {label}"), selected));
    }
    lines.push((String::new(), false));
    lines.push(("   Ctrl-a 1..9  jump to window".to_string(), false));
    lines.push(("   click / wheel focus / scroll".to_string(), false));
    lines.push((String::new(), false));
    let footer = if h.capturing {
        format!("  {}", h.msg)
    } else if h.editing {
        "  ↑/↓ select · Enter rebind · Esc back".to_string()
    } else {
        "  e edit keys · Esc/q close".to_string()
    };
    lines.push((footer, false));
    if !h.msg.is_empty() && !h.capturing {
        lines.push((format!("  {}", h.msg), false));
    }

    // Never vanish on a short/narrow terminal: clamp instead of bailing. The
    // editable key rows come first, so if anything is dropped it's the trailing
    // info/footer lines, not a binding you're trying to see or rebind.
    let max_lines = rows.saturating_sub(2) as usize;
    if lines.len() > max_lines && max_lines > 0 {
        lines.truncate(max_lines);
    }
    let max_w = cols.saturating_sub(2) as usize;
    for (l, _) in lines.iter_mut() {
        if l.chars().count() > max_w {
            *l = l.chars().take(max_w).collect();
        }
    }
    let w = (lines
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(0) as u16
        + 2)
    .min(cols);
    let bh = (lines.len() as u16 + 2).min(rows);
    if w < 2 || bh < 2 {
        return Ok(());
    }
    let x = (cols - w) / 2;
    let y = (rows - bh) / 2;

    queue!(
        out,
        cursor::Hide,
        SetForegroundColor(Color::Black),
        SetBackgroundColor(Color::Cyan)
    )?;
    queue!(
        out,
        cursor::MoveTo(x, y),
        Print(format!("┌{}┐", "─".repeat((w - 2) as usize)))
    )?;
    for (i, (line, selected)) in lines.iter().enumerate() {
        let padded = format!("{line:<width$}", width = (w - 2) as usize);
        queue!(out, cursor::MoveTo(x, y + 1 + i as u16), Print("│"))?;
        if *selected {
            // Invert (black-on-cyan -> cyan-on-black) to mark the selected row.
            queue!(
                out,
                SetAttribute(Attribute::Reverse),
                Print(&padded),
                SetAttribute(Attribute::Reset),
                SetForegroundColor(Color::Black),
                SetBackgroundColor(Color::Cyan)
            )?;
        } else {
            queue!(out, Print(&padded))?;
        }
        queue!(out, Print("│"))?;
    }
    queue!(
        out,
        cursor::MoveTo(x, y + bh - 1),
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

    fn parser() -> InputParser {
        InputParser::new(0x01, Arc::new(Mutex::new(Keys::default())))
    }

    #[test]
    fn lone_esc_passes_through() {
        // A bare ESC (last byte of the chunk) must reach the client as input,
        // so the help overlay can close on Esc.
        let mut out = Vec::new();
        parser().feed(&[0x1b], &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == &[0x1b]));
    }

    #[test]
    fn esc_sequence_not_split() {
        // An arrow key (ESC [ A) arrives atomically and passes through intact,
        // not mistaken for a lone Esc.
        let mut out = Vec::new();
        parser().feed(b"\x1b[A", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == b"\x1b[A"));
    }

    #[test]
    fn prefix_command_dispatch() {
        // Ctrl-a then default toggle_ai ('c') -> toggle the AI pane.
        let mut out = Vec::new();
        parser().feed(&[0x01, b'c'], &mut out);
        assert!(matches!(out.as_slice(), [Msg::Toggle(PaneRole::Ai)]));
    }

    #[test]
    fn decodes_overlay_keys() {
        assert!(matches!(
            decode_help_keys(&[0x1b]).as_slice(),
            [HelpKey::Esc]
        ));
        assert!(matches!(
            decode_help_keys(b"\x1b[A").as_slice(),
            [HelpKey::Up]
        ));
        assert!(matches!(
            decode_help_keys(b"\x1b[B").as_slice(),
            [HelpKey::Down]
        ));
        assert!(matches!(
            decode_help_keys(b"\r").as_slice(),
            [HelpKey::Enter]
        ));
        assert!(matches!(
            decode_help_keys(b"e").as_slice(),
            [HelpKey::Char('e')]
        ));
    }

    #[test]
    fn rebind_validation() {
        let k = Keys::default();
        // A free key is accepted and applied.
        assert_eq!(k.with_bind("toggle_ai", 'x').unwrap().toggle_ai, 'x');
        // Colliding with another action is rejected.
        assert!(k.with_bind("toggle_ai", k.quit).is_err());
        // Digits and TOML-unsafe keys are rejected.
        assert!(k.with_bind("toggle_ai", '1').is_err());
        assert!(k.with_bind("toggle_ai", '"').is_err());
    }

    #[test]
    fn cpu_percent_math() {
        // 50 of 100 non-idle jiffies -> 50% busy.
        assert_eq!(cpu_percent((100, 200), (150, 300)), 50);
        // No elapsed jiffies -> 0 (no divide by zero).
        assert_eq!(cpu_percent((100, 200), (100, 200)), 0);
    }

    #[test]
    fn metrics_readable_on_linux() {
        assert!(read_cpu().is_some());
        assert!(mem_percent().is_some_and(|m| m <= 100));
        assert!(disk_percent(std::path::Path::new("/")).is_some_and(|d| d <= 100));
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn decodes_copy_keys() {
        assert!(matches!(decode_copy_keys(b"j").as_slice(), [CopyKey::Down]));
        assert!(matches!(decode_copy_keys(b"k").as_slice(), [CopyKey::Up]));
        assert!(matches!(
            decode_copy_keys(b"\x1b[B").as_slice(),
            [CopyKey::Down]
        ));
        assert!(matches!(
            decode_copy_keys(b"\x1b[5~").as_slice(),
            [CopyKey::PageUp]
        ));
        assert!(matches!(
            decode_copy_keys(&[0x15]).as_slice(),
            [CopyKey::PageUp]
        ));
        assert!(matches!(
            decode_copy_keys(b"v").as_slice(),
            [CopyKey::Select]
        ));
        assert!(matches!(decode_copy_keys(b"y").as_slice(), [CopyKey::Yank]));
        assert!(matches!(
            decode_copy_keys(&[0x1b]).as_slice(),
            [CopyKey::Esc]
        ));
    }

    #[test]
    fn extract_selection_reads_cells() {
        let mut parser = vt100::Parser::new(4, 20, 100);
        parser.process(b"hello world\r\nsecond line");
        let screen = parser.screen();
        // Select "hello" on row 0, cols 0..=4.
        let cm = CopyMode {
            pane_id: 0,
            row: 0,
            col: 4,
            anchor: Some((0, 0)),
        };
        assert_eq!(extract_selection(screen, &cm), "hello");
        // No selection -> whole cursor line (trailing spaces trimmed).
        let line = CopyMode {
            pane_id: 0,
            row: 1,
            col: 0,
            anchor: None,
        };
        assert_eq!(extract_selection(screen, &line), "second line");
    }

    #[test]
    fn copy_selection_normalizes() {
        let mut cm = CopyMode::new(0);
        cm.anchor = Some((2, 5));
        cm.row = 1;
        cm.col = 3;
        // Cursor before anchor -> selection returned in reading order.
        assert_eq!(cm.selection(), Some(((1, 3), (2, 5))));
    }

    #[test]
    fn help_overlay_navigation() {
        let keys = Arc::new(Mutex::new(Keys::default()));
        let mut h = HelpState::new();
        // 'e' enters edit mode; Down moves the selection; Esc leaves edit mode.
        assert!(!help_input(&mut h, b"e", &keys));
        assert!(h.editing);
        assert!(!help_input(&mut h, b"\x1b[B", &keys));
        assert_eq!(h.sel, 1);
        assert!(!help_input(&mut h, &[0x1b], &keys));
        assert!(!h.editing);
        // Esc in view mode closes the overlay.
        assert!(help_input(&mut h, &[0x1b], &keys));
    }
}
