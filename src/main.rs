//! CodeForge — terminal-native IDE.
//!
//! Default IDE layout (#4). On start CodeForge opens Neovim (left), a shell and
//! the Claude CLI (stacked, right) — each a child in its own PTY + vt100 emulator
//! (`pane.rs`), arranged by a binary split tree (`layout.rs`) and blitted into
//! bordered rectangles of the real terminal. Input is routed to the focused pane;
//! its border is highlighted. The editor's IDE features (fuzzy find, grep, LSP)
//! come from the Neovim config in `config/nvim/`, loaded via NVIM_APPNAME=codeforge.
//!
//! Controls (Ctrl-a is the prefix):
//!   Ctrl-a |     split the focused pane side by side (new $SHELL)
//!   Ctrl-a -     split the focused pane top/bottom (new $SHELL)
//!   Ctrl-a hjkl  move focus left/down/up/right
//!   Ctrl-a o     cycle focus to the next pane
//!   Ctrl-a x     close the focused pane
//!   Ctrl-a p     open the project picker (re-home the current window)
//!   Ctrl-a c     new window (its own editor/shell/AI for another project)
//!   Ctrl-a n     next window   ·   Ctrl-a 1..9  jump to window
//!   Ctrl-a ?     toggle the keybinding help overlay
//!   Ctrl-a q     quit CodeForge
//!   Ctrl-a a     send a literal Ctrl-a to the child
//! Click a pane to focus it; mouse events pass through to the child (nvim mouse
//! works). Every other key is forwarded to the focused pane's child unchanged.

mod config;
mod layout;
mod pane;
mod picker;

use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread;

use anyhow::{Context, Result};
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
use pane::Pane;
use picker::{Picker, PickerAction};

/// Messages funneled into the single-threaded event loop from producer threads.
pub enum Msg {
    /// Bytes emitted by pane `id`'s child.
    Output(usize, Vec<u8>),
    /// Bytes typed by the user, already stripped of prefix sequences.
    Input(Vec<u8>),
    /// The real terminal was resized.
    Resize,
    /// Split the focused pane along `Dir`.
    Split(Dir),
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
    /// Close the focused pane.
    ClosePane,
    /// Toggle the keybinding help overlay.
    ToggleHelp,
    /// Toggle the project picker (re-homes the current window).
    OpenPicker,
    /// Open the picker to create a new window.
    NewWindow,
    /// Switch to the next window.
    NextWindow,
    /// Switch to window `n` (0-based).
    SelectWindow(usize),
    /// User asked to quit.
    Quit,
}

/// One workspace: an editor + shell + AI layout for a single directory. The app
/// holds a list of these (tabs / "windows"); only the current one is drawn.
struct Window {
    panes: Vec<Pane>,
    layout: Layout,
    focus_id: usize,
    dir: PathBuf,
    title: String,
}

/// The last path component, for the window's status-bar label.
fn dir_title(dir: &Path) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("~")
        .to_string()
}

/// Build a new window: the configured IDE layout in `dir`, ids from `base`.
fn new_window(
    dir: PathBuf,
    cfg: &Config,
    shell: &str,
    base: usize,
    tx: &Sender<Msg>,
) -> Result<Window> {
    let (panes, layout) = spawn_ide(&dir, cfg, shell, base, tx)?;
    let title = dir_title(&dir);
    Ok(Window {
        panes,
        layout,
        focus_id: base,
        dir,
        title,
    })
}

