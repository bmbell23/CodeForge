//! The new-worktree form (#50): a multi-field overlay reached from the project
//! picker's "＋ New worktree" entry. Fields, in tab order:
//!   clone (fuzzy-select a git repo under the projects root) ->
//!   [also make the auto worktree] (only for sfaos) ->
//!   ticket (prefilled "SFAP-", editable) ->
//!   short name (spaces become underscores as you type) ->
//!   upstream (shown as `origin/<branch>`, default master, optional).
//! Driven by `feed_bytes` like the picker, since the main loop owns stdin.

use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::{cursor, queue};

use crate::worktree::WorktreeSpec;

/// Max clone rows shown while selecting.
const MAX_ROWS: usize = 8;

/// What a fed keystroke resulted in.
pub enum WtformAction {
    None,
    Cancel,
    /// The form was submitted; create this worktree.
    Submit(WorktreeSpec),
}

#[derive(Clone, Copy, PartialEq)]
enum Field {
    Clone,
    AlsoAuto,
    Ticket,
    Name,
    Upstream,
}

pub struct WorktreeForm {
    clones: Vec<String>,
    // Clone selector state.
    filter: String,
    matches: Vec<usize>,
    sel: usize,
    clone: Option<String>,
    // Remaining fields.
    also_auto: bool,
    ticket: String,
    name: String,
    upstream: String,
    focus: Field,
    error: Option<String>,
    /// A create worker is running; the form stays up showing progress until
    /// its `Msg::WorktreeResult` arrives.
    creating: bool,
    /// Latest status line from the worker, and a spinner frame that advances
    /// with each update so it reads as live.
    progress: Option<String>,
    spin: usize,
    /// Escape-parse state (0 none, 1 saw ESC, 2 saw ESC[).
    esc: u8,
}

impl WorktreeForm {
    pub fn new(root: PathBuf) -> WorktreeForm {
        let mut clones = list_clones(&root);
        clones.sort_by_key(|s| s.to_lowercase());
        let mut f = WorktreeForm {
            clones,
            filter: String::new(),
            matches: Vec::new(),
            sel: 0,
            clone: None,
            also_auto: false,
            ticket: String::new(),
            name: String::new(),
            upstream: String::new(),
            focus: Field::Clone,
            error: None,
            creating: false,
            progress: None,
            spin: 0,
            esc: 0,
        };
        f.refilter();
        f
    }

    /// Mark a create worker as launched (form stays up, input frozen).
    pub fn begin_create(&mut self) {
        self.creating = true;
        self.error = None;
        self.progress = Some("starting…".into());
        self.spin = 0;
    }

    /// Update the live status from the worker (advances the spinner).
    pub fn set_progress(&mut self, msg: String) {
        self.progress = Some(msg);
        self.spin = self.spin.wrapping_add(1);
    }

    /// A create worker failed; re-enable editing and show the error.
    pub fn fail(&mut self, msg: String) {
        self.creating = false;
        self.error = Some(msg);
    }

    fn refilter(&mut self) {
        let q = self.filter.to_lowercase();
        self.matches = self
            .clones
            .iter()
            .enumerate()
            .filter(|(_, n)| n.to_lowercase().contains(&q))
            .map(|(i, _)| i)
            .collect();
        if self.sel >= self.matches.len() {
            self.sel = self.matches.len().saturating_sub(1);
        }
    }

    fn selected_clone(&self) -> Option<String> {
        self.matches.get(self.sel).map(|&i| self.clones[i].clone())
    }

    /// The active tab order — the AlsoAuto row exists only for sfaos.
    fn fields(&self) -> Vec<Field> {
        let mut v = vec![Field::Clone];
        if self.clone.as_deref() == Some("sfaos") {
            v.push(Field::AlsoAuto);
        }
        v.extend([Field::Ticket, Field::Name, Field::Upstream]);
        v
    }

    /// Confirm the highlighted clone and move on. Prefills the ticket prefix.
    fn confirm_clone(&mut self) {
        if let Some(c) = self.selected_clone() {
            self.clone = Some(c);
            if self.ticket.is_empty() {
                self.ticket = "SFAP-".to_string();
            }
            self.focus = if self.clone.as_deref() == Some("sfaos") {
                Field::AlsoAuto
            } else {
                Field::Ticket
            };
        }
    }

    fn next_field(&mut self) {
        if self.focus == Field::Clone {
            self.confirm_clone();
            return;
        }
        let fs = self.fields();
        if let Some(i) = fs.iter().position(|f| *f == self.focus) {
            if i + 1 < fs.len() {
                self.focus = fs[i + 1];
            }
        }
    }

