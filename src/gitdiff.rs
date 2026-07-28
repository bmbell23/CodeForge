//! Native git-diff list (#18). `Ctrl-a g` toggles a persistent panel over the
//! terminal pane listing the working tree's changed files with +/- line
//! counts. It stays up until toggled off (`Ctrl-a g` again or toggling the
//! terminal). It owns the keyboard only while the terminal pane it covers is
//! focused — arrows move, Enter opens, Esc dismisses; with focus elsewhere
//! keys flow to the panes untouched and only the mouse drives it. Opening a
//! row hands off to the editor for a full-window side-by-side diff (HEAD
//! left, editable working file right); Esc there brings the pane layout back.
//! This module owns the list: gathering the entries, rendering, and input.
//! The zoom + nvim RPC handoff lives in main.rs.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Result;
use crossterm::style::{
    Attribute, Color, Print, ResetColor, SetAttribute, SetBackgroundColor, SetForegroundColor,
};
use crossterm::{cursor, queue};

use crate::layout::Rect;

/// Skip line-counting untracked files bigger than this (shown as `+?`).
const MAX_COUNT_BYTES: u64 = 2 * 1024 * 1024;

/// What a fed keystroke resulted in.
pub enum DiffAction {
    /// Keep going.
    None,
    /// User dismissed the panel (Esc / q / Ctrl-c).
    Cancel,
    /// User picked a file: open the side-by-side view on this absolute path.
    Open(PathBuf),
}

/// One changed file. `added`/`removed` are `None` for binary files (numstat
/// prints `-`) and for untracked files too large to count.
pub struct Entry {
    pub rel: String,
    pub abs: PathBuf,
    /// 'M' modified, 'D' deleted, '?' untracked.
    pub status: char,
    pub added: Option<u64>,
    pub removed: Option<u64>,
}

pub struct DiffList {
    entries: Vec<Entry>,
    /// Why the list is empty when it shouldn't be (e.g. not a git repo).
    err: Option<String>,
    /// Transient hint shown in the footer (e.g. the editor didn't respond).
    note: Option<String>,
    sel: usize,
    /// Escape-sequence parse state for `feed_bytes` (0 none, 1 ESC, 2 ESC[).
    esc: u8,
}

/// List rows that fit in the overlay: the rect minus the two border rows and
/// the footer hint row. Shared by render and mouse hit-testing so they always
/// agree on which rows are visible.
pub fn view_rows(rect: &Rect) -> usize {
    rect.h.saturating_sub(3) as usize
}

impl DiffList {
    /// Gather the diff for the repo containing `dir`. Never fails: problems
    /// (not a repo, git missing) render as a message inside the overlay.
    pub fn new(dir: &Path) -> DiffList {
        let (entries, err) = match load(dir) {
            Ok(e) => (e, None),
            Err(msg) => (Vec::new(), Some(msg)),
        };
        DiffList {
            entries,
            err,
            note: None,
            sel: 0,
            esc: 0,
        }
    }

    pub fn set_note(&mut self, s: &str) {
        self.note = Some(s.to_string());
    }

    /// Absolute path of entry `i`, for the open handoff.
    pub fn entry_path(&self, i: usize) -> Option<PathBuf> {
        self.entries.get(i).map(|e| e.abs.clone())
    }

    pub fn select(&mut self, i: usize) {
        if i < self.entries.len() {
            self.sel = i;
        }
    }

    /// Move the selection by `delta` (mouse wheel).
    pub fn scroll(&mut self, delta: i32) {
        let n = self.entries.len();
        if n == 0 {
            return;
        }
        self.sel = (self.sel as i64 + delta as i64).clamp(0, n as i64 - 1) as usize;
    }

    /// The windowed slice of entries on screen: `(start, count)`. Same math as
    /// the picker so clicks map to what's drawn.
    fn window(&self, view: usize) -> (usize, usize) {
        if view == 0 {
            return (0, 0);
        }
        let visible = self.entries.len().min(view);
        let start = self
            .sel
            .saturating_sub(view - 1)
            .min(self.entries.len().saturating_sub(visible));
        (start, visible)
    }