/// Spawn the configured IDE layout (editor + shell + AI) in `project_dir`, with
/// pane ids starting at `base`. Returns the panes and the layout tree; the
/// focused pane is `base` (the editor).
fn spawn_ide(
    project_dir: &Path,
    cfg: &Config,
    shell: &str,
    base: usize,
    tx: &Sender<Msg>,
) -> Result<(Vec<Pane>, Layout)> {
    let (editor, editor_title) = command_line(&cfg.editor, project_dir);
    let (ai, ai_title) = command_line(&cfg.ai, project_dir);

    let panes = vec![
        Pane::spawn(editor, editor_title, 1, 1, base, tx.clone())?,
        Pane::spawn(
            command(shell, project_dir),
            "shell".into(),
            1,
            1,
            base + 1,
            tx.clone(),
        )?,
        Pane::spawn(ai, ai_title, 1, 1, base + 2, tx.clone())?,
    ];

    let layout = Layout::Split {
        dir: Dir::Row,
        ratio: cfg.editor_ratio,
        a: Box::new(Layout::Leaf(base)),
        b: Box::new(Layout::Split {
            dir: Dir::Col,
            ratio: cfg.right_ratio,
            a: Box::new(Layout::Leaf(base + 1)),
            b: Box::new(Layout::Leaf(base + 2)),
        }),
    };
    Ok((panes, layout))
}

