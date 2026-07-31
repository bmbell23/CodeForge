//! CodeForge — terminal-native IDE.
//!
//! Client/server. A detached **server** (`forge --server`) owns all state —
//! windows, panes (each a child in its own PTY + vt100 emulator, `pane.rs`),
//! layout (`layout.rs`) — and outlives any client, so a disconnect/SSH drop
//! never loses the session. A thin **client** (`forge`) owns the real terminal:
//! it forwards keystrokes/resizes over a unix socket (`protocol.rs`) and paints
//! the bytes the servee sends back. Bare `forge` starts a server (picking a
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
//!   Ctrl-a n     next window   ·   Ctrl-a 1..9  jump to window (1 = last used)
//!   Ctrl-a g     git diff: persistent panel of changed files with +/- counts
//!                over the terminal pane (stays up until toggled again, Esc
//!                with the terminal focused, or the terminal is toggled);
//!                arrows/Enter or a click open a full-window side-by-side
//!                diff (HEAD left, editable working file right); Esc there
//!                brings the layout back
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
mod favorites;
mod gitdiff;
mod layout;
mod pane;
mod picker;
mod protocol;
mod worktree;
mod wtform;

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
use gitdiff::{DiffAction, DiffList};
use layout::{Dir, FocusDir, Layout, Rect};
use pane::{Pane, PaneRole};
use picker::{Picker, PickerAction};
use wtform::{WorktreeForm, WtformAction};

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

/// Close the current note in the Notes editor; if it's the last one, open a
/// fresh `newnote` first so the window is never left empty (#70). The count and
/// buffer juggling happen in nvim (`CF_notes_close`); we just pass the path.
fn nvim_notes_close(sock: &Path, newnote: &Path) {
    if !sock.exists() {
        return;
    }
    let arg = newnote.to_string_lossy().replace('\'', "''");
    let _ = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg(format!("v:lua.CF_notes_close('{arg}')"))
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

/// The flag file nvim drops while its side-by-side diff view (#18) is open.
/// Lives next to the RPC socket; nvim derives the same path from v:servername.
fn diff_flag(id: usize) -> PathBuf {
    let mut p = nvim_sock(id).into_os_string();
    p.push(".diff");
    PathBuf::from(p)
}

/// Flag nvim writes while a grug-far search is open in editor `id` (#53), so
/// forge can fullscreen the editor for it (hide the terminal/AI) and restore on
/// close — the same idea as the diff view's zoom.
fn grug_flag(id: usize) -> PathBuf {
    let mut p = nvim_sock(id).into_os_string();
    p.push(".grug");
    PathBuf::from(p)
}

/// The absolute path of the file the editor is currently showing, for
/// favouriting it (#80). `None` when nvim isn't reachable or is on a scratch
/// buffer with no file.
fn nvim_current_file(sock: &Path) -> Option<PathBuf> {
    if !sock.exists() {
        return None;
    }
    let out = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg("expand('%:p')")
        .output()
        .ok()?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    (out.status.success() && !path.is_empty()).then(|| PathBuf::from(path))
}

/// Ask the editor to open CodeForge's side-by-side diff (#18) on `file`.
/// True only when nvim confirms — the caller zooms the layout on success.
fn nvim_diff_open(sock: &Path, file: &Path) -> bool {
    if !sock.exists() {
        return false;
    }
    // Vim single-quoted strings escape ' by doubling it.
    let arg = file.to_string_lossy().replace('\'', "''");
    let out = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg(format!("v:lua.CodeForgeDiffOpen('{arg}')"))
        .output();
    matches!(out, Ok(o) if o.status.success()
        && String::from_utf8_lossy(&o.stdout).trim() == "ok")
}

/// Same side-by-side view, but for `file` between two commits (#74): `base` on
/// the left, `head` on the right, both read-only since neither is the working
/// copy. True only when nvim confirms.
fn nvim_diff_open_rev(sock: &Path, file: &Path, base: &str, head: &str) -> bool {
    if !sock.exists() {
        return false;
    }
    let arg = file.to_string_lossy().replace('\'', "''");
    let (b, h) = (base.replace('\'', "''"), head.replace('\'', "''"));
    let out = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg(format!("v:lua.CodeForgeDiffOpenRev('{arg}','{b}','{h}')"))
        .output();
    matches!(out, Ok(o) if o.status.success()
        && String::from_utf8_lossy(&o.stdout).trim() == "ok")
}

/// Ask the editor to close the side-by-side diff view. Best-effort: the flag
/// file vanishing is what actually drives the layout restore.
fn nvim_diff_close(sock: &Path) {
    if !sock.exists() {
        return;
    }
    let _ = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg("v:lua.CodeForgeDiffClose()")
        .output();
}

/// The About page (#66), embedded so it opens regardless of where the shared
/// clone lives (the exe may be run from a path with no repo alongside it).
const ABOUT_DOC: &str = include_str!("../docs/ABOUT.md");

/// Write the embedded About doc to a cache file and return its path, so nvim can
/// `:edit` it. `$XDG_CACHE_HOME` or `~/.cache`, mirroring `config_path`.
fn about_doc_path() -> Option<PathBuf> {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
        });
    let dir = base.join("codeforge");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("ABOUT.md");
    // Rewrite every time so a new build's doc always wins over a stale cache.
    std::fs::write(&path, ABOUT_DOC).ok()?;
    Some(path)
}

/// Open `file` as a buffer in a running nvim (its RPC socket), so it shows up as
/// an editor tab. Best-effort; `silent!` keeps it quiet if the edit fails.
fn nvim_open_file(sock: &Path, file: &Path) {
    if !sock.exists() {
        return;
    }
    // Vim single-quoted strings escape ' by doubling it.
    let arg = file.to_string_lossy().replace('\'', "''");
    let _ = Command::new("timeout")
        .arg("2")
        .arg("nvim")
        .arg("--server")
        .arg(sock)
        .arg("--remote-expr")
        .arg(format!("execute('silent! edit {arg}')"))
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
    /// Open the About page (#66) in the focused window's editor pane.
    OpenAbout,
    /// Open the window switcher (#78): a popup list of the open projects.
    OpenWindowList,
    /// Open the favorites list for the current repo (#80).
    OpenFavorites,
    /// Favorite / unfavorite the file open in the editor (#80).
    ToggleFavorite,
    /// Toggle fullscreen of the focused pane, hiding the other two (#40).
    ZoomPane,
    /// Go to the Notes window (position 0), creating it — with a fresh
    /// timestamped note — if it doesn't exist yet (#70).
    OpenNotes,
    /// Toggle the git-diff list overlay (#18); with the full-screen diff view
    /// up, asks the editor to close it instead.
    ToggleDiff,
    /// The editor's side-by-side diff view went away (its flag file vanished):
    /// restore the pane layout saved when it opened. Payload = editor pane id.
    DiffClosed(usize),
    /// A grug-far search opened (`true`) or closed (`false`) in editor pane
    /// `id` (#53): fullscreen the editor for it, or restore the layout.
    GrugZoom(usize, bool),
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
    /// A worktree-creation worker finished (#50): the new worktree's path, or
    /// an error message to surface. Runs off-thread so the git fetch never
    /// blocks the event loop.
    WorktreeResult(Result<PathBuf, String>),
    /// A short human status from the running worktree worker (#50), shown live
    /// in the form ("fetching…", "adding worktree…", …).
    WorktreeProgress(String),
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
    /// Saved (editor, shell, ai) visibility while the focused pane is manually
    /// fullscreened (#40); `None` when not zoomed. Toggling restores it.
    zoom_prev: Option<(bool, bool, bool)>,
    /// This is THE Notes window (its dir is `notes_dir()`), so it lives at
    /// position 0 and `tab_new` makes a new timestamped note (#70).
    notes: bool,
    /// Monotonic tick of when this window was last focused, newest highest.
    /// `mod-N` numbers projects by this rather than by creation order, so the
    /// last project you were in is always `mod-1` (#77).
    used: u64,
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

/// Pane-visibility snapshot taken when the full-screen diff view (#18) opened,
/// so Esc in the editor puts the layout back exactly as it was.
struct DiffZoom {
    editor_id: usize,
    /// (show_editor, show_shell, show_ai) before the zoom.
    prev: (bool, bool, bool),
}

/// Where the git-diff list overlay sits: exactly over the terminal pane when
/// it's visible, otherwise a centered box.
fn diff_overlay_rect(w: &Window, cols: u16, rows: u16) -> Rect {
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
    if let Some((_, r)) = rects.iter().find(|(id, _)| *id == w.active[1]) {
        return *r;
    }
    let bw = cols.saturating_sub(4).clamp(24, 70);
    let bh = area.saturating_sub(2).clamp(4, 16);
    Rect {
        x: cols.saturating_sub(bw) / 2,
        y: area.saturating_sub(bh) / 2,
        w: bw,
        h: bh,
    }
}

/// Hand a picked file to the editor as a full-window side-by-side diff (#18):
/// snapshot the pane layout, zoom the editor, and RPC nvim to build the view.
/// A watcher thread waits for nvim's flag file to vanish (Esc / close / exit)
/// and then sends `Msg::DiffClosed` so the saved layout comes back. Returns
/// Why the diff handoff failed, for the panel's footer — "nothing happened" is
/// the worst possible feedback when the editor doesn't take the file (#74).
fn diff_open_failure(windows: &[Window], cur: usize) -> String {
    let ed = windows[cur].active[0];
    let sock = nvim_sock(ed);
    if !sock.exists() {
        format!(
            "no editor RPC socket ({}) — is nvim running?",
            sock.display()
        )
    } else {
        "editor didn't respond — try again".to_string()
    }
}

/// false (layout untouched) when the editor didn't respond.
/// What the full-window diff view should show: a file's working-tree diff
/// (HEAD vs the file on disk), or that file between two commits (#74).
enum DiffTarget<'a> {
    File(&'a Path),
    FileRev(&'a Path, &'a str, &'a str),
}

fn open_diff_view(
    windows: &mut [Window],
    cur: usize,
    target: DiffTarget<'_>,
    diff_zoom: &mut Option<DiffZoom>,
    cfg: &Config,
    size: (u16, u16),
    tx: &Sender<Msg>,
) -> Result<bool> {
    let w = &mut windows[cur];
    let ed = w.active[0];
    let flag = diff_flag(ed);
    // A stale flag (earlier crash) must not make the watcher fire instantly;
    // nvim re-creates it during the open call below.
    let _ = std::fs::remove_file(&flag);
    let opened = match target {
        DiffTarget::File(path) => nvim_diff_open(&nvim_sock(ed), path),
        DiffTarget::FileRev(path, base, head) => {
            nvim_diff_open_rev(&nvim_sock(ed), path, base, head)
        }
    };
    if !opened {
        return Ok(false);
    }
    if diff_zoom.is_none() {
        *diff_zoom = Some(DiffZoom {
            editor_id: ed,
            prev: (w.show_editor, w.show_shell, w.show_ai),
        });
    }
    w.show_editor = true;
    w.show_shell = false;
    w.show_ai = false;
    w.focus_id = ed;
    refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
    relayout(&mut w.panes, &w.layout, size.0, size.1.saturating_sub(1))?;
    let txc = tx.clone();
    thread::spawn(move || {
        // Poll the flag. Stepping to the next/prev file (#51) reopens the diff,
        // which briefly deletes and recreates the flag, so only a *sustained*
        // absence counts as a real close — a short grace re-check absorbs the
        // swap blip and avoids a false DiffClosed.
        loop {
            if !flag.exists() {
                thread::sleep(Duration::from_millis(300));
                if !flag.exists() {
                    break;
                }
            }
            thread::sleep(Duration::from_millis(100));
        }
        let _ = txc.send(Msg::DiffClosed(ed));
    });
    Ok(true)
}

/// The Notes project: `<projects_root>/Notes` — a real, source-controlled repo,
/// pinned to window position 0 (mod-0). New notes are timestamped `.md` files
/// filed under `History/YYYY/MM/DD/` (#70).
fn notes_dir() -> PathBuf {
    projects_root().join("Notes")
}

/// Path for a new timestamped note: `<notes>/History/YYYY/MM/DD/YYYY-MM-DD-HH:MM:SS.md`.
/// The caller must create the parent dir before the editor writes it.
fn notes_note_path(base: &Path, now: chrono::DateTime<chrono::Local>) -> PathBuf {
    base.join("History")
        .join(now.format("%Y").to_string())
        .join(now.format("%m").to_string())
        .join(now.format("%d").to_string())
        .join(format!("{}.md", now.format("%Y-%m-%d-%H:%M:%S")))
}

/// Decide which note(s) the Notes window opens on (re)build. Same-day resume
/// reopens whatever was open; on a new day (no open note dated today) it opens a
/// single fresh note for today instead, so each day starts clean (#70).
fn notes_spec_for_today(mut spec: WindowSpec, now: chrono::DateTime<chrono::Local>) -> WindowSpec {
    let today = now.format("%Y-%m-%d").to_string();
    let has_today = spec.files.iter().any(|f| {
        f.file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with(&today))
    });
    if !has_today {
        spec.files = vec![notes_note_path(&spec.dir, now)];
    }
    spec
}

/// A file under `$XDG_RUNTIME_DIR` (falling back to the temp dir), the shared
/// side-channel between the Notes sync script and CodeForge's status bar (#71).
fn runtime_file(name: &str) -> PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .ok()
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join(name)
}

/// The Notes auto-sync script in the shared clone (`<clone>/scripts/…`), derived
/// from the running binary at `<clone>/target/release/forge` (#71).
fn notes_sync_script() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let clone = exe.parent()?.parent()?.parent()?;
    let s = clone.join("scripts").join("notes-autosync.sh");
    s.exists().then_some(s)
}