    /// Entry index under screen cell (`px`, `py`), if it's a list row.
    pub fn row_at(&self, rect: &Rect, px: u16, py: u16) -> Option<usize> {
        if px <= rect.x || px >= rect.x + rect.w.saturating_sub(1) {
            return None;
        }
        let (start, visible) = self.window(view_rows(rect));
        let top = rect.y + 1;
        if py < top || py >= top + visible as u16 {
            return None;
        }
        Some(start + (py - top) as usize)
    }

    /// Drive the list from raw input bytes — used only while the terminal
    /// pane under the panel is focused (the shell is covered, so swallowing
    /// its keys is safe). Arrows/j/k move, Enter opens, Esc/q/Ctrl-c dismiss.
    pub fn feed_bytes(&mut self, bytes: &[u8]) -> DiffAction {
        for &b in bytes {
            match self.esc {
                0 => match b {
                    0x1b => self.esc = 1,
                    0x03 | b'q' => return DiffAction::Cancel, // Ctrl-c / q
                    b'\r' | b'\n' => {
                        if let Some(p) = self.entry_path(self.sel) {
                            return DiffAction::Open(p);
                        }
                    }
                    b'j' => self.scroll(1),
                    b'k' => self.scroll(-1),
                    _ => {}
                },
                1 => {
                    if b == b'[' {
                        self.esc = 2;
                    } else {
                        // A non-`[` byte right after ESC: not a sequence we
                        // know, treat the ESC as a dismissal.
                        self.esc = 0;
                        return DiffAction::Cancel;
                    }
                }
                _ => {
                    self.esc = 0;
                    match b {
                        b'A' => self.scroll(-1),
                        b'B' => self.scroll(1),
                        _ => {}
                    }
                }
            }
        }
        // A lone Esc arrives as a single 0x1b byte: dismiss.
        if self.esc == 1 {
            self.esc = 0;
            return DiffAction::Cancel;
        }
        DiffAction::None
    }