/// Build a command that inherits our environment, runs in `cwd`, and advertises
/// a 256-color terminal to the child.
fn command(program: &str, cwd: &Path) -> CommandBuilder {
    let mut c = CommandBuilder::new(program);
    c.cwd(cwd);
    c.env("TERM", "xterm-256color");
    c
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
    // Load config before touching the terminal, so a parse warning can print.
    let (cfg, cfg_warning) = Config::load();
    if let Some(w) = &cfg_warning {
        eprintln!("codeforge: {w}");
    }
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

    let (cols, rows) = terminal::size().context("querying terminal size")?;

    let (tx, rx) = mpsc::channel::<Msg>();

    // Producer: SIGWINCH -> Resize.
    let mut signals = Signals::new([SIGWINCH]).context("installing SIGWINCH handler")?;
    {
        let tx = tx.clone();
        thread::spawn(move || {
            for _ in signals.forever() {
                if tx.send(Msg::Resize).is_err() {
                    break;
                }
            }
        });
    }

    // Enter the alternate screen in raw mode; restore on any exit path. Done
    // before the startup picker, which draws to the alternate screen.
    let mut out = io::stdout();
    enable_raw_mode().context("enabling raw mode")?;
    queue!(out, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;
    out.flush()?;
    let _guard = TerminalGuard;

    // Choose the project: a CLI arg, or the startup picker (bare `forge`).
    let project_dir = match std::env::args().nth(1) {
        Some(arg) => resolve_arg_dir(&arg, &proot),
        None => Picker::new(proot.clone())
            .run_blocking(&mut out)?
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))),
    };

    // Producer: raw stdin. Started AFTER the startup picker so they don't both
    // try to read stdin at once.
    spawn_stdin(tx.clone(), prefix, keys);

    // Windows: independent editor+shell+AI workspaces. Only `cur` is drawn; the
    // last terminal row is a status bar, so the layout area is `rows - 1`.
    let mut windows = vec![new_window(project_dir, &cfg, &shell, 0, &tx)?];
    let mut cur = 0usize;
    let mut next_id: usize = 3;
    let mut show_help = false;
    let mut picker: Option<Picker> = None;
    // Whether a pending picker choice opens a new window vs re-homes the current.
    let mut picker_new_window = false;

    let area = rows.saturating_sub(1);
    {
        let w = &mut windows[cur];
        relayout(&mut w.panes, &w.layout, cols, area)?;
    }
    render(
        &mut out,
        &windows,
        cur,
        cols,
        rows,
        true,
        show_help,
        picker.as_ref(),
    )?;

    while let Ok(first) = rx.recv() {
        // Drain everything currently queued and handle it as one batch, so a
        // single redraw covers a burst of output/commands. Every message is
        // handled — none is discarded (an eaten Quit would hang the app).
        let mut batch = vec![first];
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }

        let mut dirty = false;
        let mut quit = false;
        // Geometry changes (resize/split/close/window-switch) can leave stale
        // cells, so the next paint must clear first. Plain output/focus must not.
        let mut needs_clear = false;
        for msg in batch {
            match msg {
                Msg::Output(id, bytes) => {
                    // Route to whichever window owns the pane; only redraw if it
                    // is the current (visible) window.
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
                        // Picker open: keystrokes drive it, not the panes.
                        match pk.feed_bytes(&bytes) {
                            PickerAction::None => {}
                            PickerAction::Cancel => {
                                picker = None;
                                needs_clear = true;
                            }
                            PickerAction::Chosen(dir) => {
                                picker = None;
                                needs_clear = true;
                                let (c, r) = terminal::size()?;
                                let area = r.saturating_sub(1);
                                let base = next_id;
                                next_id = base + 3;
                                if picker_new_window {
                                    windows.push(new_window(dir, &cfg, &shell, base, &tx)?);
                                    cur = windows.len() - 1;
                                } else {
                                    // Re-home the current window in the new dir.
                                    for p in &mut windows[cur].panes {
                                        p.kill();
                                    }
                                    windows[cur] = new_window(dir, &cfg, &shell, base, &tx)?;
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
                Msg::Resize => {
                    let (c, r) = terminal::size()?;
                    let area = r.saturating_sub(1);
                    for w in &mut windows {
                        relayout(&mut w.panes, &w.layout, c, area)?;
                    }
                    dirty = true;
                    needs_clear = true;
                }
                Msg::Split(dir) => {
                    let (c, r) = terminal::size()?;
                    let area = r.saturating_sub(1);
                    let new_id = next_id;
                    next_id += 1;
                    let w = &mut windows[cur];
                    let pane = Pane::spawn(
                        command(&shell, &w.dir),
                        "shell".into(),
                        1,
                        1,
                        new_id,
                        tx.clone(),
                    )?;
                    w.panes.push(pane);
                    w.layout.split(w.focus_id, new_id, dir);
                    w.focus_id = new_id;
                    relayout(&mut w.panes, &w.layout, c, area)?;
                    dirty = true;
                    needs_clear = true;
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
                    let (c, r) = terminal::size()?;
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
                    let (c, r) = terminal::size()?;
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
                Msg::ClosePane => {
                    let (c, r) = terminal::size()?;
                    let area = r.saturating_sub(1);
                    let w = &mut windows[cur];
                    close_leaf(&mut w.panes, &mut w.layout, w.focus_id);
                    if !w.panes.is_empty() {
                        if let Some(id) = w.layout.first_leaf() {
                            w.focus_id = id;
                        }
                        relayout(&mut w.panes, &w.layout, c, area)?;
                    }
                    // An emptied window is dropped by the reap pass below.
                    dirty = true;
                    needs_clear = true;
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
                Msg::NextWindow => {
                    if !windows.is_empty() {
                        cur = (cur + 1) % windows.len();
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::SelectWindow(n) => {
                    if n < windows.len() && n != cur {
                        cur = n;
                        dirty = true;
                        needs_clear = true;
                    }
                }
                Msg::Quit => {
                    quit = true;
                    break;
                }
            }
        }

        if quit {
            break;
        }

        // Reap dead panes across all windows; drop any window left empty.
        let mut wi = 0;
        while wi < windows.len() {
            let mut dead = Vec::new();
            for p in windows[wi].panes.iter_mut() {
                if p.is_dead() {
                    dead.push(p.id);
                }
            }
            for id in dead {
                let w = &mut windows[wi];
                close_leaf(&mut w.panes, &mut w.layout, id);
                if wi == cur {
                    needs_clear = true;
                }
                dirty = true;
            }
            if windows[wi].panes.is_empty() {
                windows.remove(wi);
                if cur > wi || (cur == wi && cur > 0) {
                    cur -= 1;
                }
                needs_clear = true;
                dirty = true;
            } else {
                let w = &mut windows[wi];
                if !leaf_exists(&w.layout, w.focus_id) {
                    if let Some(f) = w.layout.first_leaf() {
                        w.focus_id = f;
                    }
                }
                wi += 1;
            }
        }
        if windows.is_empty() {
            break;
        }
        if cur >= windows.len() {
            cur = windows.len() - 1;
        }

        if dirty {
            let (c, r) = terminal::size()?;
            render(
                &mut out,
                &windows,
                cur,
                c,
                r,
                needs_clear,
                show_help,
                picker.as_ref(),
            )?;
        }
    }

    for w in &mut windows {
        for p in &mut w.panes {
            p.kill();
        }
    }
    Ok(())
}

/// Does `id` still appear as a leaf in the tree?
fn leaf_exists(layout: &Layout, id: usize) -> bool {
    let mut ls = Vec::new();
    layout.leaves(&mut ls);
    ls.contains(&id)
}

/// Kill and drop the pane `id`, then prune its leaf from the tree.
fn close_leaf(panes: &mut Vec<Pane>, layout: &mut Layout, id: usize) {
    if let Some(pos) = panes.iter().position(|p| p.id == id) {
        panes.remove(pos).kill();
    }
    // `Layout::remove` consumes self; swap a throwaway leaf in to move it out.
    let taken = std::mem::replace(layout, Layout::Leaf(usize::MAX));
    *layout = taken.remove(id).unwrap_or(Layout::Leaf(usize::MAX));
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

/// Read raw stdin bytes: intercept `Ctrl-a` prefix commands and SGR mouse
/// events, and forward everything else to the focused pane unchanged.
///
/// Mouse events (enabled via `EnableMouseCapture`) arrive as `ESC [ < cb;x;y M`
/// (press/drag) or `... m` (release). We pull them out so clicks can focus a
/// pane; the main loop re-forwards them, remapped, into the target child. Non-
/// mouse escape sequences (arrow keys, etc.) pass straight through.
fn spawn_stdin(tx: Sender<Msg>, prefix: u8, keys: Keys) {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buf = [0u8; 4096];
        let mut state = InState::Normal;
        let mut mouse: Vec<u8> = Vec::new();
        let mut passthrough: Vec<u8> = Vec::new();

        // Send any buffered passthrough, preserving order before a command.
        let flush = |pt: &mut Vec<u8>| -> bool {
            if pt.is_empty() {
                return true;
            }
            tx.send(Msg::Input(std::mem::take(pt))).is_ok()
        };

        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            for &b in &buf[..n] {
                match state {
                    InState::Normal => match b {
                        _ if b == prefix => state = InState::Prefix,
                        0x1b => state = InState::Esc,
                        _ => passthrough.push(b),
                    },
                    InState::Prefix => {
                        state = InState::Normal;
                        let c = b as char;
                        let cmd = if c == keys.quit {
                            Some(Msg::Quit)
                        } else if c == keys.split_row {
                            Some(Msg::Split(Dir::Row))
                        } else if c == keys.split_col {
                            Some(Msg::Split(Dir::Col))
                        } else if c == keys.cycle {
                            Some(Msg::FocusNext)
                        } else if c == keys.focus_left {
                            Some(Msg::Focus(FocusDir::Left))
                        } else if c == keys.focus_down {
                            Some(Msg::Focus(FocusDir::Down))
                        } else if c == keys.focus_up {
                            Some(Msg::Focus(FocusDir::Up))
                        } else if c == keys.focus_right {
                            Some(Msg::Focus(FocusDir::Right))
                        } else if c == keys.close {
                            Some(Msg::ClosePane)
                        } else if c == keys.help {
                            Some(Msg::ToggleHelp)
                        } else if c == keys.picker {
                            Some(Msg::OpenPicker)
                        } else if c == keys.win_new {
                            Some(Msg::NewWindow)
                        } else if c == keys.win_next {
                            Some(Msg::NextWindow)
                        } else if b.is_ascii_digit() && b != b'0' {
                            // prefix 1..9 -> jump to that window.
                            Some(Msg::SelectWindow((b - b'1') as usize))
                        } else if b == b'a' || b == prefix {
                            // prefix a / prefix prefix -> a literal prefix byte.
                            passthrough.push(prefix);
                            None
                        } else {
                            None
                        };
                        if let Some(m) = cmd {
                            if !flush(&mut passthrough) || tx.send(m).is_err() {
                                return;
                            }
                        }
                    }
                    InState::Esc => match b {
                        b'[' => state = InState::Csi,
                        // Not a CSI: forward the ESC and reinterpret this byte.
                        _ => {
                            passthrough.push(0x1b);
                            if b == prefix {
                                state = InState::Prefix;
                            } else if b == 0x1b {
                                state = InState::Esc;
                            } else {
                                passthrough.push(b);
                                state = InState::Normal;
                            }
                        }
                    },
                    InState::Csi => {
                        if b == b'<' {
                            state = InState::Mouse;
                            mouse.clear();
                        } else {
                            // Ordinary CSI (arrows, etc.): forward ESC [ then b.
                            passthrough.extend_from_slice(b"\x1b[");
                            passthrough.push(b);
                            state = InState::Normal;
                        }
                    }
                    InState::Mouse => {
                        if b == b'M' || b == b'm' {
                            state = InState::Normal;
                            if let Some(ev) = parse_mouse(&mouse, b == b'M') {
                                if !flush(&mut passthrough) || tx.send(ev).is_err() {
                                    return;
                                }
                            }
                        } else {
                            mouse.push(b);
                        }
                    }
                }
            }
            if !flush(&mut passthrough) {
                break;
            }
        }
    });
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
    out: &mut io::Stdout,
    windows: &[Window],
    cur: usize,
    cols: u16,
    rows: u16,
    clear: bool,
    show_help: bool,
    picker: Option<&Picker>,
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

    draw_status(out, cols, rows, windows, cur)?;

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
        draw_help(out, cols, rows)?;
    }
    if let Some(pk) = picker {
        pk.render(out, cols, rows)?;
    }
    out.flush()?;
    Ok(())
}

/// Draw the bottom status bar: window tabs (1:name …) with the current one
/// highlighted, plus a hint.
fn draw_status(
    out: &mut io::Stdout,
    cols: u16,
    rows: u16,
    windows: &[Window],
    cur: usize,
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
        if i == cur {
            queue!(
                out,
                SetBackgroundColor(Color::Cyan),
                SetForegroundColor(Color::Black)
            )?;
        } else {
            queue!(
                out,
                SetBackgroundColor(Color::DarkGrey),
                SetForegroundColor(Color::White)
            )?;
        }
        queue!(out, Print(format!(" {}:{} ", i + 1, w.title)))?;
    }
    queue!(
        out,
        SetBackgroundColor(Color::DarkGrey),
        SetForegroundColor(Color::Grey),
        Print("  ^a c:new  ^a n:next  ^a ?:help"),
        ResetColor,
    )?;
    Ok(())
}

/// Draw a centered overlay listing the CodeForge keybindings.
fn draw_help(out: &mut io::Stdout, cols: u16, rows: u16) -> Result<()> {
    let lines = [
        "  CodeForge — keys (prefix Ctrl-a)  ",
        "",
        "  Ctrl-a |      split side by side   ",
        "  Ctrl-a -      split top/bottom     ",
        "  Ctrl-a hjkl   move focus           ",
        "  Ctrl-a o      cycle focus          ",
        "  Ctrl-a x      close pane           ",
        "  Ctrl-a p      switch project       ",
        "  Ctrl-a c      new window           ",
        "  Ctrl-a n      next window          ",
        "  Ctrl-a 1..9   jump to window       ",
        "  Ctrl-a q      quit                 ",
        "  Ctrl-a ?      toggle this help     ",
        "  click / wheel  focus / scroll      ",
        "",
        "  edit ~/.config/codeforge/config.toml",
    ];
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
fn blit_pane(out: &mut io::Stdout, screen: &vt100::Screen, inner: Rect) -> Result<()> {
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
fn draw_border(out: &mut io::Stdout, rect: &Rect, title: &str, focused: bool) -> Result<()> {
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
