//! A fuzzy project picker: lists immediate subdirectories of the projects root,
//! filters as you type, and returns the chosen path. Used two ways:
//!   * at startup (bare `forge`) via `run_blocking`, which owns stdin directly;
//!   * live (`Ctrl-a p`) as an overlay in the main loop, driven by `feed_bytes`
//!     since the main loop already owns stdin.

use std::io::{self, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode};
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::{cursor, queue, terminal};

/// Max project rows shown at once.
const MAX_ROWS: usize = 12;

/// What a fed keystroke resulted in.
pub enum PickerAction {
    /// Keep going.
    None,
    /// User cancelled (Ctrl-c).
    Cancel,
    /// User picked a project directory.
    Chosen(PathBuf),
    /// User asked to create a new worktree (Ctrl-n) — open the worktree form.
    NewWorktree,
}

pub struct Picker {
    root: PathBuf,
    all: Vec<String>,
    /// Parallel to `all`: this project already has a window, so choosing it
    /// jumps there instead of starting a fresh one (#104).
    open: Vec<bool>,
    filter: String,
    matches: Vec<usize>,
    sel: usize,
    /// Escape-sequence parse state for `feed_bytes` (0 none, 1 saw ESC, 2 saw ESC[).
    esc: u8,
}

impl Picker {
    /// List *every* project under `root`. `open` are directories already showing
    /// in a window; they stay in the list and are marked, because choosing one
    /// focuses that window rather than opening a second editor over the same
    /// working tree (#104). Hiding them (#82's first fix) made the list a
    /// different set every time and left no single "go to project" gesture.
    pub fn new_with_open(root: PathBuf, open: &[PathBuf]) -> Picker {
        let mut all: Vec<String> = list_dirs(&root);
        all.sort_by_key(|s| s.to_lowercase());
        let is_open = all
            .iter()
            .map(|name| open.iter().any(|d| d == &root.join(name)))
            .collect();
        let mut p = Picker {
            root,
            all,
            open: is_open,
            filter: String::new(),
            matches: Vec::new(),
            sel: 0,
            esc: 0,
        };
        p.refilter();
        p
    }

    /// Every project under `root` — the startup picker, where nothing is open.
    pub fn new(root: PathBuf) -> Picker {
        Picker::new_with_open(root, &[])
    }

    fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
        self.matches = self
            .all
            .iter()
            .enumerate()
            .filter(|(_, n)| n.to_lowercase().contains(&f))
            .map(|(i, _)| i)
            .collect();
        if self.sel >= self.matches.len() {
            self.sel = self.matches.len().saturating_sub(1);
        }
    }

    fn input_char(&mut self, c: char) {
        self.filter.push(c);
        self.refilter();
    }
    fn backspace(&mut self) {
        self.filter.pop();
        self.refilter();
    }
    fn up(&mut self) {
        self.sel = self.sel.saturating_sub(1);
    }
    fn down(&mut self) {
        if self.sel + 1 < self.matches.len() {
            self.sel += 1;
        }
    }
    fn selected(&self) -> Option<PathBuf> {
        self.matches
            .get(self.sel)
            .map(|&i| self.root.join(&self.all[i]))
    }

    /// The windowed slice of matches currently on screen: `(start, count)`.
    /// Depends only on `sel` and `MAX_ROWS`, so `render` and `feed_bytes` agree
    /// on which rows are visible (and thus which number picks which project).
    fn window(&self) -> (usize, usize) {
        let visible = self.matches.len().min(MAX_ROWS);
        let start = self
            .sel
            .saturating_sub(MAX_ROWS - 1)
            .min(self.matches.len().saturating_sub(visible));
        (start, visible)
    }

    /// Project at visible row `n` (0-based), if that row exists on screen.
    fn at_visible(&self, n: usize) -> Option<PathBuf> {
        let (start, visible) = self.window();
        if n < visible {
            let mi = self.matches[start + n];
            Some(self.root.join(&self.all[mi]))
        } else {
            None
        }
    }

    /// Drive the picker from raw input bytes (main-loop / overlay mode).
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> PickerAction {
        for &b in bytes {
            match self.esc {
                0 => match b {
                    0x1b => self.esc = 1,
                    0x03 => return PickerAction::Cancel, // Ctrl-c
                    0x0e => return PickerAction::NewWorktree, // Ctrl-n
                    b'\r' | b'\n' => {
                        if let Some(p) = self.selected() {
                            return PickerAction::Chosen(p);
                        }
                    }
                    // A digit opens the numbered row directly (shown 1-9). If no
                    // such row is on screen, fall through and treat it as filter
                    // text so a project name with a digit is still typable.
                    b'1'..=b'9' => {
                        if let Some(p) = self.at_visible((b - b'1') as usize) {
                            return PickerAction::Chosen(p);
                        }
                        self.input_char(b as char);
                    }
                    0x7f | 0x08 => self.backspace(),
                    0x20..=0x7e => self.input_char(b as char),
                    _ => {}
                },
                1 => {
                    // ESC then `[` begins an arrow sequence; anything else means
                    // it was a lone Esc keypress — handled after the loop.
                    if b == b'[' {
                        self.esc = 2;
                    } else {
                        self.esc = 0;
                        // A non-`[` byte right after ESC: not a sequence we know.
                        return PickerAction::Cancel;
                    }
                }
                _ => {
                    self.esc = 0;
                    match b {
                        b'A' => self.up(),
                        b'B' => self.down(),
                        _ => {}
                    }
                }
            }
        }
        // A lone Esc (no `[` followed) arrives as a single 0x1b byte: cancel.
        if self.esc == 1 {
            self.esc = 0;
            return PickerAction::Cancel;
        }
        PickerAction::None
    }

    /// Run the picker as a blocking modal that owns stdin (startup path).
    /// Returns the chosen directory, or `None` if cancelled.
    pub fn run_blocking(&mut self, out: &mut io::Stdout) -> Result<Option<PathBuf>> {
        loop {
            let (cols, rows) = terminal::size()?;
            let mut buf = Vec::new();
            queue!(buf, terminal::Clear(terminal::ClearType::All))?;
            self.render(&mut buf, cols, rows)?;
            out.write_all(&buf)?;
            out.flush()?;
            match event::read()? {
                Event::Key(k) => match k.code {
                    KeyCode::Esc => return Ok(None),
                    KeyCode::Char('c') if k.modifiers.contains(event::KeyModifiers::CONTROL) => {
                        return Ok(None)
                    }
                    KeyCode::Enter => {
                        if let Some(p) = self.selected() {
                            return Ok(Some(p));
                        }
                    }
                    KeyCode::Backspace => self.backspace(),
                    KeyCode::Up => self.up(),
                    KeyCode::Down => self.down(),
                    // Digit opens the numbered row (see feed_bytes); otherwise
                    // it's filter text.
                    KeyCode::Char(c) if c.is_ascii_digit() && c != '0' => {
                        if let Some(p) = self.at_visible(c as usize - '1' as usize) {
                            return Ok(Some(p));
                        }
                        self.input_char(c);
                    }
                    KeyCode::Char(c) => self.input_char(c),
                    _ => {}
                },
                Event::Resize(..) => {}
                _ => {}
            }
        }
    }

    /// Draw the picker box centered on the screen into `out`.
    pub fn render(&self, out: &mut Vec<u8>, cols: u16, rows: u16) -> Result<()> {
        let w: u16 = 44.min(cols.saturating_sub(2)).max(12);
        let inner_w = (w - 2) as usize;
        let visible = self.matches.len().min(MAX_ROWS);
        // Rows: title, filter, separator, list...  + 2 borders.
        let body = 3 + visible.max(1);
        let h = body as u16 + 2;
        if w + 2 > cols || h + 2 > rows {
            return Ok(());
        }
        let x = (cols - w) / 2;
        let y = (rows - h) / 2;

        queue!(
            out,
            SetForegroundColor(Color::Cyan),
            SetAttribute(Attribute::Bold)
        )?;
        queue!(
            out,
            cursor::MoveTo(x, y),
            Print(format!(
                "┌ open project {}┐",
                "─".repeat(inner_w.saturating_sub(13))
            ))
        )?;

        let mut row = y + 1;
        let line = |out: &mut Vec<u8>, ry: u16, text: String| -> Result<()> {
            let t: String = text.chars().take(inner_w).collect();
            let pad = inner_w.saturating_sub(t.chars().count());
            queue!(
                out,
                cursor::MoveTo(x, ry),
                Print(format!("│{t}{}│", " ".repeat(pad)))
            )?;
            Ok(())
        };

        queue!(
            out,
            SetAttribute(Attribute::Reset),
            SetForegroundColor(Color::Cyan)
        )?;
        line(out, row, format!(" > {}", self.filter))?;
        row += 1;
        line(
            out,
            row,
            format!(
                " {} matches · 1-9 open · ^n worktree · Esc",
                self.matches.len()
            ),
        )?;
        row += 1;

        // Window the list around the selection (same math as `window()`).
        let (start, _) = self.window();
        for (i, &mi) in self.matches.iter().enumerate().skip(start).take(visible) {
            let name = &self.all[mi];
            let vis = i - start; // 0-based row on screen
            let num = if vis < 9 {
                (b'1' + vis as u8) as char
            } else {
                ' '
            };
            let marker = if i == self.sel { "❯" } else { " " };
            let open = self.open[mi];
            if i == self.sel {
                queue!(
                    out,
                    SetBackgroundColor(Color::Cyan),
                    SetForegroundColor(Color::Black)
                )?;
            } else if open {
                // Dimmed and suffixed: picking this row switches to the window
                // that already has it, rather than opening another (#104).
                queue!(out, ResetColor, SetForegroundColor(Color::Grey))?;
            } else {
                queue!(out, ResetColor, SetForegroundColor(Color::White))?;
            }
            let tag = if open { " · open" } else { "" };
            line(out, row, format!(" {num} {marker} {name}{tag}"))?;
            row += 1;
        }
        if visible == 0 {
            queue!(out, ResetColor, SetForegroundColor(Color::DarkGrey))?;
            line(out, row, "  (no matches)".into())?;
            row += 1;
        }

        queue!(out, ResetColor, SetForegroundColor(Color::Cyan))?;
        queue!(
            out,
            cursor::MoveTo(x, row),
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

/// Immediate, non-hidden subdirectories of `root`.
fn list_dirs(root: &Path) -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(rd) = std::fs::read_dir(root) {
        for e in rd.flatten() {
            if e.path().is_dir() {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Every project stays in the list, open or not — the picker is the one
    /// "go to project" gesture, and an open one is marked rather than hidden
    /// (#104). Choosing a marked row focuses its window (`src/main.rs`).
    #[test]
    fn open_projects_are_listed_and_marked() {
        let tmp = std::env::temp_dir().join(format!("cf-picker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        for d in ["alpha", "beta", "gamma"] {
            std::fs::create_dir_all(tmp.join(d)).unwrap();
        }
        // A hidden dir and a plain file are never projects.
        std::fs::create_dir_all(tmp.join(".hidden")).unwrap();
        std::fs::write(tmp.join("a-file"), "x").unwrap();

        // The startup picker has nothing open and marks nothing.
        let fresh = Picker::new(tmp.clone());
        assert_eq!(fresh.all, ["alpha", "beta", "gamma"]);
        assert_eq!(fresh.open, [false, false, false]);

        let p = Picker::new_with_open(tmp.clone(), &[tmp.join("beta")]);
        assert_eq!(p.all, ["alpha", "beta", "gamma"]);
        assert_eq!(p.open, [false, true, false]);
        // The open one is still selectable, and still filterable by name.
        let mut p = p;
        p.input_char('b');
        assert_eq!(p.selected(), Some(tmp.join("beta")));

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