    /// Draw the overlay filling `rect` (the terminal pane's footprint).
    pub fn render(&self, out: &mut Vec<u8>, rect: &Rect, focused: bool) -> Result<()> {
        if rect.w < 24 || rect.h < 4 {
            return Ok(());
        }
        let inner_w = (rect.w - 2) as usize;
        // Border is bright when the panel has focus, dim otherwise — matching the
        // pane borders, so it doesn't look permanently focused (#61).
        let border = if focused {
            Color::Cyan
        } else {
            Color::DarkGrey
        };
        let (tot_a, tot_d) = self.entries.iter().fold((0u64, 0u64), |(a, d), e| {
            (a + e.added.unwrap_or(0), d + e.removed.unwrap_or(0))
        });

        // Top border with the summary as its title.
        let title = if self.entries.is_empty() {
            " git diff ".to_string()
        } else {
            format!(
                " git diff · {} file{} +{tot_a} -{tot_d} ",
                self.entries.len(),
                if self.entries.len() == 1 { "" } else { "s" },
            )
        };
        let t: String = title.chars().take(inner_w).collect();
        queue!(
            out,
            ResetColor,
            SetForegroundColor(border),
            SetAttribute(Attribute::Bold),
            cursor::MoveTo(rect.x, rect.y),
            Print(format!(
                "┌{t}{}┐",
                "─".repeat(inner_w.saturating_sub(t.chars().count()))
            )),
            SetAttribute(Attribute::Reset),
        )?;

        let view = view_rows(rect);
        let (start, visible) = self.window(view);
        let mut y = rect.y + 1;

        // The +/- column is right-aligned; compute its width from the window.
        let count_str = |v: Option<u64>, sign: char| match v {
            Some(n) => format!("{sign}{n}"),
            None => format!("{sign}?"),
        };
        let cw = self
            .entries
            .iter()
            .skip(start)
            .take(visible)
            .map(|e| count_str(e.added, '+').len() + 1 + count_str(e.removed, '-').len())
            .max()
            .unwrap_or(0);

        for vis in 0..view {
            queue!(
                out,
                cursor::MoveTo(rect.x, y),
                ResetColor,
                SetForegroundColor(border),
                Print("│"),
            )?;
            let idx = start + vis;
            if vis < visible {
                let e = &self.entries[idx];
                let selected = idx == self.sel;
                let marker = if selected { '❯' } else { ' ' };
                // " ❯ M path" ... "+12 -3 "
                let fixed = 5 + cw + 1; // lead-in + counts + trailing space
                let pw = inner_w.saturating_sub(fixed);
                let path = clip_left(&e.rel, pw);
                let pad = pw.saturating_sub(path.chars().count());
                if selected {
                    queue!(
                        out,
                        SetBackgroundColor(Color::Cyan),
                        SetForegroundColor(Color::Black)
                    )?;
                } else {
                    queue!(out, ResetColor)?;
                }
                queue!(out, Print(format!(" {marker} ")))?;
                if !selected {
                    let sc = match e.status {
                        'D' => Color::Red,
                        '?' => Color::Green,
                        _ => Color::Yellow,
                    };
                    queue!(out, SetForegroundColor(sc))?;
                }
                queue!(out, Print(e.status), Print(" "))?;
                if !selected {
                    queue!(out, SetForegroundColor(Color::White))?;
                }
                queue!(out, Print(&path), Print(" ".repeat(pad)))?;
                // +/- counts: green/red normally, inherited fg when selected.
                let (plus, minus) = (count_str(e.added, '+'), count_str(e.removed, '-'));
                let lead = cw - (plus.len() + 1 + minus.len());
                queue!(out, Print(" ".repeat(lead)))?;
                if !selected {
                    queue!(out, SetForegroundColor(Color::Green))?;
                }
                queue!(out, Print(&plus), Print(" "))?;
                if !selected {
                    queue!(out, SetForegroundColor(Color::Red))?;
                }
                queue!(out, Print(&minus), Print(" "))?;
            } else if vis == 0 && self.entries.is_empty() {
                // First row carries the empty/error message.
                let msg = match &self.err {
                    Some(e) => format!(" {e}"),
                    None => " working tree clean — nothing to diff".to_string(),
                };
                let m: String = msg.chars().take(inner_w).collect();
                queue!(
                    out,
                    ResetColor,
                    SetForegroundColor(if self.err.is_some() {
                        Color::Red
                    } else {
                        Color::Green
                    }),
                    Print(&m),
                    Print(" ".repeat(inner_w.saturating_sub(m.chars().count()))),
                )?;
            } else {
                queue!(out, ResetColor, Print(" ".repeat(inner_w)))?;
            }
            queue!(out, ResetColor, SetForegroundColor(border), Print("│"))?;
            y += 1;
        }

        // Footer hint row + bottom border.
        let hint = match &self.note {
            Some(n) => format!(" {n}"),
            None if self.entries.is_empty() => " Esc or Ctrl-a g close".to_string(),
            None => " ↑↓ move · Enter/click open · Esc or Ctrl-a g close".to_string(),
        };
        let h: String = hint.chars().take(inner_w).collect();
        queue!(
            out,
            cursor::MoveTo(rect.x, y),
            SetForegroundColor(border),
            Print("│"),
            SetForegroundColor(Color::DarkGrey),
            Print(&h),
            Print(" ".repeat(inner_w.saturating_sub(h.chars().count()))),
            SetForegroundColor(border),
            Print("│"),
            cursor::MoveTo(rect.x, y + 1),
            Print(format!("└{}┘", "─".repeat(inner_w))),
            ResetColor,
            SetAttribute(Attribute::Reset),
            cursor::Hide,
        )?;
        Ok(())
    }
}

/// Keep the tail of a path when it doesn't fit (the filename end matters most).
fn clip_left(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let tail: String = s.chars().skip(n - (w - 1)).collect();
    format!("…{tail}")
}

/// Run git in `dir`, returning stdout or the trimmed stderr as the error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Changed files for the repo containing `dir`: `git diff HEAD --numstat`
/// (staged + unstaged in one view) plus untracked files, in that order.
fn load(dir: &Path) -> Result<Vec<Entry>, String> {
    let root = PathBuf::from(
        git(dir, &["rev-parse", "--show-toplevel"])
            .map_err(|_| "not a git repository".to_string())?
            .trim(),
    );
    let mut entries = Vec::new();
    // A repo with no commits yet has no HEAD; fall back to the index diff.
    let numstat = git(&root, &["diff", "HEAD", "--numstat"])
        .or_else(|_| git(&root, &["diff", "--numstat"]))?;
    for line in numstat.lines() {
        let mut it = line.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let abs = root.join(p);
        entries.push(Entry {
            rel: p.to_string(),
            status: if abs.exists() { 'M' } else { 'D' },
            abs,
            added: a.parse().ok(),
            removed: d.parse().ok(),
        });
    }
    if let Ok(unt) = git(&root, &["ls-files", "--others", "--exclude-standard"]) {
        for p in unt.lines().filter(|l| !l.is_empty()) {
            let abs = root.join(p);
            entries.push(Entry {
                rel: p.to_string(),
                added: count_lines(&abs),
                abs,
                status: '?',
                removed: Some(0),
            });
        }
    }
    Ok(entries)
}

