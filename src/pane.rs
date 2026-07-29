//! A single pane: one child process running in a PTY, with a vt100 emulator
//! tracking its screen. The compositor in `main.rs` owns a collection of these
//! and blits each one into its own rectangle of the real terminal.

use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::Msg;

/// What a pane runs, for session capture/restore.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum PaneRole {
    Editor,
    Shell,
    Ai,
}

/// Owns the PTY, the emulator, and the child for one pane.
pub struct Pane {
    /// Stable id, assigned at spawn and never reused. Used to route the child's
    /// output stream back to this pane regardless of its position in the list.
    pub id: usize,
    /// What this pane runs (editor / shell / AI).
    pub role: PaneRole,
    /// PTY master — kept so we can resize it as the pane geometry changes.
    master: Box<dyn MasterPty + Send>,
    /// Write side of the PTY: user keystrokes go here.
    writer: Box<dyn Write + Send>,
    /// The child process (shell/nvim/claude). Kept to poll for exit and kill.
    child: Box<dyn Child + Send + Sync>,
    /// vt100 emulator holding the child's current screen.
    parser: vt100::Parser,
    /// How many rows we're scrolled back into history (0 = live bottom).
    scroll: usize,
    /// Copy-mode freeze: while true, incoming output is buffered (not fed to the
    /// emulator) so the scrolled view stays put and vt100 can't panic feeding a
    /// scrolled grid. Applied on `unfreeze`.
    frozen: bool,
    pending: Vec<u8>,
    /// When this child was spawned (to distinguish a quick crash from a normal
    /// exit when deciding whether to respawn).
    spawned_at: Instant,
    /// Consecutive fast respawns, to break a crash loop.
    respawns: u32,
    /// Shown in the pane's border.
    pub title: String,
}

/// Scrollback history kept per pane, in rows.
const SCROLLBACK: usize = 10_000;

impl Pane {
    /// Spawn `cmd` in a fresh PTY sized to `rows`x`cols`, streaming its output
    /// back to the event loop tagged with `id`. `title` is shown in the border.
    pub fn spawn(
        cmd: CommandBuilder,
        title: String,
        role: PaneRole,
        rows: u16,
        cols: u16,
        id: usize,
        tx: Sender<Msg>,
    ) -> Result<Pane> {
        let (rows, cols) = clamp_size(rows, cols);
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening pty")?;

        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("spawning pane {id}"))?;
        // Slave is dropped here; the master keeps the PTY open.

        let writer = pair.master.take_writer().context("taking pty writer")?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("cloning pty reader")?;

        thread::spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(Msg::Output(id, buf[..n].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(Pane {
            id,
            role,
            master: pair.master,
            writer,
            child,
            parser: vt100::Parser::new(rows, cols, SCROLLBACK),
            scroll: 0,
            frozen: false,
            pending: Vec::new(),
            spawned_at: Instant::now(),
            respawns: 0,
            title,
        })
    }

    /// The child's process id, if known (used to read its cwd via /proc).
    pub fn pid(&self) -> Option<u32> {
        self.child.process_id()
    }

    /// How long this child has been alive.
    pub fn age(&self) -> Duration {
        self.spawned_at.elapsed()
    }

    /// Consecutive fast respawns (crash-loop guard).
    pub fn respawns(&self) -> u32 {
        self.respawns
    }

    /// Carry the respawn counter onto a freshly respawned pane.
    pub fn set_respawns(&mut self, n: u32) {
        self.respawns = n;
    }

    /// Feed child output into the emulator. While frozen (copy mode) the bytes
    /// are buffered instead. Otherwise, if we were scrolled back via the wheel,
    /// new output snaps to the live bottom first — both the natural behavior and
    /// a guard against a vt100 panic when processing into a scrolled grid.
    ///
    /// vt100 can also panic on other internal edge cases (e.g. wrapping a wide
    /// glyph in a degenerate grid); contain it so a single pane's byte stream
    /// can never take down the whole server.
    pub fn feed(&mut self, bytes: &[u8]) {
        if self.frozen {
            self.pending.extend_from_slice(bytes);
            return;
        }
        if self.scroll != 0 {
            self.scroll = 0;
            self.parser.set_scrollback(0);
        }
        let parser = &mut self.parser;
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser.process(bytes)));
    }

    /// Freeze the emulator (enter copy mode): buffer incoming output.
    pub fn freeze(&mut self) {
        self.frozen = true;
    }

    /// Unfreeze (leave copy mode): drop the scroll, then apply buffered output.
    pub fn unfreeze(&mut self) {
        self.frozen = false;
        self.scroll = 0;
        self.parser.set_scrollback(0);
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() {
            let parser = &mut self.parser;
            let _ =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| parser.process(&pending)));
        }
    }

    /// Whether the child is on the alternate screen (a fullscreen TUI like
    /// Claude/vim). Such panes have no meaningful line scrollback.
    pub fn alternate_screen(&self) -> bool {
        self.parser.screen().alternate_screen()
    }

    /// Forward user input to the child.
    pub fn write_input(&mut self, bytes: &[u8]) -> Result<()> {
        self.writer.write_all(bytes)?;
        self.writer.flush()?;
        Ok(())
    }

    /// Resize the PTY and emulator to a new content area.
    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        let (rows, cols) = clamp_size(rows, cols);
        self.master.resize(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        self.parser.set_size(rows, cols);
        Ok(())
    }

    /// The emulator screen, for rendering.
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }

    /// Cumulative count of audible bells (BEL) the child has emitted. Claude
    /// rings the bell when it wants input, so a rising count means "attention".
    pub fn bell_count(&self) -> usize {
        self.parser.screen().audible_bell_count()
    }

    /// Whether the child has enabled a mouse reporting mode. Forwarding mouse
    /// events to a child that hasn't (e.g. a bare shell) just prints junk.
    /// The child's current mouse-reporting mode, so forge only forwards the
    /// event classes the app actually asked for (motion vs click), and never
    /// leaks raw sequences into a pane in click-only / stale mode (#73).
    pub fn mouse_mode(&self) -> vt100::MouseProtocolMode {
        self.parser.screen().mouse_protocol_mode()
    }

    /// Scroll the view by `delta` rows into history (positive = back in time).
    /// Used for mouse-wheel scrollback in panes whose child doesn't handle the
    /// mouse itself (e.g. a shell).
    pub fn scroll(&mut self, delta: i32) {
        let next = (self.scroll as i32 + delta).max(0) as usize;
        self.scroll = next.min(SCROLLBACK);
        self.parser.set_scrollback(self.scroll);
    }

    /// Jump back to the live bottom of the buffer (e.g. when the user types).
    pub fn scroll_to_bottom(&mut self) {
        if self.scroll != 0 {
            self.scroll = 0;
            self.parser.set_scrollback(0);
        }
    }

    /// Has the child exited?
    pub fn is_dead(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Kill the child (used when closing a pane).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

/// vt100 panics on a 0-sized grid, and its wrap/scroll math underflows on a
/// 1-row or 1-col grid (grid.rs col_wrap), so keep every emulator at least 2x2.
/// Panes drawn smaller than that have no inner area and aren't blitted anyway.
fn clamp_size(rows: u16, cols: u16) -> (u16, u16) {
    (rows.max(2), cols.max(2))
}