/// The Notes sync status line for the status bar: conflict, unsaved (needs
/// Ctrl-s), or synced with the last-sync time. The script maintains the flag /
/// timestamp files; dirtiness is a cheap `git status` (#71).
fn notes_status_line(dir: &Path) -> String {
    if runtime_file("codeforge-notes-conflict").exists() {
        return "Notes: CONFLICT — resolve".into();
    }
    let dirty = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain"])
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(false);
    if dirty {
        return "Notes: unsaved — ^S to save".into();
    }
    let synced = std::fs::read_to_string(runtime_file("codeforge-notes-synced-at"))
        .ok()
        .and_then(|s| s.trim().parse::<i64>().ok())
        .and_then(|e| {
            use chrono::TimeZone;
            chrono::Local.timestamp_opt(e, 0).single()
        });
    match synced {
        Some(d) => format!("Notes: synced {}", d.format("%H:%M")),
        None => "Notes: synced".into(),
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
        zoom_prev: None,
        // The Notes window is the one pinned to the Notes repo; this also
        // re-marks it after a session restore, which keys off the dir (#70).
        notes: spec.dir == notes_dir(),
        // Ordered behind every window already in use, so a newly opened project
        // slots in as the least-recent until you switch to it (#77).
        used: 0,
    })
}

/// Directories that already have a window, so the picker can leave them out and
/// callers can focus the existing window instead of opening a duplicate (#82).
fn open_dirs(windows: &[Window]) -> Vec<PathBuf> {
    windows.iter().map(|w| w.dir.clone()).collect()
}

/// The window already showing `dir`, if there is one. Two windows on one
/// directory means two editors over the same working tree — the second one's
/// writes silently fight the first's.
fn window_for_dir(windows: &[Window], dir: &Path) -> Option<usize> {
    windows.iter().position(|w| w.dir == dir)
}

/// Project windows ordered most-recently-used first, skipping the current one
/// (you're already there) and Notes (pinned to `mod-0`). `mod-N` selects the
/// Nth of these, so `mod-1` is always the project you were in last (#77).
fn mru_order(windows: &[Window], cur: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..windows.len())
        .filter(|&i| i != cur && !windows[i].notes)
        .collect();
    // Newest first; ties (never-visited windows) keep their creation order.
    idx.sort_by_key(|&i| (std::cmp::Reverse(windows[i].used), i));
    idx
}

/// Left-to-right order of the tab strip (#77): Notes stays pinned to the far
/// left as `mod-0`, then the current window (no number), then the other projects
/// by recency. Projects shuffle as you use them, so "the project I was just in"
/// is always the tab right after the current one, and always `mod-1`.
fn tab_order(windows: &[Window], cur: usize) -> Vec<usize> {
    let mut order: Vec<usize> = (0..windows.len())
        .filter(|&i| windows[i].notes && i != cur)
        .collect();
    if cur < windows.len() {
        order.push(cur);
    }
    order.extend(mru_order(windows, cur));
    order
}

