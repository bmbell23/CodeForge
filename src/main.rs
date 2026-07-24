//! CodeForge — terminal-native IDE.
//!
//! Multi-pane compositor (#1). CodeForge hosts N child processes, each in its
//! own PTY + vt100 emulator (see `pane.rs`), and tiles them into columns of the
//! real terminal with borders. Input is routed to the focused pane; the focused
//! border is highlighted. Splits with real direction (#2) and directional focus
//! (#3) build on this layout; for now panes are equal-width columns and the
//! prefix keys below add/close/cycle them.
//!
//! Controls (Ctrl-a is the prefix):
//!   Ctrl-a q   quit CodeForge
//!   Ctrl-a c   new pane (spawns $SHELL)          [temporary until splits, #2]
//!   Ctrl-a o   cycle focus to the next pane      [temporary until focus, #3]
//!   Ctrl-a x   close the focused pane
//!   Ctrl-a a   send a literal Ctrl-a to the child
//! Every other key is forwarded to the focused pane's child unchanged.

mod pane;

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Sender};
use std::thread;

use anyhow::{Context, Result};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, queue, terminal};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;

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
    /// Spawn a new pane.
    NewPane,
    /// Move focus to the next pane.
    FocusNext,
    /// Close the focused pane.
    ClosePane,
    /// User asked to quit.
    Quit,
}

/// A rectangle in terminal cells, borders included.
#[derive(Clone, Copy)]
struct Rect {
    x: u16,
    y: u16,
    w: u16,
    h: u16,
}

impl Rect {
    /// The drawable content area inside the 1-cell border, if any remains.
    fn inner(&self) -> Option<Rect> {
        if self.w < 3 || self.h < 3 {
            return None;
        }
        Some(Rect {
            x: self.x + 1,
            y: self.y + 1,
            w: self.w - 2,
            h: self.h - 2,
        })
    }
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

    // Start with a single pane; its geometry is fixed up on the first render.
    let mut panes: Vec<Pane> = Vec::new();
    let mut next_id: usize = 0;
    let mut focus: usize = 0;

    let rects = layout_rects(cols, rows, 1);
    let inner = rects[0].inner().unwrap_or(Rect {
        x: 0,
        y: 0,
        w: cols,
        h: rows,
    });
    panes.push(Pane::spawn(&shell, inner.h, inner.w, next_id, tx.clone())?);
    next_id += 1;

    // Enter the alternate screen in raw mode; restore on any exit path.
    let mut out = io::stdout();
    enable_raw_mode().context("enabling raw mode")?;
    queue!(out, EnterAlternateScreen, cursor::Hide)?;
    out.flush()?;
    let _guard = TerminalGuard;

    apply_layout(&mut panes, cols, rows)?;
    render(&mut out, &panes, focus, cols, rows)?;

    while let Ok(msg) = rx.recv() {
        let mut dirty = true;
        match msg {
            Msg::Output(id, bytes) => {
                if let Some(p) = pane_by_id(&mut panes, id) {
                    p.feed(&bytes);
                }
                // Coalesce any further pending output before one redraw.
                while let Ok(Msg::Output(id2, more)) = rx.try_recv() {
                    if let Some(p) = pane_by_id(&mut panes, id2) {
                        p.feed(&more);
                    }
                }
            }
            Msg::Input(bytes) => {
                if let Some(p) = panes.get_mut(focus) {
                    p.write_input(&bytes)?;
                }
                dirty = false;
            }
            Msg::Resize => {
                let (c, r) = terminal::size()?;
                apply_layout(&mut panes, c, r)?;
            }
            Msg::NewPane => {
                let (c, r) = terminal::size()?;
                panes.push(Pane::spawn(&shell, 1, 1, next_id, tx.clone())?);
                next_id += 1;
                focus = panes.len() - 1;
                apply_layout(&mut panes, c, r)?;
            }
            Msg::FocusNext => {
                if !panes.is_empty() {
                    focus = (focus + 1) % panes.len();
                }
            }
            Msg::ClosePane => {
                if let Some(mut p) = take_pane(&mut panes, focus) {
                    p.kill();
                }
                if panes.is_empty() {
                    break;
                }
                focus = focus.min(panes.len() - 1);
                let (c, r) = terminal::size()?;
                apply_layout(&mut panes, c, r)?;
            }
            Msg::Quit => break,
        }

        // Reap any panes whose child exited on its own.
        reap_dead(&mut panes, &mut focus);
        if panes.is_empty() {
            break;
        }

        if dirty {
            let (c, r) = terminal::size()?;
            render(&mut out, &panes, focus, c, r)?;
        }
    }

    for p in &mut panes {
        p.kill();
    }
    Ok(())
}