/// Line count of an untracked file (its whole content is "added"). `None` for
/// unreadable or oversized files.
fn count_lines(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > MAX_COUNT_BYTES {
        return None;
    }
    let s = std::fs::read_to_string(path).ok()?;
    Some(s.lines().count() as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windowing_follows_selection() {
        let mut dl = DiffList {
            entries: (0..20)
                .map(|i| Entry {
                    rel: format!("f{i}"),
                    abs: PathBuf::from(format!("/tmp/f{i}")),
                    status: 'M',
                    added: Some(i),
                    removed: Some(0),
                })
                .collect(),
            err: None,
            note: None,
            sel: 0,
            esc: 0,
        };
        assert_eq!(dl.window(5), (0, 5));
        dl.sel = 10;
        let (start, vis) = dl.window(5);
        assert_eq!(vis, 5);
        assert!(start <= 10 && 10 < start + vis, "selection stays visible");
        dl.sel = 19;
        assert_eq!(dl.window(5), (15, 5));
    }

    #[test]
    fn clip_keeps_tail() {
        assert_eq!(clip_left("short", 10), "short");
        let c = clip_left("a/very/long/path/file.rs", 10);
        assert_eq!(c.chars().count(), 10);
        assert!(c.ends_with("file.rs"));
        assert!(c.starts_with('…'));
    }

    #[test]
    fn wheel_scroll_clamps_and_click_maps_to_row() {
        let mut dl = DiffList {
            entries: (0..3)
                .map(|i| Entry {
                    rel: format!("f{i}"),
                    abs: PathBuf::from(format!("/tmp/f{i}")),
                    status: 'M',
                    added: Some(1),
                    removed: Some(1),
                })
                .collect(),
            err: None,
            note: None,
            sel: 0,
            esc: 0,
        };
        dl.scroll(5);
        assert_eq!(dl.sel, 2, "scroll clamps to the last entry");
        dl.scroll(-99);
        assert_eq!(dl.sel, 0, "scroll clamps to the first entry");
        let rect = Rect {
            x: 10,
            y: 5,
            w: 40,
            h: 10,
        };
        // Row 1 of the list starts just under the top border.
        assert_eq!(dl.row_at(&rect, 12, 6), Some(0));
        assert_eq!(dl.row_at(&rect, 12, 8), Some(2));
        assert_eq!(dl.row_at(&rect, 12, 9), None, "below the last entry");
        assert_eq!(dl.row_at(&rect, 10, 6), None, "border isn't a row");
        assert_eq!(
            dl.entry_path(dl.row_at(&rect, 12, 8).unwrap()),
            Some(PathBuf::from("/tmp/f2"))
        );
    }

    #[test]
    fn keys_move_open_and_dismiss() {
        let mut dl = DiffList {
            entries: (0..3)
                .map(|i| Entry {
                    rel: format!("f{i}"),
                    abs: PathBuf::from(format!("/tmp/f{i}")),
                    status: 'M',
                    added: Some(1),
                    removed: Some(1),
                })
                .collect(),
            err: None,
            note: None,
            sel: 0,
            esc: 0,
        };
        assert!(matches!(dl.feed_bytes(b"\x1b[B"), DiffAction::None));
        assert_eq!(dl.sel, 1, "down arrow moves the selection");
        assert!(matches!(dl.feed_bytes(b"\x1b[A"), DiffAction::None));
        assert_eq!(dl.sel, 0, "up arrow moves it back");
        match dl.feed_bytes(b"\r") {
            DiffAction::Open(p) => assert_eq!(p, PathBuf::from("/tmp/f0")),
            _ => panic!("enter should open the selection"),
        }
        assert!(matches!(dl.feed_bytes(&[0x1b]), DiffAction::Cancel));
    }
}