    fn prev_field(&mut self) {
        let fs = self.fields();
        if let Some(i) = fs.iter().position(|f| *f == self.focus) {
            if i > 0 {
                self.focus = fs[i - 1];
            }
        }
    }

    /// Build and validate the spec, or set `error` and stay open.
    fn submit(&mut self) -> WtformAction {
        let Some(clone) = self.clone.clone() else {
            self.error = Some("choose a clone".into());
            return WtformAction::None;
        };
        if self.name.trim().is_empty() {
            self.error = Some("a short name is required".into());
            self.focus = Field::Name;
            return WtformAction::None;
        }
        WtformAction::Submit(WorktreeSpec {
            clone,
            ticket: self.ticket.clone(),
            description: self.name.clone(),
            upstream: self.upstream.clone(),
            also_auto: self.also_auto,
        })
    }

    fn edit_field(&mut self) -> &mut String {
        match self.focus {
            Field::Ticket => &mut self.ticket,
            Field::Name => &mut self.name,
            _ => &mut self.upstream,
        }
    }

    fn type_char(&mut self, c: char) {
        self.error = None;
        match self.focus {
            Field::Clone => {
                self.filter.push(c);
                self.refilter();
            }
            Field::AlsoAuto => {
                if c == ' ' {
                    self.also_auto = !self.also_auto;
                }
            }
            Field::Name => {
                // Spaces become underscores as you type (#50).
                self.edit_field().push(if c == ' ' { '_' } else { c });
            }
            _ => self.edit_field().push(c),
        }
    }

    fn backspace(&mut self) {
        self.error = None;
        match self.focus {
            Field::Clone => {
                self.filter.pop();
                self.refilter();
            }
            Field::AlsoAuto => {}
            _ => {
                self.edit_field().pop();
            }
        }
    }

    /// Drive the form from raw input bytes.
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> WtformAction {
        // While a create worker runs, only Esc / Ctrl-c (cancel the overlay)
        // are honored; everything else is frozen until the result arrives.
        if self.creating {
            for &b in bytes {
                if b == 0x1b || b == 0x03 {
                    return WtformAction::Cancel;
                }
            }
            return WtformAction::None;
        }
        for &b in bytes {
            match self.esc {
                0 => match b {
                    0x1b => self.esc = 1,
                    0x03 => return WtformAction::Cancel, // Ctrl-c
                    b'\t' => self.next_field(),
                    b'\r' | b'\n' => {
                        // Enter advances; on the last field it submits.
                        if self.focus == *self.fields().last().unwrap() {
                            let a = self.submit();
                            if !matches!(a, WtformAction::None) {
                                return a;
                            }
                        } else {
                            self.next_field();
                        }
                    }
                    0x7f | 0x08 => self.backspace(),
                    0x20..=0x7e => self.type_char(b as char),
                    _ => {}
                },
                1 => {
                    if b == b'[' {
                        self.esc = 2;
                    } else {
                        self.esc = 0;
                        return WtformAction::Cancel; // lone Esc
                    }
                }
                _ => {
                    self.esc = 0;
                    match b {
                        b'A' => {
                            if self.focus == Field::Clone {
                                self.sel = self.sel.saturating_sub(1);
                            }
                        }
                        b'B' => {
                            if self.focus == Field::Clone && self.sel + 1 < self.matches.len() {
                                self.sel += 1;
                            }
                        }
                        b'Z' => self.prev_field(), // Shift-Tab
                        _ => {}
                    }
                }
            }
        }
        if self.esc == 1 {
            self.esc = 0;
            return WtformAction::Cancel;
        }
        WtformAction::None
    }

    /// Draw the form centered on the screen.
    pub fn render(&self, out: &mut Vec<u8>, cols: u16, rows: u16) -> Result<()> {
        let w: u16 = 54.min(cols.saturating_sub(2)).max(20);
        let inner_w = (w - 2) as usize;
        let is_sfaos = self.clone.as_deref() == Some("sfaos");
        let clone_rows = if self.focus == Field::Clone {
            self.matches.len().clamp(1, MAX_ROWS)
        } else {
            0
        };
        // title + clone line (+list) + [auto] + ticket + name + upstream + hint.
        let body = 1 + clone_rows + if is_sfaos { 1 } else { 0 } + 3 + 1 + 1;
        let h = body as u16 + 2;
        if w + 2 > cols || h + 2 > rows {
            return Ok(());
        }
        let x = (cols - w) / 2;
        let y = (rows - h) / 2;

        let line = |out: &mut Vec<u8>, ry: u16, text: &str| -> Result<()> {
            let t: String = text.chars().take(inner_w).collect();
            let pad = inner_w.saturating_sub(t.chars().count());
            queue!(
                out,
                cursor::MoveTo(x, ry),
                Print(format!("│{t}{}│", " ".repeat(pad)))
            )?;
            Ok(())
        };
        let field_line =
            |out: &mut Vec<u8>, ry: u16, label: &str, val: &str, focused: bool| -> Result<()> {
                if focused {
                    queue!(
                        out,
                        SetBackgroundColor(Color::Cyan),
                        SetForegroundColor(Color::Black)
                    )?;
                } else {
                    queue!(out, ResetColor, SetForegroundColor(Color::White))?;
                }
                let marker = if focused { "❯" } else { " " };
                line(out, ry, &format!(" {marker} {label} {val}"))?;
                Ok(())
            };

        queue!(
            out,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold)
        )?;
        queue!(
            out,
            cursor::MoveTo(x, y),
            Print(format!(
                "┌ new worktree {}┐",
                "─".repeat(inner_w.saturating_sub(13))
            ))
        )?;
        queue!(out, SetAttribute(Attribute::Reset))?;
        let mut r = y + 1;

