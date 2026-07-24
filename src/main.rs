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
//!   Ctrl-a q     quit CodeForge
//!   Ctrl-a a     send a literal Ctrl-a to the child
//! Click a pane to focus it; mouse events pass through to the child (nvim mouse
//! works). Every other key is forwarded to the focused pane's child unchanged.

mod layout;
mod pane;

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

use layout::{Dir, FocusDir, Layout, Rect};
use pane::Pane;

/// The tmux-style prefix. Ctrl-a (0x01) begins a CodeForge command.
const PREFIX: u8 = 0x01;

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
    /// User asked to quit.
    Quit,
}

/// Build a command that inherits our environment, runs in `cwd`, and advertises
/// a 256-color terminal to the child.
fn command(program: &str, cwd: &Path) -> CommandBuilder {
    let mut c = CommandBuilder::new(program);
    c.cwd(cwd);
    c.env("TERM", "xterm-256color");
    c
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

/// Resolve the project directory to open: the first CLI arg (an absolute/relative
/// path, or a bare name resolved under the projects root), else the current dir.
fn resolve_project_dir() -> PathBuf {
    if let Some(arg) = std::env::args().nth(1) {
        let p = PathBuf::from(&arg);
        if p.is_dir() {
            return p.canonicalize().unwrap_or(p);
        }
        let under = projects_root().join(&arg);
        if under.is_dir() {
            return under;
        }
        return p; // Let the child surface the error (e.g. nvim on a new path).
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn main() -> Result<()> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let (cols, rows) = terminal::size().context("querying terminal size")?;

    let (tx, rx) = mpsc::channel::<Msg>();

    // Producer: raw stdin, with the Ctrl-a prefix handled locally.
    spawn_stdin(tx.clone());

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

    // Default IDE layout: nvim on the left, a shell and Claude stacked on the
    // right. Real sizes are set by `relayout` below; spawn at 1x1 and let the
    // children reflow via SIGWINCH.
    //
    //   ┌────────────┬──────────┐
    //   │            │  shell   │
    //   │    nvim    ├──────────┤
    //   │            │  claude  │
    //   └────────────┴──────────┘
    // All panes open in the selected project directory.
    let project_dir = resolve_project_dir();

    let mut nvim = command("nvim", &project_dir);
    nvim.arg(".");
    // Isolate our editor config under ~/.config/codeforge (see config/nvim/).
    nvim.env("NVIM_APPNAME", "codeforge");

    let mut panes: Vec<Pane> = Vec::new();
    panes.push(Pane::spawn(nvim, "nvim".into(), 1, 1, 0, tx.clone())?);
    panes.push(Pane::spawn(
        command(&shell, &project_dir),
        "shell".into(),
        1,
        1,
        1,
        tx.clone(),
    )?);
    panes.push(Pane::spawn(
        command("claude", &project_dir),
        "claude".into(),
        1,
        1,
        2,
        tx.clone(),
    )?);

    let mut layout = Layout::Split {
        dir: Dir::Row,
        ratio: 0.68,
        a: Box::new(Layout::Leaf(0)),
        b: Box::new(Layout::Split {
            dir: Dir::Col,
            ratio: 0.55,
            a: Box::new(Layout::Leaf(1)),
            b: Box::new(Layout::Leaf(2)),
        }),
    };
    let mut focus_id: usize = 0;
    let mut next_id: usize = 3;

    // Enter the alternate screen in raw mode; restore on any exit path.
    let mut out = io::stdout();
    enable_raw_mode().context("enabling raw mode")?;
    queue!(out, EnterAlternateScreen, EnableMouseCapture, cursor::Hide)?;
    out.flush()?;
    let _guard = TerminalGuard;

    relayout(&mut panes, &layout, cols, rows)?;
    render(&mut out, &panes, &layout, focus_id, cols, rows)?;

    while let Ok(first) = rx.recv() {
        // Drain everything currently queued and handle it as one batch, so a
        // single redraw covers a burst of output/commands. Every message is
        // handled — none is discarded — which is why we collect rather than
        // pattern-match the drain (an eaten Quit would hang the app).
        let mut batch = vec![first];
        while let Ok(m) = rx.try_recv() {
            batch.push(m);
        }

        let mut dirty = false;
        let mut quit = false;
        for msg in batch {
            match msg {
                Msg::Output(id, bytes) => {
                    if let Some(p) = pane_by_id(&mut panes, id) {
                        p.feed(&bytes);
                    }
                    dirty = true;
                }
                Msg::Input(bytes) => {
                    if let Some(p) = pane_by_id(&mut panes, focus_id) {
                        p.write_input(&bytes)?;
                    }
                }
                Msg::Resize => {
                    let (c, r) = terminal::size()?;
                    relayout(&mut panes, &layout, c, r)?;
                    dirty = true;
                }
                Msg::Split(dir) => {
                    let (c, r) = terminal::size()?;
                    let new_id = next_id;
                    next_id += 1;
                    panes.push(Pane::spawn(
                        command(&shell, &project_dir),
                        "shell".into(),
                        1,
                        1,
                        new_id,
                        tx.clone(),
                    )?);
                    layout.split(focus_id, new_id, dir);
                    focus_id = new_id;
                    relayout(&mut panes, &layout, c, r)?;
                    dirty = true;
                }
                Msg::FocusNext => {
                    let mut ls = Vec::new();
                    layout.leaves(&mut ls);
                    if let Some(pos) = ls.iter().position(|&x| x == focus_id) {
                        focus_id = ls[(pos + 1) % ls.len()];
                    }
                    dirty = true;
                }
                Msg::Mouse { x, y, cb, press } => {
                    let (c, r) = terminal::size()?;
                    let mut rects = Vec::new();
                    layout.rects(
                        Rect {
                            x: 0,
                            y: 0,
                            w: c,
                            h: r,
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
                        // A press focuses the pane under the cursor.
                        if press && focus_id != id {
                            focus_id = id;
                            dirty = true;
                        }
                        // Forward the event to the pane, remapped to its content
                        // origin, so the child (e.g. nvim) sees the click locally.
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
                                if let Some(p) = pane_by_id(&mut panes, id) {
                                    p.write_input(seq.as_bytes())?;
                                }
                            }
                        }
                    }
                }
                Msg::Focus(dir) => {
                    let (c, r) = terminal::size()?;
                    let mut rects = Vec::new();
                    layout.rects(
                        Rect {
                            x: 0,
                            y: 0,
                            w: c,
                            h: r,
                        },
                        &mut rects,
                    );
                    if let Some(id) = layout::neighbor(&rects, focus_id, dir) {
                        focus_id = id;
                    }
                    dirty = true;
                }
                Msg::ClosePane => {
                    close_leaf(&mut panes, &mut layout, focus_id);
                    match layout.first_leaf() {
                        Some(id) if !panes.is_empty() => {
                            focus_id = id;
                            let (c, r) = terminal::size()?;
                            relayout(&mut panes, &layout, c, r)?;
                            dirty = true;
                        }
                        _ => {
                            quit = true;
                            break;
                        }
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

        // Reap panes whose child exited on its own.
        let mut dead = Vec::new();
        for p in panes.iter_mut() {
            if p.is_dead() {
                dead.push(p.id);
            }
        }
        for id in dead {
            close_leaf(&mut panes, &mut layout, id);
            dirty = true;
        }
        match layout.first_leaf() {
            Some(_) if panes.is_empty() => break,
            None => break,
            Some(id) => {
                if !leaf_exists(&layout, focus_id) {
                    focus_id = id;
                }
            }
        }

        if dirty {
            let (c, r) = terminal::size()?;
            render(&mut out, &panes, &layout, focus_id, c, r)?;
        }
    }

    for p in &mut panes {
        p.kill();
    }
    Ok(())
}

/// Resolve a stable pane id to its current slot.
fn pane_by_id(panes: &mut [Pane], id: usize) -> Option<&mut Pane> {
    panes.iter_mut().find(|p| p.id == id)
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
fn spawn_stdin(tx: Sender<Msg>) {
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
                        PREFIX => state = InState::Prefix,
                        0x1b => state = InState::Esc,
                        _ => passthrough.push(b),
                    },
                    InState::Prefix => {
                        state = InState::Normal;
                        let cmd = match b {
                            b'q' => Some(Msg::Quit),
                            b'|' => Some(Msg::Split(Dir::Row)),
                            b'-' => Some(Msg::Split(Dir::Col)),
                            b'o' => Some(Msg::FocusNext),
                            b'h' => Some(Msg::Focus(FocusDir::Left)),
                            b'j' => Some(Msg::Focus(FocusDir::Down)),
                            b'k' => Some(Msg::Focus(FocusDir::Up)),
                            b'l' => Some(Msg::Focus(FocusDir::Right)),
                            b'x' => Some(Msg::ClosePane),
                            // Ctrl-a a -> literal Ctrl-a to the child.
                            b'a' | PREFIX => {
                                passthrough.push(PREFIX);
                                None
                            }
                            _ => None,
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
                            match b {
                                PREFIX => state = InState::Prefix,
                                0x1b => state = InState::Esc,
                                _ => {
                                    passthrough.push(b);
                                    state = InState::Normal;
                                }
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

/// Paint every pane into its rectangle, then place the hardware cursor in the
/// focused pane. Rendering is per-cell so panes can be tiled.
fn render(
    out: &mut io::Stdout,
    panes: &[Pane],
    layout: &Layout,
    focus_id: usize,
    cols: u16,
    rows: u16,
) -> Result<()> {
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
    queue!(
        out,
        cursor::Hide,
        ResetColor,
        terminal::Clear(terminal::ClearType::All)
    )?;

    for (id, rect) in &rects {
        if let Some(p) = panes.iter().find(|p| p.id == *id) {
            draw_border(out, rect, &p.title, *id == focus_id)?;
            if let Some(inner) = rect.inner() {
                blit_pane(out, p.screen(), inner)?;
            }
        }
    }

    // Position the real cursor inside the focused pane.
    if let Some((_, rect)) = rects.iter().find(|(id, _)| *id == focus_id) {
        if let (Some(inner), Some(p)) = (rect.inner(), panes.iter().find(|p| p.id == focus_id)) {
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
    out.flush()?;
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