/// Resolve a stable pane id to its current slot (indices shift when panes close).
fn pane_by_id(panes: &mut [Pane], id: usize) -> Option<&mut Pane> {
    panes.iter_mut().find(|p| p.id == id)
}

/// Remove and return the pane at `idx`, if present.
fn take_pane(panes: &mut Vec<Pane>, idx: usize) -> Option<Pane> {
    if idx < panes.len() {
        Some(panes.remove(idx))
    } else {
        None
    }
}

/// Drop panes whose child has exited, fixing up the focus index.
fn reap_dead(panes: &mut Vec<Pane>, focus: &mut usize) {
    let mut i = 0;
    while i < panes.len() {
        if panes[i].is_dead() {
            panes.remove(i);
            if *focus >= i && *focus > 0 {
                *focus -= 1;
            }
        } else {
            i += 1;
        }
    }
    if !panes.is_empty() {
        *focus = (*focus).min(panes.len() - 1);
    }
}

/// Recompute geometry for the current pane count and resize every PTY.
fn apply_layout(panes: &mut [Pane], cols: u16, rows: u16) -> Result<()> {
    let rects = layout_rects(cols, rows, panes.len());
    for (p, rect) in panes.iter_mut().zip(rects.iter()) {
        if let Some(inner) = rect.inner() {
            p.resize(inner.h, inner.w)?;
        }
    }
    Ok(())
}

/// Tile `n` panes as equal-width columns spanning the whole terminal.
///
/// #2 replaces this with a real layout tree (horizontal *and* vertical splits);
/// columns are enough to prove the compositor.
fn layout_rects(cols: u16, rows: u16, n: usize) -> Vec<Rect> {
    if n == 0 {
        return Vec::new();
    }
    let n_u16 = n as u16;
    let base = cols / n_u16;
    let extra = cols % n_u16;
    let mut rects = Vec::with_capacity(n);
    let mut x = 0u16;
    for i in 0..n_u16 {
        // Distribute the remainder across the leftmost columns.
        let w = base + if i < extra { 1 } else { 0 };
        rects.push(Rect {
            x,
            y: 0,
            w,
            h: rows,
        });
        x += w;
    }
    rects
}

/// Read raw stdin bytes, peel off `Ctrl-a` prefix commands, forward the rest.
fn spawn_stdin(tx: Sender<Msg>) {
    thread::spawn(move || {
        let mut stdin = io::stdin();
        let mut buf = [0u8; 4096];
        let mut prefixed = false;
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut passthrough: Vec<u8> = Vec::with_capacity(n);
            for &b in &buf[..n] {
                if prefixed {
                    prefixed = false;
                    let cmd = match b {
                        b'q' => Some(Msg::Quit),
                        b'c' => Some(Msg::NewPane),
                        b'o' => Some(Msg::FocusNext),
                        b'x' => Some(Msg::ClosePane),
                        // Ctrl-a a -> literal Ctrl-a to the child.
                        b'a' | PREFIX => {
                            passthrough.push(PREFIX);
                            None
                        }
                        // Unknown command: swallow it.
                        _ => None,
                    };
                    if let Some(m) = cmd {
                        // Flush pending passthrough before the command.
                        if !passthrough.is_empty() {
                            let _ = tx.send(Msg::Input(std::mem::take(&mut passthrough)));
                        }
                        if tx.send(m).is_err() {
                            return;
                        }
                    }
                } else if b == PREFIX {
                    prefixed = true;
                } else {
                    passthrough.push(b);
                }
            }
            if !passthrough.is_empty() && tx.send(Msg::Input(passthrough)).is_err() {
                break;
            }
        }
    });
}

/// Paint every pane into its rectangle, then place the hardware cursor in the
/// focused pane. Rendering is per-cell so panes can be tiled (vt100's
/// whole-screen `contents_formatted` only works for a full-terminal single pane).
fn render(out: &mut io::Stdout, panes: &[Pane], focus: usize, cols: u16, rows: u16) -> Result<()> {
    let rects = layout_rects(cols, rows, panes.len());
    queue!(
        out,
        cursor::Hide,
        ResetColor,
        terminal::Clear(terminal::ClearType::All)
    )?;

    for (i, (p, rect)) in panes.iter().zip(rects.iter()).enumerate() {
        draw_border(out, rect, &p.title, i == focus)?;
        if let Some(inner) = rect.inner() {
            blit_pane(out, p.screen(), inner)?;
        }
    }

    // Position the real cursor inside the focused pane.
    if let (Some(p), Some(rect)) = (panes.get(focus), rects.get(focus)) {
        if let Some(inner) = rect.inner() {
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
        let _ = queue!(out, cursor::Show, LeaveAlternateScreen);
        let _ = out.flush();
        let _ = disable_raw_mode();
    }
}