        // Clone: chosen value, or the filter + a match list while focused.
        let clone_val = self
            .clone
            .clone()
            .unwrap_or_else(|| format!("{}▏", self.filter));
        field_line(out, r, "clone   ", &clone_val, self.focus == Field::Clone)?;
        r += 1;
        if self.focus == Field::Clone {
            let visible = self.matches.len().clamp(1, MAX_ROWS);
            let start = self.sel.saturating_sub(MAX_ROWS - 1).min(
                self.matches
                    .len()
                    .saturating_sub(self.matches.len().min(MAX_ROWS)),
            );
            for row in 0..visible {
                if let Some(&mi) = self.matches.get(start + row) {
                    let sel = start + row == self.sel;
                    if sel {
                        queue!(
                            out,
                            SetBackgroundColor(Color::DarkCyan),
                            SetForegroundColor(Color::Black)
                        )?;
                    } else {
                        queue!(out, ResetColor, SetForegroundColor(Color::Grey))?;
                    }
                    line(
                        out,
                        r,
                        &format!("     {} {}", if sel { "❯" } else { " " }, self.clones[mi]),
                    )?;
                } else {
                    queue!(out, ResetColor, SetForegroundColor(Color::DarkGrey))?;
                    line(out, r, "     (no matches)")?;
                }
                r += 1;
            }
        }

        if is_sfaos {
            let box_ = if self.also_auto { "[x]" } else { "[ ]" };
            field_line(
                out,
                r,
                "        ",
                &format!("{box_} also make the auto worktree"),
                self.focus == Field::AlsoAuto,
            )?;
            r += 1;
        }

        field_line(
            out,
            r,
            "ticket  ",
            &self.ticket,
            self.focus == Field::Ticket,
        )?;
        r += 1;
        field_line(out, r, "name    ", &self.name, self.focus == Field::Name)?;
        r += 1;
        let up = if self.upstream.trim().is_empty() {
            "origin/master".to_string()
        } else {
            format!("origin/{}", self.upstream)
        };
        field_line(out, r, "upstream", &up, self.focus == Field::Upstream)?;
        r += 1;

        // Hint / error footer.
        queue!(out, ResetColor)?;
        if self.creating {
            const SPIN: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
            let frame = SPIN[self.spin % SPIN.len()];
            let msg = self.progress.as_deref().unwrap_or("working…");
            queue!(out, SetForegroundColor(Color::Yellow))?;
            line(out, r, &format!(" {frame} {msg}"))?;
        } else if let Some(e) = &self.error {
            queue!(out, SetForegroundColor(Color::Red))?;
            line(out, r, &format!(" ⚠ {e}"))?;
        } else {
            queue!(out, SetForegroundColor(Color::DarkGrey))?;
            line(out, r, " Tab next · Enter submit · Esc cancel")?;
        }
        r += 1;

        queue!(out, ResetColor, SetForegroundColor(Color::Cyan))?;
        queue!(
            out,
            cursor::MoveTo(x, r),
            Print(format!("└{}┘", "─".repeat(inner_w)))
        )?;
        queue!(
            out,
            ResetColor,
            SetAttribute(Attribute::Reset),
            cursor::Hide
        )?;
        Ok(())
    }
}

/// Immediate, non-hidden subdirectories of `root` that are git clones — i.e.
/// their `.git` is a *directory*. A linked worktree's `.git` is a *file* (a
/// gitfile pointing back at the main repo), so this excludes worktrees and
/// leaves only the clones you can branch new worktrees from.
fn list_clones(root: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() && p.join(".git").is_dir() {
                if let Some(name) = e.file_name().to_str() {
                    if !name.starts_with('.') {
                        v.push(name.to_string());
                    }
                }
            }
        }
    }
    v
}
