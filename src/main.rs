//! CodeForge — terminal-native IDE.
//!
//! v0.1 vertical slice: a single-pane PTY host. It spawns a child process
//! (default `$SHELL`) inside a pseudo-terminal, runs the bytes through a vt100
//! emulator, and renders the resulting screen into the real terminal while
//! forwarding keystrokes back to the child. This is the hard core that
//! everything else (splits, tabs, an nvim/claude pane layout) builds on.
//!
//! Controls:
//!   Ctrl-a q   quit CodeForge
//!   Ctrl-a a   send a literal Ctrl-a to the child
//! Every other key is forwarded to the focused child unchanged.

use std::io::{self, Read, Write};
use std::sync::mpsc::{self, Sender};
use std::thread;

use anyhow::{Context, Result};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{cursor, queue, terminal};
use portable_pty::{native_pty_system, CommandBuilder, PtySize};
use signal_hook::consts::SIGWINCH;
use signal_hook::iterator::Signals;

/// The tmux-style prefix. Ctrl-a (0x01) begins a CodeForge command.
const PREFIX: u8 = 0x01;

/// Messages funneled into the single-threaded event loop from the various
/// producer threads (PTY reader, stdin reader, signal handler).
enum Msg {
    /// Bytes emitted by the child process.
    Output(Vec<u8>),
    /// Bytes typed by the user, already stripped of prefix sequences.
    Input(Vec<u8>),
    /// The real terminal was resized.
    Resize,
    /// User asked to quit (Ctrl-a q).
    Quit,
}

fn main() -> Result<()> {
    let (cols, rows) = terminal::size().context("querying terminal size")?;

    // Spawn the child inside a PTY sized to the current terminal.
    let pty_system = native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("opening pty")?;

    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".to_string());
    let cmd = CommandBuilder::new(&shell);
    let mut child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("spawning {shell}"))?;

    // The vt100 parser turns the child's raw byte stream into a screen grid.
    let mut parser = vt100::Parser::new(rows, cols, 0);

    let (tx, rx) = mpsc::channel::<Msg>();

    // Producer: forward the child's output into the loop.
    let mut reader = pair
        .master
        .try_clone_reader()
        .context("cloning pty reader")?;
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Msg::Output(buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
    }

    // Producer: read raw stdin, handling the Ctrl-a prefix locally.
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

    // A writer handle to send user input to the child.
    let mut writer = pair.master.take_writer().context("taking pty writer")?;

    // Enter the alternate screen in raw mode; restore on any exit path.
    let mut out = io::stdout();
    enable_raw_mode().context("enabling raw mode")?;
    queue!(out, EnterAlternateScreen, cursor::Hide)?;
    out.flush()?;
    let _guard = TerminalGuard;

    render(&mut out, parser.screen())?;

    while let Ok(msg) = rx.recv() {
        match msg {
            Msg::Output(bytes) => {
                parser.process(&bytes);
                // Coalesce any further pending output before a single redraw.
                while let Ok(next) = rx.try_recv() {
                    match next {
                        Msg::Output(more) => parser.process(&more),
                        Msg::Input(b) => {
                            writer.write_all(&b)?;
                            writer.flush()?;
                        }
                        Msg::Resize => resize(&pair, &mut parser)?,
                        Msg::Quit => return Ok(()),
                    }
                }
                render(&mut out, parser.screen())?;
            }
            Msg::Input(bytes) => {
                writer.write_all(&bytes)?;
                writer.flush()?;
            }
            Msg::Resize => {
                resize(&pair, &mut parser)?;
                render(&mut out, parser.screen())?;
            }
            Msg::Quit => break,
        }

        // If the child exited, leave.
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
    }

    Ok(())
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
                    match b {
                        b'q' => {
                            let _ = tx.send(Msg::Quit);
                            return;
                        }
                        // Ctrl-a a -> literal Ctrl-a to the child.
                        b'a' | PREFIX => passthrough.push(PREFIX),
                        // Unknown command: swallow it (no-op).
                        _ => {}
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

/// Resize the PTY and the emulator to match the real terminal.
fn resize(pair: &portable_pty::PtyPair, parser: &mut vt100::Parser) -> Result<()> {
    let (cols, rows) = terminal::size()?;
    pair.master.resize(PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    })?;
    parser.set_size(rows, cols);
    Ok(())
}

/// Paint the emulator screen into the real terminal.
///
/// `contents_formatted` returns escape sequences that reproduce the whole
/// screen from a clean state, so a clear + write is enough for the v0.1 slice.
/// (Diff-based rendering to kill flicker is a follow-up.)
fn render(out: &mut io::Stdout, screen: &vt100::Screen) -> Result<()> {
    queue!(
        out,
        cursor::Hide,
        cursor::MoveTo(0, 0),
        terminal::Clear(terminal::ClearType::All)
    )?;
    out.write_all(&screen.contents_formatted())?;
    let (row, col) = screen.cursor_position();
    queue!(out, cursor::MoveTo(col, row))?;
    if !screen.hide_cursor() {
        queue!(out, cursor::Show)?;
    }
    out.flush()?;
    Ok(())
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
