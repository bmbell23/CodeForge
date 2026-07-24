//! A single pane: one child process running in a PTY, with a vt100 emulator
//! tracking its screen. The compositor in `main.rs` owns a collection of these
//! and blits each one into its own rectangle of the real terminal.

use std::io::{Read, Write};
use std::sync::mpsc::Sender;
use std::thread;

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};

use crate::Msg;

/// Owns the PTY, the emulator, and the child for one pane.
pub struct Pane {
    /// Stable id, assigned at spawn and never reused. Used to route the child's
    /// output stream back to this pane regardless of its position in the list.
    pub id: usize,
    /// PTY master — kept so we can resize it as the pane geometry changes.
    master: Box<dyn MasterPty + Send>,
    /// Write side of the PTY: user keystrokes go here.
    writer: Box<dyn Write + Send>,
    /// The child process (shell/nvim/claude). Kept to poll for exit and kill.
    child: Box<dyn Child + Send + Sync>,
    /// vt100 emulator holding the child's current screen.
    parser: vt100::Parser,
    /// Shown in the pane's border.
    pub title: String,
}

impl Pane {
    /// Spawn `command` in a fresh PTY sized to `rows`x`cols`, streaming its
    /// output back to the event loop tagged with `id`.
    pub fn spawn(command: &str, rows: u16, cols: u16, id: usize, tx: Sender<Msg>) -> Result<Pane> {
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
            .spawn_command(CommandBuilder::new(command))
            .with_context(|| format!("spawning {command}"))?;
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
            master: pair.master,
            writer,
            child,
            parser: vt100::Parser::new(rows, cols, 0),
            title: command.to_string(),
        })
    }

    /// Feed child output into the emulator.
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
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

    /// Has the child exited?
    pub fn is_dead(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(Some(_)))
    }

    /// Kill the child (used when closing a pane).
    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

/// A PTY of zero rows/cols is invalid and vt100 panics on it; keep at least 1x1.
fn clamp_size(rows: u16, cols: u16) -> (u16, u16) {
    (rows.max(1), cols.max(1))
}