/// The `mod-N` number shown on each window's tab: `0` for Notes, its recency
/// position for the rest, and `None` for the current window — pressing a number
/// to get where you already are is wasted (#77).
fn tab_numbers(windows: &[Window], cur: usize) -> Vec<Option<usize>> {
    let order = mru_order(windows, cur);
    (0..windows.len())
        .map(|i| {
            if windows[i].notes {
                Some(0)
            } else if i == cur {
                None
            } else {
                order.iter().position(|&j| j == i).map(|p| p + 1)
            }
        })
        .collect()
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
    // Resume the prior conversation for this project if it has one, else start
    // fresh — so a brand-new project isn't stranded on the resume picker (#54).
    let (ai, ai_title) = command_line(&ai_cmd_for_dir(cfg, dir), dir);

    // Watch for grug-far opening in this editor (#53): nvim writes the .grug
    // flag while the search is up; forge fullscreens the editor for it and
    // restores on close. Polls the flag and sends only on change; runs for the
    // server's life (exits when the channel closes). Editor respawns reuse the
    // same id/flag path, so one watcher per window's editor suffices.
    {
        let txg = tx.clone();
        let flag = grug_flag(base);
        thread::spawn(move || {
            let mut last = false;
            loop {
                let now = flag.exists();
                if now != last {
                    last = now;
                    if txg.send(Msg::GrugZoom(base, now)).is_err() {
                        return;
                    }
                }
                thread::sleep(Duration::from_millis(120));
            }
        });
    }

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
        // Editor keybinds (open file, explorer, tab cycling, …) flow to init.lua
        // as `name=token` lines so the finder maps and the splash cheatsheet read
        // from config.toml's [editor_keys] (#28).
        c.env("CODEFORGE_EDITOR_KEYS", cfg.editor_keys.env_string());
        // The prefix + its bindings, so the splash's "Ctrl-a …" cheatsheet shows
        // the live keys after a rebind rather than the hardcoded defaults (#28).
        c.env("CODEFORGE_PREFIX", &cfg.prefix);
        c.env("CODEFORGE_PREFIX_KEYS", cfg.keys.env_string());
        // So nvim can bind Ctrl-s in Notes buffers to write-all + sync (#71).
        c.env("CODEFORGE_NOTES_DIR", notes_dir());
        if let Some(s) = notes_sync_script() {
            c.env("CODEFORGE_NOTES_SYNC", s);
        }
        let sock = nvim_sock(id);
        let _ = std::fs::remove_file(&sock); // stale socket blocks --listen
        let _ = std::fs::remove_file(diff_flag(id)); // stale diff flag (#18)
        c.arg("--listen");
        c.arg(&sock);
        if !cfg.wrap {
            c.arg("-c");
            c.arg("set nowrap");
        }
        // init.lua autosaves unless this global is 0 (#19). Notes always
        // autosaves regardless of the global setting (#70).
        if !cfg.autosave && *dir != notes_dir() {
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

/// The AI command for a *fresh* session — no `--resume`/`--continue` (#49). The
/// first AI pane resumes the prior conversation (which we like), but a new AI
/// tab should start clean. For claude we drop any resume/continue flags that got
/// appended to `cfg.ai`; other CLIs relaunch as configured.
fn fresh_ai_cmd(cfg: &Config) -> String {
    if cfg.ai.trim_start().starts_with("claude") {
        "claude".to_string()
    } else {
        cfg.ai.clone()
    }
}

/// claude's per-project history key for `dir`: the absolute path with every
/// non-alphanumeric char turned into `-` (e.g. `/home/bbell/projects/CodeForge`
/// -> `-home-bbell-projects-CodeForge`).
fn claude_project_key(dir: &Path) -> String {
    dir.to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Whether claude has a prior conversation stored for `dir` (under
/// `~/.claude/projects/<key>`).
fn claude_has_session(dir: &Path) -> bool {
    let home = std::env::var("HOME").unwrap_or_default();
    Path::new(&home)
        .join(".claude/projects")
        .join(claude_project_key(dir))
        .is_dir()
}

/// The AI command for a window's *first* pane, opened on `dir` (#54). Resume the
/// prior conversation only when the project actually has one — a brand-new
/// project (e.g. a fresh worktree) has nothing to continue, so `claude
/// --continue` would strand it on the resume picker; start it fresh instead.
fn ai_cmd_for_dir(cfg: &Config, dir: &Path) -> String {
    let base = fresh_ai_cmd(cfg);
    if cfg.ai.trim_start().starts_with("claude") && claude_has_session(dir) {
        format!("{base} --continue")
    } else {
        base
    }
}

/// Open a new Claude tab in the current window's AI slot running `cmd_line`,
/// focus it, and re-lay out (#54). Shared by the tab-new chooser's resume/fresh
/// branches.
#[allow(clippy::too_many_arguments)]
fn open_ai_tab(
    windows: &mut [Window],
    cur: usize,
    cmd_line: &str,
    next_id: &mut usize,
    cfg: &Config,
    tx: &Sender<Msg>,
    size: (u16, u16),
) -> Result<()> {
    let id = *next_id;
    *next_id += 1;
    let w = &mut windows[cur];
    let dir = w.dir.clone();
    let (cmd, title) = command_line(cmd_line, &dir);
    let pane = Pane::spawn(cmd, title, PaneRole::Ai, 1, 1, id, tx.clone())?;
    w.panes.push(pane);
    w.active[role_index(PaneRole::Ai)] = id;
    w.focus_id = id;
    refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
    relayout(&mut w.panes, &w.layout, size.0, size.1.saturating_sub(1))?;
    Ok(())
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
    // Re-exec the freshly-built binary by its stable path, not the inode we're
    // running. A `cargo build --release` replaces `target/release/forge` with a
    // new inode; the running process keeps the old one open (on NFS it's
    // silly-renamed to `.nfsXXXX`, on local fs Linux marks it " (deleted)"), so
    // `current_exe()` points at the stale binary. Re-launching `<dir>/forge`
    // instead means `Ctrl-a r` (reload) actually picks up the latest build.
    let exe = exe
        .parent()
        .map(|d| d.join("forge"))
        .filter(|p| p.is_file())
        .unwrap_or(exe);
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

    let (cfg, _) = Config::load();
    let prefix = cfg.prefix_chord();
    // Shared so the `Ctrl-a ?` editor can rebind keys live: reader-thread
    // parsers and the event loop all see the same mapping.
    let keys = Arc::new(Mutex::new(cfg.keys));
    // Editor bindings edited in the `Ctrl-a ?` overlay: the overlay shows the
    // pending value here and persists to config.toml; nvim picks it up on reload
    // (#28). Kept beside `keys` so the overlay has one place to read/write.
    let editor_keys = Arc::new(Mutex::new(cfg.editor_keys.clone()));
    let shell = cfg
        .shell
        .clone()
        .unwrap_or_else(|| std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string()));
    let proot = cfg
        .projects_root
        .clone()
        .map(PathBuf::from)
        .unwrap_or_else(projects_root);

    // With dirs: one fresh window each. Without: restore the saved session.
    // The AI pane resumes per project only when that project has a prior
    // conversation (decided in spawn_ide, #54) — no global --continue mutation
    // that would leak onto brand-new projects.
    let specs: Vec<WindowSpec> = if dirs.is_empty() {
        let saved = load_snapshot();
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

    // Notes sync status for the status bar (#71): a background poll of the flag /
    // timestamp files the sync script maintains, plus a cheap git-dirty check.
    // Only nudges a redraw when the line changes.
    let notes_status = Arc::new(Mutex::new(String::new()));
    if notes_dir().is_dir() {
        let notes_status = notes_status.clone();
        let tx = tx.clone();
        let ndir = notes_dir();
        thread::spawn(move || loop {
            let line = notes_status_line(&ndir);
            {
                let mut g = notes_status.lock().unwrap();
                if *g != line {
                    *g = line;
                    let _ = tx.send(Msg::Tick);
                }
            }
            thread::sleep(Duration::from_secs(4));
        });
    }

    // A saved session (or a command line naming the same project twice) can
    // carry duplicate directories; keep the first of each so a restore can't
    // resurrect two windows over one working tree (#82).
    let mut seen_dirs: Vec<PathBuf> = Vec::new();
    let specs: Vec<WindowSpec> = specs
        .into_iter()
        .filter(|s| {
            let fresh = !seen_dirs.contains(&s.dir);
            if fresh {
                seen_dirs.push(s.dir.clone());
            }
            fresh
        })
        .collect();

    let mut windows: Vec<Window> = Vec::new();
    let mut next_id: usize = 0;
    for spec in &specs {
        // On restore, the Notes window opens today's note (fresh on a new day),
        // not yesterday's (#70).
        let spec = if spec.dir == notes_dir() {
            let s = notes_spec_for_today(spec.clone(), chrono::Local::now());
            if let Some(day) = s.files.first().and_then(|f| f.parent()) {
                let _ = std::fs::create_dir_all(day);
            }
            s
        } else {
            spec.clone()
        };
        windows.push(new_window(&spec, &cfg, &shell, next_id, &tx)?);
        next_id += 3;
    }
    // The Notes window always exists at position 0 (mod-0) (#70). If a restored
    // window is it, move it to the front; otherwise create it now with a fresh
    // timestamped note. It's a normal window otherwise — closable, returns on
    // mod-0.
    if let Some(i) = windows.iter().position(|w| w.notes) {
        if i != 0 {
            let w = windows.remove(i);
            windows.insert(0, w);
        }
    } else {
        let dir = notes_dir();
        let note = notes_note_path(&dir, chrono::Local::now());
        if let Some(day) = note.parent() {
            let _ = std::fs::create_dir_all(day);
        }
        let spec = WindowSpec {
            dir,
            shell_cwd: None,
            files: vec![note],
        };
        let w = new_window(&spec, &cfg, &shell, next_id, &tx)?;
        next_id += 3;
        windows.insert(0, w);
    }
    // Land on the first real project window when there is one, not Notes.
    let mut cur = windows.iter().position(|w| !w.notes).unwrap_or(0);
    // Monotonic clock behind `mod-N`'s recency order (#77); the window we start
    // on is stamped as the newest below.
    let mut use_tick: u64 = 0;
    let mut help: Option<HelpState> = None;
    let mut copy: Option<CopyMode> = None;
    let mut msel: Option<MouseSel> = None;
    let mut picker: Option<Picker> = None;
    let mut picker_new_window = false;
    let mut wtform: Option<WorktreeForm> = None;
    // A "new Claude tab: resume or fresh?" chooser is showing (#54); the next
    // key (r/n/Esc) resolves it.
    let mut ai_tab_prompt = false;
    // The window switcher is up (#78); it owns input until Enter/Esc.
    let mut switcher: Option<WinSwitcher> = None;
    // The favorites list is up (#80); modal in the same way.
    let mut favs: Option<FavList> = None;
    // Transient confirmation for a favorite toggle, shown on the status bar
    // until the next one — the editor pane gives no feedback of its own.
    let mut fav_note: Option<String> = None;
    let mut diff: Option<DiffList> = None;
    let mut diff_zoom: Option<DiffZoom> = None;
    // Editor fullscreen while a grug-far search is open (#53), mirroring the
    // diff view's zoom. Reuses DiffZoom (editor id + saved pane visibility).
    let mut grug_zoom: Option<DiffZoom> = None;
    // (cols, rows) of the attached client; updated on attach/resize.
    let mut size = (80u16, 24u16);
    let mut client: Option<UnixStream> = None;
    // Ships only what changed each frame; see FrameDiffer (#46).
    let mut differ = FrameDiffer::new(size.0, size.1);
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
                    // Forward any OSC 52 clipboard-set a pane emits to the client,
                    // so nvim's / the shell's "copy" reaches the real terminal's
                    // clipboard over SSH (the vt100 mirror would swallow it) (#32).
                    if let Some(cl) = client.as_mut() {
                        for r in osc52_ranges(&bytes) {
                            let _ = protocol::write_frame(cl, protocol::OUTPUT, &bytes[r]);
                        }
                    }
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
                    if let Some(fl) = favs.as_mut() {
                        // Modal: arrows move, typing filters, Enter opens (#80).
                        let mut done = false;
                        let mut pick = None;
                        if bytes.starts_with(b"\x1b[A") || bytes.starts_with(b"\x1bOA") {
                            fl.step(-1);
                        } else if bytes.starts_with(b"\x1b[B") || bytes.starts_with(b"\x1bOB") {
                            fl.step(1);
                        } else {
                            for &b in bytes.iter() {
                                if let Some(p) = fl.feed(b, &mut done) {
                                    pick = Some(p);
                                }
                                if done {
                                    break;
                                }
                            }
                        }
                        if done {
                            favs = None;
                        }
                        if let Some(path) = pick {
                            let w = &mut windows[cur];
                            let ed = w.active[role_index(PaneRole::Editor)];
                            nvim_open_file(&nvim_sock(ed), &path);
                            if !w.show_editor {
                                w.show_editor = true;
                                refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                                let (c, r) = size;
                                relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                            }
                            w.focus_id = ed;
                        }
                        dirty = true;
                        needs_clear = true;
                    } else if let Some(sw) = switcher.as_mut() {
                        // Modal: arrows move, everything else filters/picks (#78).
                        let mut done = false;
                        let mut pick = None;
                        if bytes.starts_with(b"\x1b[A") || bytes.starts_with(b"\x1bOA") {
                            sw.step(&windows, cur, -1);
                        } else if bytes.starts_with(b"\x1b[B") || bytes.starts_with(b"\x1bOB") {
                            sw.step(&windows, cur, 1);
                        } else {
                            for &b in bytes.iter() {
                                if let Some(i) = sw.feed(&windows, cur, b, &mut done) {
                                    pick = Some(i);
                                }
                                if done {
                                    break;
                                }
                            }
                        }
                        if done {
                            switcher = None;
                        }
                        if let Some(i) = pick {
                            if i != cur {
                                exit_copy(&mut copy, &mut windows);
                                use_tick += 1;
                                windows[cur].used = use_tick;
                                cur = i;
                            }
                        }
                        dirty = true;
                        needs_clear = true;
                    } else if ai_tab_prompt {
                        // Modal chooser for a new Claude tab (#54): r resume,
                        // n new, Esc/Ctrl-c cancel; other keys ignored.
                        match bytes.first().copied().unwrap_or(0) {
                            b'r' | b'R' => {
                                ai_tab_prompt = false;
                                let base = fresh_ai_cmd(&cfg);
                                open_ai_tab(
                                    &mut windows,
                                    cur,
                                    &format!("{base} --resume"),
                                    &mut next_id,
                                    &cfg,
                                    &tx,
                                    size,
                                )?;
                            }
                            b'n' | b'N' => {
                                ai_tab_prompt = false;
                                open_ai_tab(
                                    &mut windows,
                                    cur,
                                    &fresh_ai_cmd(&cfg),
                                    &mut next_id,
                                    &cfg,
                                    &tx,
                                    size,
                                )?;
                            }
                            0x1b | 0x03 => ai_tab_prompt = false,
                            _ => {}
                        }
                        dirty = true;
                        needs_clear = true;
                    } else if let Some(cm) = copy.as_mut() {
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
                        if help_input(h, &bytes, &keys, &editor_keys) {
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
                                // Belt and braces: the picker already hides open
                                // projects, but a typed/stale path could still
                                // name one. Focus that window rather than open a
                                // second editor over the same tree (#82).
                                if let Some(i) = window_for_dir(&windows, &dir) {
                                    if i != cur {
                                        use_tick += 1;
                                        windows[cur].used = use_tick;
                                        cur = i;
                                    }
                                    dirty = true;
                                    continue;
                                }
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
                            PickerAction::NewWorktree => {
                                picker = None;
                                wtform = Some(WorktreeForm::new(proot.clone()));
                                needs_clear = true;
                            }
                        }
                        dirty = true;
                    } else if let Some(wf) = wtform.as_mut() {
                        match wf.feed_bytes(&bytes) {
                            WtformAction::None => {}
                            WtformAction::Cancel => {
                                wtform = None;
                                needs_clear = true;
                            }
                            WtformAction::Submit(spec) => {
                                // Create off-thread — the git fetch must not
                                // block the event loop. The form stays up
                                // ("creating…") until the result Msg lands.
                                wf.begin_create();
                                let txw = tx.clone();
                                let root = proot.clone();
                                std::thread::spawn(move || {
                                    let txp = txw.clone();
                                    let res = worktree::create(&root, &spec, &|s: &str| {
                                        let _ = txp.send(Msg::WorktreeProgress(s.to_string()));
                                    })
                                    .map_err(|e| format!("{e:#}"));
                                    let _ = txw.send(Msg::WorktreeResult(res));
                                });
                            }
                        }
                        dirty = true;
                    } else if diff.is_some()
                        && windows[cur].focus_id == windows[cur].active[1]
                        && !diff_zoom
                            .as_ref()
                            .is_some_and(|z| windows[cur].panes.iter().any(|p| p.id == z.editor_id))
                    {
                        // The git-diff panel (#18) covers the terminal pane;
                        // while that pane is focused the panel owns the keys
                        // (arrows/Enter/Esc). Focus any other pane and input
                        // flows there untouched — the panel just stays up.
                        let action = diff
                            .as_mut()
                            .map(|dl| dl.feed_bytes(&bytes))
                            .unwrap_or(DiffAction::None);
                        match action {
                            DiffAction::None => {}
                            DiffAction::Cancel => {
                                diff = None;
                                needs_clear = true;
                            }
                            DiffAction::Open { path, rev } => {
                                let target = match &rev {
                                    Some((b, h)) => DiffTarget::FileRev(&path, b, h),
                                    None => DiffTarget::File(&path),
                                };
                                if !open_diff_view(
                                    &mut windows,
                                    cur,
                                    target,
                                    &mut diff_zoom,
                                    &cfg,
                                    size,
                                    &tx,
                                )? {
                                    let why = diff_open_failure(&windows, cur);
                                    if let Some(dl) = diff.as_mut() {
                                        dl.set_note(&why);
                                    }
                                }
                                needs_clear = true;
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
                        // Toggling the terminal (either way) dismisses the
                        // diff panel that sits on top of it (#18).
                        if matches!(role, PaneRole::Shell) {
                            diff = None;
                        }
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
                    // The git-diff panel (#18) persists over the terminal
                    // pane; it only owns mouse events that land inside its
                    // box (click opens a row, wheel moves the selection).
                    // Everything else falls through to the panes. While the
                    // full-screen diff view has this window the panel isn't
                    // drawn, so it takes nothing.
                    let zoomed_here = diff_zoom
                        .as_ref()
                        .is_some_and(|z| windows[cur].panes.iter().any(|p| p.id == z.editor_id));
                    let mut taken = false;
                    if !zoomed_here {
                        if let Some(dl) = diff.as_mut() {
                            let rect = diff_overlay_rect(&windows[cur], size.0, size.1);
                            let (px, py) = (x.saturating_sub(1), y.saturating_sub(1));
                            taken = px >= rect.x
                                && px < rect.x + rect.w
                                && py >= rect.y
                                && py < rect.y + rect.h;
                            if taken {
                                let is_wheel = cb & 64 != 0;
                                if is_wheel {
                                    if press {
                                        dl.scroll(if cb & 1 == 0 { -3 } else { 3 });
                                        dirty = true;
                                    }
                                } else if press && cb & 3 == 0 && cb & 32 == 0 {
                                    if let Some(i) = dl.row_at(&rect, px, py) {
                                        // In the commit list a click marks the
                                        // row; elsewhere it opens the file (#74).
                                        if let DiffAction::Open { path, rev } = dl.click(i) {
                                            let target = match &rev {
                                                Some((b, h)) => DiffTarget::FileRev(&path, b, h),
                                                None => DiffTarget::File(&path),
                                            };
                                            if !open_diff_view(
                                                &mut windows,
                                                cur,
                                                target,
                                                &mut diff_zoom,
                                                &cfg,
                                                size,
                                                &tx,
                                            )? {
                                                if let Some(dl) = diff.as_mut() {
                                                    dl.set_note(&diff_open_failure(&windows, cur));
                                                }
                                            }
                                            needs_clear = true;
                                        }
                                        dirty = true;
                                    }
                                }
                            }
                        }
                    }
                    if !taken {
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
                            let mouse_mode = w
                                .panes
                                .iter()
                                .find(|p| p.id == id)
                                .map(|p| p.mouse_mode())
                                .unwrap_or(vt100::MouseProtocolMode::None);
                            let wants_mouse = mouse_mode != vt100::MouseProtocolMode::None;
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
                            } else if is_motion {
                                // Motion (move / non-left drag): only forward when
                                // the app actually tracks motion, so a pane left in
                                // click-only or stale mouse mode never gets raw
                                // sequences typed into it (#73). ButtonMotion wants
                                // drags (a button held, code != 3); AnyMotion wants
                                // every move.
                                let track = match mouse_mode {
                                    vt100::MouseProtocolMode::AnyMotion => true,
                                    vt100::MouseProtocolMode::ButtonMotion => button != 3,
                                    _ => false,
                                };
                                if track {
                                    forward(w, cb, press)?;
                                }
                            } else if wants_mouse {
                                // Other buttons (right/middle) go to the app as-is.
                                forward(w, cb, press)?;
                            }
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
                    diff = None;
                    help = if help.is_some() {
                        None
                    } else {
                        Some(HelpState::new())
                    };
                    dirty = true;
                    needs_clear = true;
                }
                Msg::OpenWindowList => {
                    // Open the switcher fresh each time — a stale filter from
                    // the last visit would hide most of the list (#78).
                    switcher = Some(WinSwitcher {
                        filter: String::new(),
                        sel: 0,
                    });
                    dirty = true;
                    needs_clear = true;
                }
                Msg::OpenFavorites => {
                    favs = Some(FavList::new(&windows[cur].dir));
                    dirty = true;
                    needs_clear = true;
                }
                Msg::ToggleFavorite => {
                    // Favorite whatever the editor is showing; the confirmation
                    // rides the status bar's right side (#80).
                    let w = &windows[cur];
                    let ed = w.active[role_index(PaneRole::Editor)];
                    fav_note = match nvim_current_file(&nvim_sock(ed)) {
                        Some(file) => match favorites::toggle(&w.dir, &file) {
                            Some(true) => Some("favorited".to_string()),
                            Some(false) => Some("unfavorited".to_string()),
                            None => Some("not inside a git repo".to_string()),
                        },
                        None => Some("no file in the editor".to_string()),
                    };
                    dirty = true;
                }
                Msg::OpenAbout => {
                    // Show the bundled docs (#66) as an editor buffer: reveal +
                    // focus the editor, then RPC-open the cached ABOUT.md.
                    exit_copy(&mut copy, &mut windows);
                    help = None;
                    picker = None;
                    let w = &mut windows[cur];
                    if let Some(ed) = w
                        .panes
                        .iter()
                        .find(|p| p.role == PaneRole::Editor)
                        .map(|p| p.id)
                    {
                        if !w.show_editor {
                            w.show_editor = true;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            let (c, r) = size;
                            relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                        }
                        w.focus_id = ed;
                        if let Some(path) = about_doc_path() {
                            nvim_open_file(&nvim_sock(ed), &path);
                        }
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::ZoomPane => {
                    // Fullscreen the focused pane, hiding the other two; a second
                    // press restores the saved layout (#40).
                    exit_copy(&mut copy, &mut windows);
                    help = None;
                    picker = None;
                    let (c, r) = size;
                    let w = &mut windows[cur];
                    if let Some(prev) = w.zoom_prev.take() {
                        (w.show_editor, w.show_shell, w.show_ai) = prev;
                    } else if let Some(role) = w.focus_role() {
                        w.zoom_prev = Some((w.show_editor, w.show_shell, w.show_ai));
                        w.show_editor = role == PaneRole::Editor;
                        w.show_shell = role == PaneRole::Shell;
                        w.show_ai = role == PaneRole::Ai;
                    }
                    refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                    relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                    dirty = true;
                    needs_clear = true;
                }
                Msg::ToggleDiff => {
                    exit_copy(&mut copy, &mut windows);
                    help = None;
                    picker = None;
                    // With this window's full-screen diff view up, the same key
                    // closes it; the flag watcher then restores the layout.
                    let zoom_here = diff_zoom
                        .as_ref()
                        .is_some_and(|z| windows[cur].panes.iter().any(|p| p.id == z.editor_id));
                    if zoom_here {
                        let ed = diff_zoom.as_ref().unwrap().editor_id;
                        nvim_diff_close(&nvim_sock(ed));
                    } else if diff.is_some() {
                        diff = None;
                    } else {
                        diff = Some(DiffList::new(&windows[cur].dir));
                        let (c, r) = size;
                        let w = &mut windows[cur];
                        // The panel lives over the terminal pane. If that pane is
                        // hidden the list has no home — it floated mid-screen and
                        // couldn't take focus to be driven or dismissed (#51). So
                        // reveal the terminal first, then land focus on it so the
                        // arrows/Esc drive the list immediately.
                        if !w.show_shell {
                            w.show_shell = true;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                        }
                        w.focus_id = w.active[1];
                    }
                    dirty = true;
                    needs_clear = true;
                }
                Msg::DiffClosed(ed) => {
                    if diff_zoom.as_ref().is_some_and(|z| z.editor_id == ed) {
                        let z = diff_zoom.take().unwrap();
                        if let Some(w) = windows
                            .iter_mut()
                            .find(|w| w.panes.iter().any(|p| p.id == ed))
                        {
                            (w.show_editor, w.show_shell, w.show_ai) = z.prev;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            let (c, r) = size;
                            relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                        }
                        // The panel persists across the full-screen view;
                        // refresh it so the counts reflect the edits made, and
                        // return focus to the panel (over the terminal pane) so
                        // Esc/arrows drive the list again instead of the editor
                        // that just had focus for scrolling the diff (#51).
                        if let Some(dl) = diff.as_mut() {
                            // Refresh in place, so Esc from the full-screen view
                            // returns to the list you left — a commit range
                            // included (#74).
                            dl.refresh();
                            let w = &mut windows[cur];
                            if w.show_shell {
                                w.focus_id = w.active[1];
                            }
                        }
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::GrugZoom(id, on) => {
                    if on {
                        // Fullscreen the editor for the grug-far search: save the
                        // window's pane visibility, show only the editor, focus
                        // it (#53).
                        if grug_zoom.is_none() {
                            if let Some(w) = windows
                                .iter_mut()
                                .find(|w| w.panes.iter().any(|p| p.id == id))
                            {
                                grug_zoom = Some(DiffZoom {
                                    editor_id: id,
                                    prev: (w.show_editor, w.show_shell, w.show_ai),
                                });
                                w.show_editor = true;
                                w.show_shell = false;
                                w.show_ai = false;
                                w.focus_id = id;
                                refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                                let (c, r) = size;
                                relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                                dirty = true;
                                needs_clear = true;
                            }
                        }
                    } else if grug_zoom.as_ref().is_some_and(|z| z.editor_id == id) {
                        let z = grug_zoom.take().unwrap();
                        if let Some(w) = windows
                            .iter_mut()
                            .find(|w| w.panes.iter().any(|p| p.id == id))
                        {
                            (w.show_editor, w.show_shell, w.show_ai) = z.prev;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            let (c, r) = size;
                            relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                            dirty = true;
                            needs_clear = true;
                        }
                    }
                }
                Msg::CopyMode => {
                    // Enter copy/scroll mode on the focused pane (no overlays
                    // up; the diff panel yields rather than blocking).
                    diff = None;
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
                    diff = None;
                    picker = if picker.is_some() {
                        None
                    } else {
                        picker_new_window = false;
                        Some(Picker::new_excluding(proot.clone(), &open_dirs(&windows)))
                    };
                    dirty = true;
                    needs_clear = true;
                }
                Msg::NewWindow => {
                    exit_copy(&mut copy, &mut windows);
                    help = None;
                    diff = None;
                    picker = Some(Picker::new_excluding(proot.clone(), &open_dirs(&windows)));
                    picker_new_window = true;
                    dirty = true;
                    needs_clear = true;
                }
                Msg::OpenNotes => {
                    // Singleton at position 0: jump to Notes if it exists, else
                    // create it and open a fresh timestamped note. The note goes
                    // in via spec.files so the just-launched nvim opens it itself
                    // (its RPC isn't ready to drive yet) (#70).
                    exit_copy(&mut copy, &mut windows);
                    help = None;
                    picker = None;
                    diff = None;
                    if let Some(i) = windows.iter().position(|w| w.notes) {
                        cur = i;
                    } else {
                        let dir = notes_dir();
                        let note = notes_note_path(&dir, chrono::Local::now());
                        if let Some(day) = note.parent() {
                            let _ = std::fs::create_dir_all(day);
                        }
                        let base = next_id;
                        next_id = base + 3;
                        let spec = WindowSpec {
                            dir,
                            shell_cwd: None,
                            files: vec![note],
                        };
                        let w = new_window(&spec, &cfg, &shell, base, &tx)?;
                        windows.insert(0, w);
                        cur = 0;
                    }
                    let (c, r) = size;
                    let w = &mut windows[cur];
                    relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
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
                    let role = windows[cur].focus_role();
                    match role {
                        // In Notes, a new "tab" (mod-r) is a fresh timestamped
                        // note filed under History/YYYY/MM/DD; the editor opens it
                        // and autosave writes it once you type (#70).
                        Some(PaneRole::Editor) if windows[cur].notes => {
                            let w = &windows[cur];
                            let ed = w.active[role_index(PaneRole::Editor)];
                            let note = notes_note_path(&w.dir, chrono::Local::now());
                            if let Some(day) = note.parent() {
                                let _ = std::fs::create_dir_all(day);
                            }
                            nvim_open_file(&nvim_sock(ed), &note);
                            dirty = true;
                        }
                        Some(PaneRole::Shell) => {
                            let id = next_id;
                            next_id += 1;
                            let w = &mut windows[cur];
                            let dir = w.dir.clone();
                            let pane = Pane::spawn(
                                command(&shell, &dir),
                                "shell".into(),
                                PaneRole::Shell,
                                1,
                                1,
                                id,
                                tx.clone(),
                            )?;
                            w.panes.push(pane);
                            w.active[role_index(PaneRole::Shell)] = id;
                            w.focus_id = id;
                            refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                            relayout(&mut w.panes, &w.layout, c, area)?;
                            dirty = true;
                            needs_clear = true;
                        }
                        Some(PaneRole::Ai) => {
                            // For claude, ask resume-vs-fresh (#54); the next key
                            // resolves it. Other AI CLIs just open fresh.
                            if cfg.ai.trim_start().starts_with("claude") {
                                ai_tab_prompt = true;
                            } else {
                                open_ai_tab(
                                    &mut windows,
                                    cur,
                                    &fresh_ai_cmd(&cfg),
                                    &mut next_id,
                                    &cfg,
                                    &tx,
                                    size,
                                )?;
                            }
                            dirty = true;
                            needs_clear = true;
                        }
                        _ => {}
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
                        if w.notes {
                            // Notes: close the current note; if it was the last
                            // one, open a fresh timestamped note so the window is
                            // never left empty (#70).
                            let note = notes_note_path(&w.dir, chrono::Local::now());
                            if let Some(day) = note.parent() {
                                let _ = std::fs::create_dir_all(day);
                            }
                            nvim_notes_close(&nvim_sock(w.focus_id), &note);
                        } else {
                            // Editor is a single nvim: close the current buffer
                            // (tab) over RPC, leaving nvim alive.
                            nvim_close_buffer(&nvim_sock(w.focus_id));
                        }
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
                    // `mod-N` counts back through recently-used projects rather
                    // than indexing the window list, so `mod-1` is always the
                    // one you just came from (#77).
                    let target = n
                        .checked_sub(1)
                        .and_then(|p| mru_order(&windows, cur).get(p).copied());
                    if let Some(n) = target {
                        exit_copy(&mut copy, &mut windows);
                        cur = n;
                        // The git-diff panel follows the current project (#60):
                        // if it's open, rebuild it for the new window's repo and
                        // reveal + focus the terminal pane it lives over, just
                        // like opening it fresh there.
                        if diff.is_some() {
                            diff = Some(DiffList::new(&windows[cur].dir));
                            let (c, r) = size;
                            let w = &mut windows[cur];
                            if !w.show_shell {
                                w.show_shell = true;
                                refresh_layout(w, cfg.editor_ratio, cfg.right_ratio);
                                relayout(&mut w.panes, &w.layout, c, r.saturating_sub(1))?;
                            }
                            w.focus_id = w.active[1];
                        }
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::WorktreeResult(res) => {
                    // A create worker finished (#50). On success, open the new
                    // worktree as its own window; on failure, keep the form up
                    // with the error.
                    match res {
                        Ok(dir) => {
                            wtform = None;
                            needs_clear = true;
                            // A worktree that already has a window (re-created
                            // over an existing path) focuses it instead (#82).
                            if let Some(i) = window_for_dir(&windows, &dir) {
                                if i != cur {
                                    use_tick += 1;
                                    windows[cur].used = use_tick;
                                    cur = i;
                                }
                                dirty = true;
                                continue;
                            }
                            let (c, r) = size;
                            let area = r.saturating_sub(1);
                            let base = next_id;
                            next_id = base + 3;
                            let spec = WindowSpec::bare(dir);
                            windows.push(new_window(&spec, &cfg, &shell, base, &tx)?);
                            cur = windows.len() - 1;
                            let w = &mut windows[cur];
                            relayout(&mut w.panes, &w.layout, c, area)?;
                        }
                        Err(msg) => {
                            if let Some(wf) = wtform.as_mut() {
                                wf.fail(msg);
                            }
                        }
                    }
                    dirty = true;
                }
                Msg::WorktreeProgress(s) => {
                    if let Some(wf) = wtform.as_mut() {
                        wf.set_progress(s);
                    }
                    dirty = true;
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

        // Stamp the window we ended the batch on as the most recently used, so
        // `mod-N` numbering reflects where you've actually been (#77). Keying
        // off the tick rather than each `cur = …` site catches every way the
        // current window changes (select, new, close, Notes).
        if windows[cur].used != use_tick {
            use_tick += 1;
            windows[cur].used = use_tick;
            dirty = true;
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
                // Notes sync status shows first, but only in the Notes window (#71).
                if windows[cur].notes {
                    let ns = notes_status.lock().unwrap().clone();
                    if !ns.is_empty() {
                        segs.push(ns);
                    }
                }
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
                // A favorite toggle has no other feedback — the editor pane
                // looks identical — so lead the status bar with it (#80).
                let right_info = match &fav_note {
                    Some(n) => format!("★ {n}   {}", segs.join("  ")),
                    None => segs.join("  "),
                };
                framebuf.clear();
                let ksnap = *keys.lock().unwrap();
                let eksnap = editor_keys.lock().unwrap().clone();
                // The diff panel hides while this window is zoomed for the
                // full-screen view (its home, the terminal pane, is hidden).
                let dl = if diff_zoom
                    .as_ref()
                    .is_some_and(|z| windows[cur].panes.iter().any(|p| p.id == z.editor_id))
                {
                    None
                } else {
                    diff.as_ref()
                };
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
                    wtform.as_ref(),
                    ai_tab_prompt,
                    switcher.as_ref(),
                    favs.as_ref(),
                    dl,
                    &right_info,
                    ksnap,
                    &eksnap,
                    msel.as_ref().map(|m| {
                        let (s, e) = m.range();
                        (m.pane_id, s, e)
                    }),
                )?;
                let payload = differ.frame(&framebuf, size.0, size.1, needs_clear);
                if protocol::write_frame(cl, protocol::OUTPUT, &payload).is_err() {
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
    // Ask for the kitty keyboard protocol in "disambiguate" mode (#67) so chords
    // with no legacy encoding — Ctrl-Space, the prefix — arrive as distinct CSI-u
    // events. Opt-in (`kitty_keys`): a terminal that half-implements it can
    // mangle ordinary keys, and that is not worth risking for a session that
    // works fine on the byte path. TerminalGuard pops it on every exit path.
    let (ccfg, _) = Config::load();
    if ccfg.kitty_keys {
        out.write_all(b"\x1b[>1u")?;
        out.flush()?;
    }
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

/// Parse a kitty key event's parameters (`code[:shifted:base][;mods[:type]]`,
/// #67) into `(code, ctrl, alt)`. Shift is ignored: the terminal already sends
/// the shifted code point, so the modifier adds nothing we act on.
fn parse_key_event(params: &[u8]) -> Option<(u32, bool, bool)> {
    let s = std::str::from_utf8(params).ok()?;
    let mut fields = s.split(';');
    // Sub-parameters after `:` describe alternate keys we don't use.
    let code: u32 = fields.next()?.split(':').next()?.parse().ok()?;
    // The modifier field is a bitmask + 1; absent means no modifiers. Bit 0 is
    // shift, 1 alt, 2 ctrl.
    let mods: u32 = fields
        .next()
        .and_then(|f| f.split(':').next())
        .and_then(|m| m.parse::<u32>().ok())
        .unwrap_or(1)
        .saturating_sub(1);
    Some((code, mods & 0b100 != 0, mods & 0b010 != 0))
}

/// Whether a modified-arrow parameter (`1;<mods>`) carries modifiers beyond the
/// shift/alt/ctrl set that panes actually understand. Terminals in enhanced
/// keyboard modes report extra bits (super/hyper/caps/num), and readline prints
/// the tail of a sequence it can't parse as literal text — "29A" (#74). Those
/// get flattened to a plain arrow; ordinary Ctrl-/Alt-arrows pass through so
/// nvim still sees them.
fn exotic_modifier(params: &[u8]) -> bool {
    let Some(rest) = params.strip_prefix(b"1;") else {
        return false;
    };
    match std::str::from_utf8(rest)
        .ok()
        .and_then(|s| s.split(':').next()?.parse::<u32>().ok())
    {
        // mods is a bitmask + 1; 2..=8 covers shift/alt/ctrl and their combos.
        Some(m) => !(2..=8).contains(&m),
        None => true,
    }
}

/// Re-encode a key event as the bytes a terminal without the kitty protocol
/// would have sent, so panes see exactly what they always have (#67). Empty when
/// there's no legacy encoding (Ctrl-Space is NUL, which is why it can be a
/// prefix in the first place).
fn legacy_bytes(code: u32, ctrl: bool, alt: bool) -> Vec<u8> {
    let mut out = Vec::new();
    // Alt is the classic ESC prefix.
    if alt {
        out.push(0x1b);
    }
    match (ctrl, char::from_u32(code)) {
        // Ctrl masks bits 6/7, and only over the ASCII range that maps onto a
        // control code — Ctrl-Space (NUL) and friends have no usable byte.
        (true, Some(c)) if c.is_ascii() => {
            let b = (c as u8).to_ascii_uppercase() & 0x1f;
            if b == 0 {
                return Vec::new();
            }
            out.push(b);
        }
        (false, Some(c)) => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        _ => return Vec::new(),
    }
    out
}

/// Reader state: what we're in the middle of parsing on the stdin byte stream.
enum InState {
    /// Normal keystrokes.
    Normal,
    /// Saw the `Ctrl-a` prefix; next byte is a command.
    Prefix,
    /// Saw `ESC`; might be the start of a CSI/mouse sequence.
    Esc,
    /// Saw `ESC [`; a `<` next means an SGR mouse sequence, otherwise we collect
    /// parameters until the final byte (`u` = a kitty key event, #67).
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
    /// Parameter bytes of the CSI sequence being collected (#67).
    csi: Vec<u8>,
    passthrough: Vec<u8>,
    /// The chord that opens a command. `prefix.byte` is `None` for a chord with
    /// no legacy encoding (Ctrl-Space), which only a kitty-protocol terminal can
    /// deliver — `fallback` then keeps Ctrl-a live so no terminal locks the user
    /// out (#67).
    prefix: config::Prefix,
    fallback: Option<u8>,
    /// The prefix was pressed and a mouse sequence arrived before the command
    /// key did (#76). Moving the mouse isn't a keystroke, so it must not spend
    /// the prefix: the sequence is parsed as usual and the prefix re-arms after
    /// it, leaving `Ctrl-a`, twitch, `1` working like `Ctrl-a 1`.
    prefix_pending: bool,
    /// Shared with the event loop so live rebinds (the `Ctrl-a ?` editor) take
    /// effect immediately: the parser reads the current mapping on each prefix.
    keys: Arc<Mutex<Keys>>,
}

impl InputParser {
    fn new(prefix: config::Prefix, keys: Arc<Mutex<Keys>>) -> InputParser {
        InputParser {
            state: InState::Normal,
            mouse: Vec::new(),
            csi: Vec::new(),
            passthrough: Vec::new(),
            // Ctrl-a stays live only when the configured prefix can't be typed
            // without the kitty protocol; otherwise it's an ordinary key.
            fallback: prefix.byte.is_none().then_some(0x01),
            prefix,
            prefix_pending: false,
            keys,
        }
    }

    /// Whether byte `b` opens a command: the configured prefix, or the Ctrl-a
    /// fallback when the configured one is CSI-u only.
    fn is_prefix_byte(&self, b: u8) -> bool {
        self.prefix.byte == Some(b) || self.fallback == Some(b)
    }

    /// Flush buffered passthrough as a `Msg::Input` into `out`.
    fn flush(&mut self, out: &mut Vec<Msg>) {
        if !self.passthrough.is_empty() {
            out.push(Msg::Input(std::mem::take(&mut self.passthrough)));
        }
    }

    /// Resolve the key pressed after the prefix into a command. Shared by the
    /// byte path and the kitty key-event path (#67), which translates its chord
    /// to the same byte first.
    fn command(&mut self, b: u8, out: &mut Vec<Msg>) {
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
        } else if c == k.about {
            Some(Msg::OpenAbout)
        } else if c == k.win_list {
            Some(Msg::OpenWindowList)
        } else if c == k.zoom {
            Some(Msg::ZoomPane)
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
        } else if c == k.git_diff {
            Some(Msg::ToggleDiff)
        } else if c == k.detach {
            Some(Msg::Detach)
        } else if c == k.reload {
            Some(Msg::Reload)
        } else if c == k.fresh {
            Some(Msg::ClearSession)
        } else if b == b'0' {
            Some(Msg::OpenNotes)
        } else if b.is_ascii_digit() {
            // Notes owns index 0 (mod-0), so mod-N selects index N (#70).
            Some(Msg::SelectWindow((b - b'0') as usize))
        } else if self.is_prefix_byte(b) {
            // prefix prefix -> a literal prefix byte to the child.
            self.passthrough.push(b);
            None
        } else {
            None
        };
        if let Some(m) = cmd {
            self.flush(out);
            out.push(m);
        }
    }

    /// Handle a kitty key event (`ESC [ code ; mods u`, #67). The prefix chord
    /// opens a command; everything else is re-encoded to the bytes a plain
    /// terminal would have sent, because the panes below us (bash/readline
    /// especially) don't speak the protocol — only the outer client does.
    fn key_event(&mut self, params: &[u8], out: &mut Vec<Msg>) {
        let Some(ev) = parse_key_event(params) else {
            self.prefix_pending = false;
            return;
        };
        let (code, ctrl, alt) = ev;
        if code == self.prefix.code && ctrl == self.prefix.ctrl && alt == self.prefix.alt {
            // The prefix itself: arm it (pressing it twice sends one literally,
            // matching the byte path).
            if matches!(self.state, InState::Prefix) && !self.prefix_pending {
                self.passthrough
                    .extend_from_slice(&legacy_bytes(code, ctrl, alt));
                self.state = InState::Normal;
            } else {
                self.prefix_pending = false;
                self.state = InState::Prefix;
            }
            return;
        }
        let bytes = legacy_bytes(code, ctrl, alt);
        // A parked prefix (#76) or one armed by this very chord resolves against
        // the translated byte, so `Ctrl-Space` then `1` works like any command.
        let armed = self.prefix_pending || matches!(self.state, InState::Prefix);
        self.prefix_pending = false;
        if armed {
            self.state = InState::Normal;
            if let Some(&first) = bytes.first() {
                self.command(first, out);
            }
        } else {
            self.passthrough.extend_from_slice(&bytes);
        }
    }

    /// Feed a chunk of input bytes, appending resulting messages to `out`.
    fn feed(&mut self, bytes: &[u8], out: &mut Vec<Msg>) {
        let n = bytes.len();
        for (i, &b) in bytes.iter().enumerate() {
            let last = i + 1 == n;
            match self.state {
                InState::Normal => match b {
                    _ if self.is_prefix_byte(b) => self.state = InState::Prefix,
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
                // An ESC here starts a sequence, not a command: park the prefix
                // and pick it back up once the sequence is parsed (#76). A lone
                // ESC (last byte of the chunk) is a real keypress — same rule as
                // in `Normal` — and cancels the prefix as it always has.
                InState::Prefix if b == 0x1b && !last => {
                    self.prefix_pending = true;
                    self.state = InState::Esc;
                }
                InState::Prefix => {
                    self.state = InState::Normal;
                    self.command(b, out);
                }
                InState::Esc => match b {
                    b'[' => self.state = InState::Csi,
                    _ => {
                        // Not a sequence: this was a real Esc keypress, which
                        // cancels a parked prefix like any other non-command key.
                        self.prefix_pending = false;
                        self.passthrough.push(0x1b);
                        if self.is_prefix_byte(b) {
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
                    if self.csi.is_empty() && b == b'<' {
                        self.state = InState::Mouse;
                        self.mouse.clear();
                    } else if (0x40..=0x7e).contains(&b) {
                        // Final byte: `u` is a kitty key event (#67), anything
                        // else is an ordinary sequence (arrows, Home, …).
                        self.state = InState::Normal;
                        let params = std::mem::take(&mut self.csi);
                        if b == b'u' {
                            self.key_event(&params, out);
                        } else if matches!(b, b'A'..=b'D' | b'H' | b'F') && exotic_modifier(&params)
                        {
                            // A modified arrow/Home/End (`ESC [ 1;<mods> A`).
                            // Panes and overlays only understand the plain form:
                            // readline prints the tail of anything else as
                            // literal text ("29A"), and the diff panel's parser
                            // gives up on it. Drop the modifier and forward the
                            // legacy sequence (#74).
                            self.prefix_pending = false;
                            self.passthrough.extend_from_slice(b"\x1b[");
                            self.passthrough.push(b);
                        } else {
                            // A real key: it isn't a prefix binding, so it spends
                            // a parked prefix and goes to the pane as-is.
                            self.prefix_pending = false;
                            self.passthrough.extend_from_slice(b"\x1b[");
                            self.passthrough.extend_from_slice(&params);
                            self.passthrough.push(b);
                        }
                    } else {
                        self.csi.push(b);
                    }
                }
                InState::Mouse => {
                    if b == b'M' || b == b'm' {
                        // Re-arm a prefix the mouse interrupted (#76).
                        self.state = if self.prefix_pending {
                            self.prefix_pending = false;
                            InState::Prefix
                        } else {
                            InState::Normal
                        };
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

/// Server-side mirror of the composited frame, so the client is sent only what
/// changed instead of the whole screen every render (#46).
///
/// We still build the full frame into `framebuf` exactly as before, but feed it
/// to a vt100 parser that mirrors what the client currently shows, then ship
/// vt100's minimal `contents_diff` against the previously presented screen. A
/// keystroke echo then repaints a couple of cells on the client rather than all
/// ~2000 — that full-screen repaint per keystroke was the typing/general lag.
/// A fresh attach / resize / clear sends the full `contents_formatted` instead.
struct FrameDiffer {
    parser: vt100::Parser,
    prev: Option<vt100::Screen>,
    dims: (u16, u16), // (cols, rows)
}

impl FrameDiffer {
    fn new(cols: u16, rows: u16) -> Self {
        FrameDiffer {
            parser: vt100::Parser::new(rows.max(1), cols.max(1), 0),
            prev: None,
            dims: (cols, rows),
        }
    }

    /// Bytes to ship for this frame. `force_full` (clear / fresh attach) or a
    /// size change sends the whole screen; otherwise a minimal diff against the
    /// last presented screen. Wrapped in DEC 2026 synchronized output so the
    /// client still presents each frame atomically.
    fn frame(&mut self, framebuf: &[u8], cols: u16, rows: u16, force_full: bool) -> Vec<u8> {
        let mut full = force_full;
        if self.dims != (cols, rows) {
            // Resized: start a fresh mirror at the new geometry and repaint all.
            self.parser = vt100::Parser::new(rows.max(1), cols.max(1), 0);
            self.prev = None;
            self.dims = (cols, rows);
            full = true;
        }
        self.parser.process(framebuf);
        let cur = self.parser.screen().clone();
        let body = match (&self.prev, full) {
            (Some(prev), false) => cur.contents_diff(prev),
            _ => cur.contents_formatted(),
        };
        self.prev = Some(cur);
        let mut out = Vec::with_capacity(body.len() + 16);
        out.extend_from_slice(b"\x1b[?2026h");
        out.extend_from_slice(&body);
        out.extend_from_slice(b"\x1b[?2026l");
        out
    }
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
    wtform: Option<&WorktreeForm>,
    ai_prompt: bool,
    switcher: Option<&WinSwitcher>,
    favs: Option<&FavList>,
    diff: Option<&DiffList>,
    right_info: &str,
    keys: Keys,
    editor_keys: &config::EditorKeys,
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
        draw_help(out, cols, rows, keys, editor_keys, h)?;
    }
    if let Some(pk) = picker {
        pk.render(out, cols, rows)?;
    }
    if let Some(wf) = wtform {
        wf.render(out, cols, rows)?;
    }
    if let Some(dl) = diff {
        dl.render(
            out,
            &diff_overlay_rect(w, cols, rows),
            w.focus_id == w.active[1],
        )?;
    }
    if let Some(sw) = switcher {
        sw.render(out, windows, cur, cols, rows)?;
    }
    if let Some(fl) = favs {
        fl.render(out, cols, rows)?;
    }
    if ai_prompt {
        draw_ai_tab_prompt(out, cols, rows)?;
    }
    // End the synchronized frame: present everything queued above at once.
    out.extend_from_slice(b"\x1b[?2026l");
    out.flush()?;
    Ok(())
}

/// The window switcher (#78): a modal list of the open projects, filtered by
/// typing and picked by Enter or by the window's `mod-N` number.
struct WinSwitcher {
    /// Typed filter; empty means "show everything", and only then do digits
    /// jump to a window instead of filtering (project names contain digits).
    filter: String,
    /// Highlighted position within the filtered rows.
    sel: usize,
}

/// One row of the switcher: which window, and the `mod-N` number it answers to
/// (`None` for the current window, which has no number).
struct WinRow {
    idx: usize,
    number: Option<usize>,
}

impl WinSwitcher {
    /// Rows in the order they're shown: the recency-numbered projects first
    /// (`mod-1` at the top, which is what you're usually reaching for), then
    /// Notes, then the window you're already in.
    fn rows(windows: &[Window], cur: usize) -> Vec<WinRow> {
        let numbers = tab_numbers(windows, cur);
        let mut rows: Vec<WinRow> = (0..windows.len())
            .map(|idx| WinRow {
                idx,
                number: numbers[idx],
            })
            .collect();
        rows.sort_by_key(|r| match r.number {
            Some(0) => (1, 0), // Notes
            Some(n) => (0, n), // projects, most recent first
            None => (2, 0),    // the current window
        });
        rows
    }

    /// Rows surviving the filter (case-insensitive substring of the title).
    fn filtered(&self, windows: &[Window], cur: usize) -> Vec<WinRow> {
        let f = self.filter.to_lowercase();
        Self::rows(windows, cur)
            .into_iter()
            .filter(|r| f.is_empty() || windows[r.idx].title.to_lowercase().contains(&f))
            .collect()
    }

    /// Handle a key. `Some(idx)` selects that window and closes the switcher;
    /// the caller closes it on `None` only when `done` is set.
    fn feed(&mut self, windows: &[Window], cur: usize, b: u8, done: &mut bool) -> Option<usize> {
        let rows = self.filtered(windows, cur);
        match b {
            0x1b | 0x03 => *done = true, // Esc / Ctrl-c
            b'\r' | b'\n' => {
                *done = true;
                return rows.get(self.sel).map(|r| r.idx);
            }
            // A digit with nothing typed yet is the window's own number.
            b'0'..=b'9' if self.filter.is_empty() => {
                let n = (b - b'0') as usize;
                *done = true;
                return rows.iter().find(|r| r.number == Some(n)).map(|r| r.idx);
            }
            0x7f | 0x08 => {
                self.filter.pop();
                self.sel = 0;
            }
            0x20..=0x7e => {
                self.filter.push(b as char);
                self.sel = 0;
            }
            _ => {}
        }
        None
    }

    /// Move the highlight, clamped to the filtered rows.
    fn step(&mut self, windows: &[Window], cur: usize, delta: i32) {
        let n = self.filtered(windows, cur).len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as i64 + delta as i64).clamp(0, n as i64 - 1) as usize;
    }

    /// Draw the popup centered over the screen.
    fn render(
        &self,
        out: &mut Vec<u8>,
        windows: &[Window],
        cur: usize,
        cols: u16,
        rows_h: u16,
    ) -> Result<()> {
        let rows = self.filtered(windows, cur);
        let hint = " type to filter · number jumps · Enter open · Esc cancel ";
        let widest = rows
            .iter()
            .map(|r| windows[r.idx].title.chars().count() + 6)
            .chain(std::iter::once(hint.chars().count()))
            .max()
            .unwrap_or(20);
        let iw = widest.min(cols.saturating_sub(4) as usize);
        let bw = iw as u16 + 2;
        // Rows + the filter line + the hint line + both borders.
        let bh = rows.len() as u16 + 4;
        if bw > cols || bh > rows_h {
            return Ok(());
        }
        let x = (cols - bw) / 2;
        let y = (rows_h - bh) / 2;
        let line = |s: &str| -> String {
            let t: String = s.chars().take(iw).collect();
            format!("{t}{}", " ".repeat(iw.saturating_sub(t.chars().count())))
        };
        queue!(
            out,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            cursor::MoveTo(x, y),
            Print(format!("┌{}┐", "─".repeat(iw))),
            SetAttribute(Attribute::Reset),
        )?;
        // Filter line, so you can see what you've typed.
        queue!(
            out,
            cursor::MoveTo(x, y + 1),
            SetForegroundColor(Color::Cyan),
            Print("│"),
            SetForegroundColor(Color::White),
            Print(line(&format!(" › {}▏", self.filter))),
            SetForegroundColor(Color::Cyan),
            Print("│"),
        )?;
        for (i, r) in rows.iter().enumerate() {
            let w = &windows[r.idx];
            let tag = match r.number {
                Some(n) => format!("{n}"),
                None => "•".to_string(), // where you are now
            };
            queue!(
                out,
                cursor::MoveTo(x, y + 2 + i as u16),
                SetForegroundColor(Color::Cyan),
                Print("│"),
            )?;
            if i == self.sel {
                queue!(
                    out,
                    SetBackgroundColor(Color::Cyan),
                    SetForegroundColor(Color::Black)
                )?;
            } else {
                queue!(out, ResetColor)?;
            }
            queue!(out, Print(line(&format!(" {tag}  {}", w.title))))?;
            queue!(out, ResetColor, SetForegroundColor(Color::Cyan), Print("│"))?;
        }
        queue!(
            out,
            cursor::MoveTo(x, y + 2 + rows.len() as u16),
            SetForegroundColor(Color::Cyan),
            Print("│"),
            SetForegroundColor(Color::DarkGrey),
            Print(line(hint)),
            SetForegroundColor(Color::Cyan),
            Print("│"),
            cursor::MoveTo(x, y + 3 + rows.len() as u16),
            Print(format!("└{}┘", "─".repeat(iw))),
            ResetColor,
            SetAttribute(Attribute::Reset),
            cursor::Hide,
        )?;
        Ok(())
    }
}

/// The favorites list (#80): the repo's favorite files, filtered by typing and
/// opened with Enter. Favorites are shared by every worktree and clone of the
/// repo, so an entry can be missing in *this* checkout — those are listed
/// dimmed and can't be opened, rather than silently dropped.
struct FavList {
    entries: Vec<favorites::Favorite>,
    filter: String,
    sel: usize,
}

impl FavList {
    fn new(dir: &Path) -> FavList {
        FavList {
            entries: favorites::list(dir),
            filter: String::new(),
            sel: 0,
        }
    }

    /// Indices of the entries matching the filter.
    fn matches(&self) -> Vec<usize> {
        let f = self.filter.to_lowercase();
        (0..self.entries.len())
            .filter(|&i| f.is_empty() || self.entries[i].rel.to_lowercase().contains(&f))
            .collect()
    }

    fn step(&mut self, delta: i32) {
        let n = self.matches().len();
        if n > 0 {
            self.sel = (self.sel as i64 + delta as i64).clamp(0, n as i64 - 1) as usize;
        }
    }

    /// Handle a key; `Some(path)` opens that file and closes the list.
    fn feed(&mut self, b: u8, done: &mut bool) -> Option<PathBuf> {
        match b {
            0x1b | 0x03 => *done = true,
            b'\r' | b'\n' => {
                let hit = self.matches().get(self.sel).map(|&i| &self.entries[i]);
                // A favorite missing here can't be opened; leave the list up so
                // it's clear which entry was refused.
                if let Some(f) = hit.filter(|f| f.exists) {
                    *done = true;
                    return Some(f.abs.clone());
                }
            }
            0x7f | 0x08 => {
                self.filter.pop();
                self.sel = 0;
            }
            0x20..=0x7e => {
                self.filter.push(b as char);
                self.sel = 0;
            }
            _ => {}
        }
        None
    }

    fn render(&self, out: &mut Vec<u8>, cols: u16, rows_h: u16) -> Result<()> {
        let hits = self.matches();
        let hint = if self.entries.is_empty() {
            " no favorites yet — Ctrl-a B favorites the editor's file "
        } else {
            " type to filter · Enter open · Esc cancel "
        };
        let widest = hits
            .iter()
            .map(|&i| self.entries[i].rel.chars().count() + 4)
            .chain(std::iter::once(hint.chars().count()))
            .max()
            .unwrap_or(20);
        let iw = widest.min(cols.saturating_sub(4) as usize);
        let bw = iw as u16 + 2;
        let bh = hits.len() as u16 + 4;
        if bw > cols || bh > rows_h {
            return Ok(());
        }
        let (x, y) = ((cols - bw) / 2, (rows_h - bh) / 2);
        let line = |s: &str| -> String {
            let t: String = s.chars().take(iw).collect();
            format!("{t}{}", " ".repeat(iw.saturating_sub(t.chars().count())))
        };
        queue!(
            out,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold),
            cursor::MoveTo(x, y),
            Print(format!("┌{}┐", "─".repeat(iw))),
            SetAttribute(Attribute::Reset),
            cursor::MoveTo(x, y + 1),
            SetForegroundColor(Color::Cyan),
            Print("│"),
            SetForegroundColor(Color::White),
            Print(line(&format!(" ★ {}▏", self.filter))),
            SetForegroundColor(Color::Cyan),
            Print("│"),
        )?;
        for (row, &i) in hits.iter().enumerate() {
            let f = &self.entries[i];
            queue!(
                out,
                cursor::MoveTo(x, y + 2 + row as u16),
                SetForegroundColor(Color::Cyan),
                Print("│"),
            )?;
            if row == self.sel {
                queue!(
                    out,
                    SetBackgroundColor(Color::Cyan),
                    SetForegroundColor(Color::Black)
                )?;
            } else if !f.exists {
                // Dimmed: the favorite belongs to the repo but not this checkout.
                queue!(out, ResetColor, SetForegroundColor(Color::DarkGrey))?;
            } else {
                queue!(out, ResetColor, SetForegroundColor(Color::White))?;
            }
            let mark = if f.exists { " " } else { "!" };
            queue!(out, Print(line(&format!(" {mark} {}", f.rel))))?;
            queue!(out, ResetColor, SetForegroundColor(Color::Cyan), Print("│"))?;
        }
        queue!(
            out,
            cursor::MoveTo(x, y + 2 + hits.len() as u16),
            SetForegroundColor(Color::Cyan),
            Print("│"),
            SetForegroundColor(Color::DarkGrey),
            Print(line(hint)),
            SetForegroundColor(Color::Cyan),
            Print("│"),
            cursor::MoveTo(x, y + 3 + hits.len() as u16),
            Print(format!("└{}┘", "─".repeat(iw))),
            ResetColor,
            SetAttribute(Attribute::Reset),
            cursor::Hide,
        )?;
        Ok(())
    }
}

/// Draw the "new Claude tab: resume or fresh?" chooser, centered (#54).
fn draw_ai_tab_prompt(out: &mut Vec<u8>, cols: u16, rows: u16) -> Result<()> {
    let text = " new Claude tab:  r resume · n new · Esc cancel ";
    let iw = text.chars().count();
    let bw = iw as u16 + 2;
    if bw > cols || rows < 3 {
        return Ok(());
    }
    let x = (cols - bw) / 2;
    let y = rows / 2;
    queue!(
        out,
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        cursor::MoveTo(x, y - 1),
        Print(format!("┌{}┐", "─".repeat(iw))),
        cursor::MoveTo(x, y),
        Print("│"),
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::White),
        Print(text),
        SetForegroundColor(Color::Cyan),
        SetAttribute(Attribute::Bold),
        Print("│"),
        cursor::MoveTo(x, y + 1),
        Print(format!("└{}┘", "─".repeat(iw))),
        ResetColor,
        SetAttribute(Attribute::Reset),
        cursor::Hide,
    )?;
    Ok(())
}

/// Pick the contiguous run of window tabs to show so the strip fits `avail`
/// columns on one row and always includes `cur` (#55). Returns the `start..end`
/// index range; the caller draws a `‹` before it when `start > 0` and a `›`
/// after when `end < n`, so overflow never wraps the status bar to a second row
/// (which would scroll the whole layout up).
fn visible_tabs(widths: &[usize], cur: usize, avail: usize) -> (usize, usize) {
    let n = widths.len();
    if n == 0 {
        return (0, 0);
    }
    if widths.iter().sum::<usize>() <= avail {
        return (0, n); // everything fits, no markers needed
    }
    // Truncated: reserve a column for each overflow marker (worst case both).
    let budget = avail.saturating_sub(2);
    let (mut start, mut end) = (cur.min(n - 1), cur.min(n - 1) + 1);
    let mut used = widths[start];
    loop {
        let mut grew = false;
        if end < n && used + widths[end] <= budget {
            used += widths[end];
            end += 1;
            grew = true;
        }
        if start > 0 && used + widths[start - 1] <= budget {
            start -= 1;
            used += widths[start];
            grew = true;
        }
        if !grew {
            break;
        }
    }
    (start, end)
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
    )?;

    // Tabs must stay on this one row: if they'd overflow they wrap past the last
    // cell and scroll the whole layout up (#55). Fit a scrolling window of tabs
    // (always including the current one) into the space between " forge " and
    // the right-aligned info, with ‹/› markers for hidden tabs.
    // Notes keeps mod-0 (#70); projects are numbered by recency and the current
    // window shows no number at all, matching what `mod-N` actually does (#77).
    let numbers = tab_numbers(windows, cur);
    // The strip itself is ordered by recency (#77): the project you're on sits
    // first with no number, then mod-1, mod-2, … behind it, with Notes (mod-0)
    // pinned to the end. So the tab you'd reach for is always in the same place.
    let order = tab_order(windows, cur);
    let labels: Vec<String> = order
        .iter()
        .map(|&i| match numbers[i] {
            Some(n) => format!(" {}:{} ", n, windows[i].title),
            None => format!(" {} ", windows[i].title),
        })
        .collect();
    let widths: Vec<usize> = labels.iter().map(|s| s.chars().count()).collect();
    let rwidth = if right_info.is_empty() {
        0
    } else {
        right_info.chars().count() + 2
    };
    // Leave the final column untouched so nothing triggers an auto-wrap/scroll.
    let avail = (cols as usize).saturating_sub(rwidth + 1);
    // Scrolling anchors on wherever the current window landed in the strip
    // (position 1 when Notes is pinned ahead of it).
    let anchor = order.iter().position(|&i| i == cur).unwrap_or(0);
    let (start, end) = visible_tabs(&widths, anchor, avail);
    if start > 0 {
        queue!(
            out,
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White),
            Print("‹")
        )?;
    }
    for (slot, &i) in order.iter().enumerate().take(end).skip(start) {
        let w = &windows[i];
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
        queue!(out, Print(&labels[slot]))?;
    }
    if end < order.len() {
        queue!(
            out,
            SetBackgroundColor(Color::DarkGrey),
            SetForegroundColor(Color::White),
            Print("›")
        )?;
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

/// What the overlay is currently capturing for a rebind.
enum Capture {
    /// Not capturing.
    None,
    /// Waiting for a single printable key — a prefix (`Ctrl-a X`) binding.
    Char,
    /// Typing a token (e.g. `C-p`, `Space e`) — an editor binding. The text
    /// accumulates in `HelpState::text_buf`.
    Text,
}

/// Which key surface a selectable overlay row belongs to.
enum EditKind {
    /// A prefix binding: single char, rebinds live.
    Prefix,
    /// An editor (Neovim) binding: a token persisted to `[editor_keys]`, applied
    /// on the next reload (`Ctrl-a r`).
    Editor,
}

/// The selectable (editable) rows of the overlay, in display order: the prefix
/// bindings followed by the editor bindings. `HelpState::sel` indexes this list;
/// fixed/read-only rows are drawn but never selected.
fn editable_rows() -> Vec<(EditKind, &'static str, &'static str)> {
    let mut v = Vec::new();
    for (f, l) in config::EDITABLE.iter() {
        v.push((EditKind::Prefix, *f, *l));
    }
    for (f, l) in config::EDITOR_EDITABLE.iter() {
        v.push((EditKind::Editor, *f, *l));
    }
    v
}

/// State of the `Ctrl-a ?` overlay. It starts as a read-only key reference and
/// can flip into an inline editor that rebinds keys (Story #10 / #28).
struct HelpState {
    /// In the editor (rows selectable) vs. just viewing the reference.
    editing: bool,
    /// Selected row, an index into `editable_rows()`.
    sel: usize,
    /// What, if anything, we're capturing for the selected row.
    capture: Capture,
    /// Token being typed while `capture == Text`.
    text_buf: String,
    /// Transient status / error line shown at the bottom.
    msg: String,
    /// First visible body row — the overlay scrolls because the full key list is
    /// taller than most terminals. Set by `draw_help` (which knows the height) to
    /// keep the selected row visible; nudged directly for view-mode scrolling.
    scroll: std::cell::Cell<usize>,
}

impl HelpState {
    fn new() -> HelpState {
        HelpState {
            editing: false,
            sel: 0,
            capture: Capture::None,
            text_buf: String::new(),
            msg: String::new(),
            scroll: std::cell::Cell::new(0),
        }
    }
}

/// A decoded overlay keypress (raw bytes -> intent).
enum HelpKey {
    Up,
    Down,
    Enter,
    Esc,
    Backspace,
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
            0x7f | 0x08 => v.push(HelpKey::Backspace),
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
fn help_input(
    h: &mut HelpState,
    bytes: &[u8],
    keys: &Arc<Mutex<Keys>>,
    ekeys: &Arc<Mutex<config::EditorKeys>>,
) -> bool {
    let rows = editable_rows();
    let n = rows.len();
    for ev in decode_help_keys(bytes) {
        match h.capture {
            // Capturing a single key for a prefix (Ctrl-a X) binding: applied live.
            Capture::Char => match ev {
                HelpKey::Esc => {
                    h.capture = Capture::None;
                    h.msg = "cancelled".into();
                }
                HelpKey::Char(c) => {
                    let field = rows[h.sel].1;
                    let cur = *keys.lock().unwrap();
                    match cur.with_bind(field, c) {
                        Ok(nk) => {
                            *keys.lock().unwrap() = nk; // live: reader threads see it
                            config::persist_keybind(field, c); // survive restart
                            h.capture = Capture::None;
                            h.msg = format!("bound {field} to '{c}'");
                        }
                        Err(e) => h.msg = e, // stay capturing so they can retry
                    }
                }
                _ => h.msg = "press a printable key (Esc to cancel)".into(),
            },
            // Typing a token for an editor binding: persisted; applied on reload.
            Capture::Text => match ev {
                HelpKey::Esc => {
                    h.capture = Capture::None;
                    h.text_buf.clear();
                    h.msg = "cancelled".into();
                }
                HelpKey::Backspace => {
                    h.text_buf.pop();
                }
                HelpKey::Char(c) => h.text_buf.push(c),
                HelpKey::Enter => {
                    let field = rows[h.sel].1;
                    match config::validate_editor_token(&h.text_buf) {
                        Ok(tok) => {
                            ekeys.lock().unwrap().set(field, tok.clone());
                            config::persist_editor_key(field, &tok);
                            h.capture = Capture::None;
                            h.text_buf.clear();
                            h.msg = format!(
                                "{field} = {} · Ctrl-a r to apply",
                                config::disp_token(&tok)
                            );
                        }
                        Err(e) => h.msg = e, // stay in text entry to fix it
                    }
                }
                _ => {}
            },
            Capture::None => {
                if !h.editing {
                    match ev {
                        HelpKey::Esc => return true,
                        HelpKey::Char('e') | HelpKey::Char('E') => {
                            h.editing = true;
                            h.msg.clear();
                        }
                        // View-mode scroll (draw_help clamps to range).
                        HelpKey::Up | HelpKey::Char('k') => {
                            h.scroll.set(h.scroll.get().saturating_sub(1))
                        }
                        HelpKey::Down | HelpKey::Char('j') => h.scroll.set(h.scroll.get() + 1),
                        // 'q' or the (current) help key also closes.
                        HelpKey::Char(c) if c == 'q' || c == keys.lock().unwrap().help => {
                            return true;
                        }
                        _ => {}
                    }
                } else {
                    match ev {
                        HelpKey::Esc => {
                            h.editing = false;
                            h.msg.clear();
                        }
                        HelpKey::Up | HelpKey::Char('k') => h.sel = (h.sel + n - 1) % n,
                        HelpKey::Down | HelpKey::Char('j') => h.sel = (h.sel + 1) % n,
                        HelpKey::Enter => match rows[h.sel].0 {
                            EditKind::Prefix => {
                                h.capture = Capture::Char;
                                h.msg = format!("press new key for {}", rows[h.sel].2);
                            }
                            EditKind::Editor => {
                                h.capture = Capture::Text;
                                h.text_buf.clear();
                                h.msg = format!("type token for {}", rows[h.sel].2);
                            }
                        },
                        HelpKey::Char('q') => return true,
                        _ => {}
                    }
                }
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

/// Byte ranges of OSC 52 clipboard sequences (`ESC ] 52 ; … (BEL | ESC \)`) in
/// raw pane output. forge renders each pane's vt100 *screen*, which swallows
/// these, so we lift them out and forward them verbatim to the client's real
/// terminal — that's how nvim's / the shell's "copy to system clipboard" reaches
/// the clipboard over SSH (#32). Sequences split across output chunks aren't
/// stitched; fine for path-sized payloads.
fn osc52_ranges(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    const START: &[u8] = b"\x1b]52;";
    let mut out = Vec::new();
    let mut i = 0;
    while i + START.len() <= bytes.len() {
        if &bytes[i..i + START.len()] != START {
            i += 1;
            continue;
        }
        let mut j = i + START.len();
        let mut end = None;
        while j < bytes.len() {
            if bytes[j] == 0x07 {
                end = Some(j + 1);
                break;
            }
            if bytes[j] == 0x1b && bytes.get(j + 1) == Some(&0x5c) {
                end = Some(j + 2);
                break;
            }
            j += 1;
        }
        match end {
            Some(e) => {
                out.push(i..e);
                i = e;
            }
            None => break, // incomplete at the chunk boundary
        }
    }
    out
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

/// Print one overlay cell: `s` padded (or truncated) to `w` columns, inverted
/// when it's the selected row. Assumes the box's black-on-cyan is already set.
fn print_cell(out: &mut Vec<u8>, s: &str, w: usize, sel: bool) -> Result<()> {
    let n = s.chars().count();
    let t: String = if n >= w {
        s.chars().take(w).collect()
    } else {
        format!("{}{}", s, " ".repeat(w - n))
    };
    if sel {
        queue!(
            out,
            SetAttribute(Attribute::Reverse),
            Print(&t),
            SetAttribute(Attribute::Reset),
            SetForegroundColor(Color::Black),
            SetBackgroundColor(Color::Cyan)
        )?;
    } else {
        queue!(out, Print(&t))?;
    }
    Ok(())
}

/// Draw the centered `Ctrl-a ?` overlay — a key reference, or the inline key
/// editor when `h.editing`. Two columns (prefix left; editor + fixed right) so
/// the whole key set fits without a tiny scrolling box. Keys come from the live
/// (possibly just-rebound) config so the display always reflects the mapping.
fn draw_help(
    out: &mut Vec<u8>,
    cols: u16,
    rows: u16,
    k: Keys,
    ek: &config::EditorKeys,
    h: &HelpState,
) -> Result<()> {
    let title = if h.editing {
        " CodeForge — edit keys"
    } else {
        " CodeForge — keys"
    };
    let capturing = !matches!(h.capture, Capture::None);
    let p = config::EDITABLE.len();

    // Left column: prefix bindings (single char after Ctrl-a, rebound live).
    let mut left: Vec<(String, bool)> = Vec::new();
    left.push(("Prefix — Ctrl-a + key".to_string(), false));
    for (i, (field, label)) in config::EDITABLE.iter().enumerate() {
        let key = k.get(field).map(config::key_label).unwrap_or_default();
        let sel = h.editing && i == h.sel;
        let marker = if sel { "›" } else { " " };
        left.push((format!("{marker} Ctrl-a {key:<2}  {label}"), sel));
    }

    // Right column: editor bindings (typed tokens, applied on reload) then the
    // fixed reference rows.
    let mut right: Vec<(String, bool)> = Vec::new();
    right.push(("Editor (Neovim) — Ctrl-a r to apply".to_string(), false));
    for (j, (field, label)) in config::EDITOR_EDITABLE.iter().enumerate() {
        let sel = h.editing && p + j == h.sel;
        let marker = if sel { "›" } else { " " };
        let shown = if sel && matches!(h.capture, Capture::Text) {
            format!("{}_", h.text_buf) // live text entry
        } else {
            config::disp_token(ek.get(field).unwrap_or(""))
        };
        right.push((format!("{marker} {shown:<14} {label}"), sel));
    }
    right.push((String::new(), false));
    right.push(("Fixed".to_string(), false));
    // Recency order, not creation order: 1 is the project you were in last, and
    // the one you're in now has no number (#77).
    right.push((
        "  Ctrl-a 1..9    jump to window (1 = last used)".to_string(),
        false,
    ));
    right.push(("  click / wheel  focus / scroll".to_string(), false));
    right.push((
        "  gd gr gi gt    definition/refs/impl/type".to_string(),
        false,
    ));
    right.push(("  K              hover docs".to_string(), false));
    right.push(("  Space rn / ca  rename / code action".to_string(), false));

    // The paired row of the current selection (columns share row numbers).
    let sel_row = if !h.editing {
        None
    } else if h.sel < p {
        Some(h.sel + 1)
    } else {
        Some(h.sel - p + 1)
    };

    let footer = match h.capture {
        Capture::Char => format!(" {}", h.msg),
        Capture::Text => " type e.g. Ctrl-p or Space e · Enter save · Esc cancel".to_string(),
        Capture::None if h.editing => " ↑/↓ select · Enter rebind · Esc back".to_string(),
        Capture::None => " ↑/↓ scroll · e edit · Esc/q close".to_string(),
    };
    let msg_line = (!h.msg.is_empty() && !capturing).then(|| format!(" {}", h.msg));

    // Geometry. Two columns padded to their own widths, joined by a gap; the box
    // is as wide as the content (clamped to the screen) — no cramped single col.
    let n = left.len().max(right.len());
    let lw = left
        .iter()
        .map(|(s, _)| s.chars().count())
        .max()
        .unwrap_or(0);
    let rw = right
        .iter()
        .map(|(s, _)| s.chars().count())
        .max()
        .unwrap_or(0);
    const GAP: usize = 3;
    let foot_h = 1 + msg_line.is_some() as usize;
    // Rows available for the paired list, after borders + title + footer.
    let vh = (rows as usize).saturating_sub(2 + 1 + foot_h).max(1);
    let shown = n.min(vh);
    let max_scroll = n.saturating_sub(shown);
    let mut scroll = h.scroll.get().min(max_scroll);
    if let Some(sr) = sel_row {
        if sr < scroll {
            scroll = sr;
        } else if sr >= scroll + shown {
            scroll = sr + 1 - shown;
        }
    }
    scroll = scroll.min(max_scroll);
    h.scroll.set(scroll);

    let inner = (lw + GAP + rw)
        .max(title.chars().count())
        .max(footer.chars().count())
        .min((cols as usize).saturating_sub(2));
    let rw_eff = rw.min(inner.saturating_sub(lw + GAP));
    let w = (inner + 2) as u16;
    let bh = (2 + 1 + shown + foot_h) as u16;
    if w < 4 || bh < 4 {
        return Ok(());
    }
    let x = (cols.saturating_sub(w)) / 2;
    let y = (rows.saturating_sub(bh)) / 2;

    queue!(
        out,
        cursor::Hide,
        SetForegroundColor(Color::Black),
        SetBackgroundColor(Color::Cyan),
        cursor::MoveTo(x, y),
        Print(format!("┌{}┐", "─".repeat(inner))),
    )?;
    // Title.
    queue!(
        out,
        cursor::MoveTo(x, y + 1),
        Print("│"),
        SetAttribute(Attribute::Bold)
    )?;
    print_cell(out, title, inner, false)?;
    queue!(
        out,
        SetAttribute(Attribute::Reset),
        SetForegroundColor(Color::Black),
        SetBackgroundColor(Color::Cyan),
        Print("│")
    )?;
    // Two-column list rows.
    for (ri, r) in (scroll..scroll + shown).enumerate() {
        let (ls, lsel) = left.get(r).cloned().unwrap_or_default();
        let (rs, rsel) = right.get(r).cloned().unwrap_or_default();
        queue!(out, cursor::MoveTo(x, y + 2 + ri as u16), Print("│"))?;
        print_cell(out, &ls, lw, lsel)?;
        queue!(out, Print(" ".repeat(GAP)))?;
        print_cell(out, &rs, rw_eff, rsel)?;
        // Pad any remaining width (when the box is wider than the two columns).
        let used = lw + GAP + rw_eff;
        if inner > used {
            queue!(out, Print(" ".repeat(inner - used)))?;
        }
        queue!(out, Print("│"))?;
    }
    // Footer (+ optional status line).
    queue!(out, cursor::MoveTo(x, y + 2 + shown as u16), Print("│"))?;
    print_cell(out, &footer, inner, false)?;
    queue!(out, Print("│"))?;
    if let Some(m) = &msg_line {
        queue!(out, cursor::MoveTo(x, y + 3 + shown as u16), Print("│"))?;
        print_cell(out, m, inner, false)?;
        queue!(out, Print("│"))?;
    }
    queue!(
        out,
        cursor::MoveTo(x, y + bh - 1),
        Print(format!("└{}┘", "─".repeat(inner))),
        ResetColor,
        SetAttribute(Attribute::Reset)
    )?;
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
        // Pop the kitty keyboard flags first (#67) — leaving them pushed would
        // hand the user's shell a terminal that reports Esc as CSI 27u. A pop
        // with nothing pushed is ignored, so this is safe on every exit path.
        let _ = out.write_all(b"\x1b[<u");
        let _ = queue!(out, cursor::Show, DisableMouseCapture, LeaveAlternateScreen);
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_differ_ships_full_then_minimal_diffs() {
        // A framebuf that writes `ch` at the top-left via absolute positioning,
        // the way render() composites cells.
        let at = |ch: char| format!("\x1b[1;1H{ch}").into_bytes();
        let mut d = FrameDiffer::new(20, 5);

        // First frame with no prior screen -> full repaint, includes the glyph.
        let p1 = d.frame(&at('A'), 20, 5, false);
        assert!(p1.windows(1).any(|w| w == b"A"), "first frame paints A");

        // Identical frame -> diff against an identical screen is empty; the
        // payload is just the sync wrappers, and much smaller than a full paint.
        let p2 = d.frame(&at('A'), 20, 5, false);
        assert!(p2.len() < p1.len(), "identical frame ships less than full");
        assert!(
            !p2.windows(1).any(|w| w == b"A"),
            "no redraw when unchanged"
        );

        // A changed cell -> a small delta that carries the new glyph, still
        // smaller than a full repaint.
        let p3 = d.frame(&at('B'), 20, 5, false);
        assert!(
            p3.windows(1).any(|w| w == b"B"),
            "change ships the new glyph"
        );
        assert!(
            p3.len() < p1.len(),
            "a one-cell change ships less than full"
        );

        // force_full repaints everything again even when nothing changed.
        let p4 = d.frame(&at('B'), 20, 5, true);
        assert!(p4.windows(1).any(|w| w == b"B"), "force_full repaints");
        assert!(p4.len() > p3.len(), "forced full is larger than a diff");
    }

    #[test]
    fn osc52_ranges_extracts_clipboard_sequences() {
        // BEL-terminated, embedded in surrounding output.
        let bel = b"before\x1b]52;c;aGk=\x07after".to_vec();
        let r = osc52_ranges(&bel);
        assert_eq!(r.len(), 1);
        assert_eq!(&bel[r[0].clone()], b"\x1b]52;c;aGk=\x07");

        // ST-terminated (ESC \) and two sequences in one chunk.
        let two = b"\x1b]52;c;YQ==\x1b\\x\x1b]52;c;Yg==\x07".to_vec();
        let r2 = osc52_ranges(&two);
        assert_eq!(r2.len(), 2);
        assert_eq!(&two[r2[0].clone()], b"\x1b]52;c;YQ==\x1b\\");
        assert_eq!(&two[r2[1].clone()], b"\x1b]52;c;Yg==\x07");

        // No OSC 52 -> nothing; an unterminated one is skipped (chunk boundary).
        assert!(osc52_ranges(b"plain text \x1b[1mbold\x1b[0m").is_empty());
        assert!(osc52_ranges(b"\x1b]52;c;dHJ1bmNhdGVk").is_empty());
    }

    #[test]
    fn notes_note_path_files_by_day() {
        use chrono::TimeZone;
        let now = chrono::Local
            .with_ymd_and_hms(2026, 7, 29, 14, 30, 12)
            .unwrap();
        let p = notes_note_path(Path::new("/n/Notes"), now);
        assert_eq!(
            p,
            PathBuf::from("/n/Notes/History/2026/07/29/2026-07-29-14:30:12.md")
        );
    }

    #[test]
    fn claude_project_key_matches_scheme() {
        // Slashes (and any non-alnum) become '-'; case is kept (#54).
        assert_eq!(
            claude_project_key(std::path::Path::new("/home/bbell/projects/CodeForge")),
            "-home-bbell-projects-CodeForge"
        );
        assert_eq!(
            claude_project_key(std::path::Path::new("/home/bbell/projects/auto-SFAP-1-x")),
            "-home-bbell-projects-auto-SFAP-1-x"
        );
    }

    #[test]
    fn fresh_ai_drops_resume_flags() {
        let mk = |ai: &str| Config {
            ai: ai.into(),
            ..Config::default()
        };
        // #49: a new AI tab is fresh — resume/continue flags are dropped.
        assert_eq!(fresh_ai_cmd(&mk("claude --continue")), "claude");
        assert_eq!(fresh_ai_cmd(&mk("claude")), "claude");
        assert_eq!(fresh_ai_cmd(&mk("augment")), "augment"); // non-claude as-is
    }

    #[test]
    fn visible_tabs_fits_one_row_and_keeps_current() {
        // All fit -> show everything, no markers.
        let w = vec![5, 5, 5];
        assert_eq!(visible_tabs(&w, 1, 100), (0, 3));

        // Overflow: the returned run always includes `cur` and fits `avail`
        // (minus 2 cols reserved for ‹/› markers).
        let w = vec![6; 10]; // 60 total
        for cur in 0..10 {
            let (s, e) = visible_tabs(&w, cur, 20);
            assert!(s <= cur && cur < e, "cur {cur} must be visible in {s}..{e}");
            let used: usize = w[s..e].iter().sum();
            assert!(used <= 20, "run width {used} exceeds avail");
        }

        // A current tab wider than the budget still returns just that tab.
        let w = vec![3, 3, 50, 3];
        let (s, e) = visible_tabs(&w, 2, 10);
        assert_eq!((s, e), (2, 3));
    }

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
        InputParser::new(
            config::Prefix::ctrl_a(),
            Arc::new(Mutex::new(Keys::default())),
        )
    }

    /// A parser whose prefix is Ctrl-Space: no legacy byte, CSI-u only (#67).
    fn ctrl_space_parser() -> InputParser {
        InputParser::new(
            config::Prefix {
                byte: None,
                code: 32,
                ctrl: true,
                alt: false,
            },
            Arc::new(Mutex::new(Keys::default())),
        )
    }

    #[test]
    fn ctrl_space_prefix_via_kitty_protocol() {
        // Ctrl-Space arrives as CSI 32;5u and opens a command: then 'c' toggles
        // the AI pane, exactly as Ctrl-a c does on the byte path (#67).
        let mut p = ctrl_space_parser();
        let mut out = Vec::new();
        p.feed(b"\x1b[32;5u", &mut out);
        assert!(out.is_empty(), "the prefix alone emits nothing");
        p.feed(b"c", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Toggle(PaneRole::Ai)]));

        // Ctrl-a stays live as a fallback, so a terminal without the protocol
        // can never leave the session un-drivable.
        let mut out = Vec::new();
        p.feed(&[0x01, b'c'], &mut out);
        assert!(matches!(out.as_slice(), [Msg::Toggle(PaneRole::Ai)]));
    }

    #[test]
    fn key_events_reach_panes_as_legacy_bytes() {
        // Panes don't speak the protocol, so every non-prefix event is
        // re-encoded to what a plain terminal would have sent (#67).
        assert_eq!(legacy_bytes(27, false, false), vec![0x1b], "Esc");
        assert_eq!(legacy_bytes(97, false, false), b"a".to_vec());
        assert_eq!(legacy_bytes(97, true, false), vec![0x01], "Ctrl-a");
        assert_eq!(legacy_bytes(97, false, true), vec![0x1b, b'a'], "Alt-a");
        assert!(
            legacy_bytes(32, true, false).is_empty(),
            "Ctrl-Space is NUL"
        );

        // Disambiguate mode reports Esc as CSI 27u; the shell must still see 0x1b.
        let mut p = ctrl_space_parser();
        let mut out = Vec::new();
        p.feed(b"\x1b[27u", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == &[0x1b]));

        // An ordinary CSI sequence (arrow) passes through untouched.
        let mut out = Vec::new();
        p.feed(b"\x1b[1;5A", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == b"\x1b[1;5A"));
    }

    #[test]
    fn parses_key_event_params() {
        assert_eq!(parse_key_event(b"32;5"), Some((32, true, false)));
        assert_eq!(parse_key_event(b"27"), Some((27, false, false)), "no mods");
        // Sub-parameters (alternate keys, event type) are ignored.
        assert_eq!(parse_key_event(b"97:65;3:1"), Some((97, false, true)));
        assert_eq!(parse_key_event(b"junk"), None);
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
    fn mouse_motion_doesnt_eat_the_prefix() {
        // #76: prefix, then the mouse twitches before the command key. The
        // motion is dispatched as a mouse event and the prefix survives it, so
        // `Ctrl-a` … `1` still selects window 1 instead of typing a literal 1.
        let mut p = parser();
        let mut out = Vec::new();
        p.feed(&[0x01], &mut out);
        p.feed(b"\x1b[<35;10;5M", &mut out);
        assert!(
            matches!(out.as_slice(), [Msg::Mouse { .. }]),
            "the motion is still parsed as a mouse event"
        );
        out.clear();
        p.feed(b"1", &mut out);
        assert!(
            matches!(out.as_slice(), [Msg::SelectWindow(1)]),
            "the prefix re-arms after the sequence"
        );
    }

    #[test]
    fn prefix_then_arrow_key_is_not_swallowed() {
        // An arrow after the prefix isn't a binding: it spends the prefix and
        // reaches the pane as the keypress it is, rather than leaking `[A` as
        // literal text (#76).
        let mut p = parser();
        let mut out = Vec::new();
        p.feed(&[0x01], &mut out);
        p.feed(b"\x1b[A", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == b"\x1b[A"));
    }

    #[test]
    fn modified_arrows_reach_panes_as_plain_arrows() {
        // Some terminals report arrows with a modifier parameter
        // (`ESC [ 1;29 A`). readline prints the tail of that as literal text
        // ("29A") and the diff panel's parser can't read it, so forge forwards
        // the plain legacy form instead (#74).
        let mut out = Vec::new();
        parser().feed(b"\x1b[1;29A", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == b"\x1b[A"));

        // An unmodified arrow is untouched, and a non-arrow CSI still passes
        // through with its parameters intact.
        let mut out = Vec::new();
        parser().feed(b"\x1b[B", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == b"\x1b[B"));
        let mut out = Vec::new();
        parser().feed(b"\x1b[200~", &mut out);
        assert!(matches!(out.as_slice(), [Msg::Input(b)] if b == b"\x1b[200~"));
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

    /// A window with just enough shape for the numbering helpers.
    fn win(title: &str, notes: bool, used: u64) -> Window {
        Window {
            panes: Vec::new(),
            layout: Layout::Leaf(0),
            focus_id: 0,
            active: [0; 3],
            dir: PathBuf::from("/tmp"),
            title: title.to_string(),
            show_editor: true,
            show_shell: true,
            show_ai: true,
            attention: false,
            last_ai_bell: 0,
            zoom_prev: None,
            notes,
            used,
        }
    }

    #[test]
    fn window_numbers_follow_recency() {
        // Notes, plus three projects last visited in the order c, a, b.
        let ws = [
            win("notes", true, 0),
            win("a", false, 7),
            win("b", false, 5),
            win("c", false, 9),
        ];
        // Current is "c": the others rank by recency, most recent first.
        let order = mru_order(&ws, 3);
        assert_eq!(
            order
                .iter()
                .map(|&i| ws[i].title.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"],
            "mod-1 is the project used most recently before this one"
        );
        // The strip leads with the current window, then recency, Notes last.
        assert_eq!(
            tab_order(&ws, 3)
                .iter()
                .map(|&i| ws[i].title.as_str())
                .collect::<Vec<_>>(),
            ["notes", "c", "a", "b"],
            "Notes stays pinned far left; current next, then recency"
        );
        let nums = tab_numbers(&ws, 3);
        assert_eq!(nums[0], Some(0), "Notes keeps mod-0");
        assert_eq!(nums[3], None, "the current window has no number");
        assert_eq!((nums[1], nums[2]), (Some(1), Some(2)));

        // Switch to "a": "c" is now the last-used, so it becomes mod-1 —
        // bouncing between two projects is always mod-1.
        let mut ws = ws;
        ws[1].used = 10;
        assert_eq!(tab_numbers(&ws, 1)[3], Some(1));
    }

    /// Walking a sequence of switches: the order must always read most-recent
    /// first, including the window just left (#77).
    #[test]
    fn recency_order_tracks_a_switch_sequence() {
        let mut ws = vec![
            win("notes", true, 0),
            win("a", false, 0),
            win("b", false, 0),
            win("c", false, 0),
        ];
        let mut tick = 0u64;
        let mut cur = 1; // start on "a"
        let switch = |ws: &mut Vec<Window>, cur: &mut usize, to: usize, tick: &mut u64| {
            *tick += 1;
            ws[*cur].used = *tick;
            *cur = to;
        };
        switch(&mut ws, &mut cur, 2, &mut tick); // a -> b
        switch(&mut ws, &mut cur, 3, &mut tick); // b -> c
        let names = |ws: &Vec<Window>, cur: usize| {
            mru_order(ws, cur)
                .iter()
                .map(|&i| ws[i].title.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&ws, cur), ["b", "a"], "just-left window is mod-1");
        switch(&mut ws, &mut cur, 1, &mut tick); // c -> a
        assert_eq!(names(&ws, cur), ["c", "b"], "and again after bouncing");
        switch(&mut ws, &mut cur, 3, &mut tick); // a -> c
        assert_eq!(names(&ws, cur), ["a", "b"]);
    }

    #[test]
    fn switcher_filters_and_picks_by_number() {
        let ws = [
            win("notes", true, 0),
            win("alpha", false, 7),
            win("beta", false, 5),
            win("gamma", false, 9),
        ];
        let cur = 3; // "gamma"
        let mut sw = WinSwitcher {
            filter: String::new(),
            sel: 0,
        };
        // Rows lead with the numbered projects, then Notes, then where you are.
        let rows = sw.filtered(&ws, cur);
        assert_eq!(
            rows.iter()
                .map(|r| ws[r.idx].title.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta", "notes", "gamma"]
        );

        // A digit on an empty filter jumps to that window.
        let mut done = false;
        assert_eq!(sw.feed(&ws, cur, b'2', &mut done), Some(2));
        assert!(done, "picking closes the switcher");

        // Typing filters instead, and digits then belong to the filter.
        let mut sw = WinSwitcher {
            filter: String::new(),
            sel: 0,
        };
        let mut done = false;
        for &b in b"bet" {
            assert_eq!(sw.feed(&ws, cur, b, &mut done), None);
        }
        let rows = sw.filtered(&ws, cur);
        assert_eq!(rows.len(), 1);
        assert_eq!(ws[rows[0].idx].title, "beta");
        assert_eq!(sw.feed(&ws, cur, b'\r', &mut done), Some(2));

        // Esc picks nothing.
        let mut sw = WinSwitcher {
            filter: String::new(),
            sel: 0,
        };
        let mut done = false;
        assert_eq!(sw.feed(&ws, cur, 0x1b, &mut done), None);
        assert!(done);
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
        let ekeys = Arc::new(Mutex::new(config::EditorKeys::default()));
        let mut h = HelpState::new();
        // 'e' enters edit mode; Down moves the selection; Esc leaves edit mode.
        assert!(!help_input(&mut h, b"e", &keys, &ekeys));
        assert!(h.editing);
        assert!(!help_input(&mut h, b"\x1b[B", &keys, &ekeys));
        assert_eq!(h.sel, 1);
        assert!(!help_input(&mut h, &[0x1b], &keys, &ekeys));
        assert!(!h.editing);
        // Esc in view mode closes the overlay.
        assert!(help_input(&mut h, &[0x1b], &keys, &ekeys));
    }

    #[test]
    fn help_overlay_edits_editor_key_by_token() {
        let keys = Arc::new(Mutex::new(Keys::default()));
        let ekeys = Arc::new(Mutex::new(config::EditorKeys::default()));
        let mut h = HelpState::new();
        h.editing = true;
        // Select the first editor row (open_file), just past the prefix rows.
        h.sel = config::EDITABLE.len();
        // Enter starts token entry; typing + Backspace edits the buffer.
        assert!(!help_input(&mut h, b"\r", &keys, &ekeys));
        assert!(matches!(h.capture, Capture::Text));
        help_input(&mut h, b"C-x", &keys, &ekeys);
        help_input(&mut h, &[0x7f], &keys, &ekeys); // backspace the 'x'
        help_input(&mut h, b"o", &keys, &ekeys);
        // Enter commits: the editor key is updated in the shared state.
        assert!(!help_input(&mut h, b"\r", &keys, &ekeys));
        assert!(matches!(h.capture, Capture::None));
        assert_eq!(ekeys.lock().unwrap().open_file, "C-o");
    }
}
