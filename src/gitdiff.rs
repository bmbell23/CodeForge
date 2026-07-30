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
    /// `rev` is `None` for the working tree (HEAD vs the file on disk) and
    /// `Some((base, head))` for a commit range (#74) — both sides read-only.
    Open {
        path: PathBuf,
        rev: Option<(String, String)>,
    },
}

/// Which list the panel is showing (#74). `l` descends Worktree -> Log, picking
/// commits descends Log -> Range; Esc walks back up.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// Changed files in the working tree — the panel's original view.
    Worktree,
    /// One-line commits, two of which can be marked to diff.
    Log,
    /// Changed files between the two picked revisions.
    Range,
}

/// One row of the commit list.
pub struct Commit {
    sha: String,
    subject: String,
    /// Relative author date (`2 days ago`).
    when: String,
}

/// A picked endpoint of the diff: a marked commit, or a rev typed at the `/`
/// prompt (`origin/main`, a tag, a full SHA).
struct Mark {
    /// Resolved short SHA — what git is actually given.
    sha: String,
    /// What the user picked, for the header (`origin/main` reads better than
    /// its SHA).
    label: String,
}

/// git's empty-tree object: the "before" side of a root commit, which has no
/// parent to diff against.
const EMPTY_TREE: &str = "4b825dc642cb6eb9a060e54bf8d69288fbee4904";

/// How far back the commit list goes. Deep enough to cover any branch you'd
/// actually diff, bounded so a huge repo doesn't stall the panel.
const LOG_LIMIT: usize = 200;

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
    /// Repo root; `None` when `dir` isn't a git repo. Every later git call
    /// (log, range diff) runs here rather than re-resolving it.
    root: Option<PathBuf>,
    mode: Mode,
    /// Commits for `Mode::Log`, newest first; loaded on first descent.
    commits: Vec<Commit>,
    /// The one or two picked endpoints. One = that commit vs its parent.
    marks: Vec<Mark>,
    /// The range `Mode::Range` is showing, as `(base, head)` short SHAs, with
    /// display labels for the header.
    range: Option<(Mark, Mark)>,
    /// Selection to restore when Esc walks back from Range to Log.
    log_sel: usize,
    /// Text typed at the `/` rev prompt, while it's open.
    input: Option<String>,
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
            root: repo_root(dir),
            mode: Mode::Worktree,
            commits: Vec::new(),
            marks: Vec::new(),
            range: None,
            log_sel: 0,
            input: None,
        }
    }

    /// Rows in the current mode's list — what windowing, hit-testing and
    /// selection all measure against.
    fn len(&self) -> usize {
        match self.mode {
            Mode::Log => self.commits.len(),
            _ => self.entries.len(),
        }
    }

    /// Switch to the commit list, loading it on first use (#74).
    fn enter_log(&mut self) {
        let Some(root) = self.root.clone() else {
            self.note = Some("not a git repository".into());
            return;
        };
        if self.commits.is_empty() {
            match load_log(&root) {
                Ok(c) if c.is_empty() => {
                    self.note = Some("no commits yet".into());
                    return;
                }
                Ok(c) => self.commits = c,
                Err(e) => {
                    self.note = Some(e);
                    return;
                }
            }
        }
        self.mode = Mode::Log;
        self.sel = self.log_sel.min(self.commits.len().saturating_sub(1));
        self.note = None;
    }

    /// Esc one level up: Range -> Log -> Worktree. Returns `Cancel` at the top,
    /// where Esc closes the panel as it always has.
    fn back(&mut self) -> DiffAction {
        match self.mode {
            Mode::Range => {
                self.mode = Mode::Log;
                self.sel = self.log_sel.min(self.commits.len().saturating_sub(1));
                self.range = None;
                self.entries = Vec::new();
                self.note = None;
                DiffAction::None
            }
            Mode::Log => {
                self.mode = Mode::Worktree;
                self.marks.clear();
                self.sel = 0;
                self.note = None;
                if let Some(root) = self.root.clone() {
                    self.entries = load(&root).unwrap_or_default();
                }
                DiffAction::None
            }
            Mode::Worktree => DiffAction::Cancel,
        }
    }

    /// Toggle the mark on commit `i`. Two endpoints is the maximum — a third
    /// press is refused rather than silently dropping an earlier pick.
    fn toggle_mark(&mut self, i: usize) {
        let Some(c) = self.commits.get(i) else { return };
        if let Some(pos) = self.marks.iter().position(|m| m.sha == c.sha) {
            self.marks.remove(pos);
            self.note = None;
            return;
        }
        if self.marks.len() == 2 {
            self.note = Some("two commits picked — Space unmarks one".into());
            return;
        }
        self.marks.push(Mark {
            sha: c.sha.clone(),
            label: c.sha.clone(),
        });
        self.note = None;
    }

    /// Resolve the rev typed at the `/` prompt and add it as an endpoint.
    fn add_typed_rev(&mut self, rev: &str) {
        let rev = rev.trim();
        if rev.is_empty() {
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        match git(&root, &["rev-parse", "--short", rev]) {
            Ok(sha) => {
                if self.marks.len() == 2 {
                    self.note = Some("two commits picked — Space unmarks one".into());
                    return;
                }
                self.marks.push(Mark {
                    sha: sha.trim().to_string(),
                    label: rev.to_string(),
                });
                self.note = None;
            }
            Err(_) => self.note = Some(format!("unknown rev: {rev}")),
        }
    }

    /// Diff the marked endpoints: two marks diff against each other, one marks
    /// that commit against its parent. Order comes from ancestry, not from the
    /// order they were picked, so the older side is always the base.
    fn diff_marks(&mut self) -> DiffAction {
        let Some(root) = self.root.clone() else {
            return DiffAction::None;
        };
        let (base, head) = match self.marks.len() {
            0 => {
                self.note = Some("Space marks a commit · / types a rev".into());
                return DiffAction::None;
            }
            1 => {
                let m = &self.marks[0];
                // A root commit has no parent — diff it against the empty tree
                // so its first import still shows as all-added.
                let parent = git(&root, &["rev-parse", "--short", &format!("{}^", m.sha)])
                    .map(|s| s.trim().to_string())
                    .unwrap_or_else(|_| EMPTY_TREE.to_string());
                (
                    Mark {
                        label: parent.clone(),
                        sha: parent,
                    },
                    Mark {
                        sha: m.sha.clone(),
                        label: m.label.clone(),
                    },
                )
            }
            _ => {
                let (a, b) = (&self.marks[0], &self.marks[1]);
                let older_first =
                    git(&root, &["merge-base", "--is-ancestor", &a.sha, &b.sha]).is_ok();
                let (lo, hi) = if older_first { (a, b) } else { (b, a) };
                (
                    Mark {
                        sha: lo.sha.clone(),
                        label: lo.label.clone(),
                    },
                    Mark {
                        sha: hi.sha.clone(),
                        label: hi.label.clone(),
                    },
                )
            }
        };
        match load_range(&root, &base.sha, &head.sha) {
            Ok(entries) => {
                self.log_sel = self.sel;
                self.entries = entries;
                self.range = Some((base, head));
                self.mode = Mode::Range;
                self.sel = 0;
                self.note = if self.entries.is_empty() {
                    Some("no changes between those commits".into())
                } else {
                    None
                };
                DiffAction::None
            }
            Err(e) => {
                self.note = Some(e);
                DiffAction::None
            }
        }
    }

    /// Open the file at row `i` in the current list, carrying the range when
    /// one is active.
    fn open(&self, i: usize) -> DiffAction {
        let Some(path) = self.entry_path(i) else {
            return DiffAction::None;
        };
        DiffAction::Open {
            path,
            rev: self
                .range
                .as_ref()
                .map(|(b, h)| (b.sha.clone(), h.sha.clone())),
        }
    }

    /// A click on row `i`: marks a commit in the log, opens a file elsewhere.
    pub fn click(&mut self, i: usize) -> DiffAction {
        self.select(i);
        match self.mode {
            Mode::Log => {
                self.toggle_mark(i);
                DiffAction::None
            }
            _ => self.open(i),
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
        if i < self.len() {
            self.sel = i;
        }
    }

    /// Move the selection by `delta` (mouse wheel).
    pub fn scroll(&mut self, delta: i32) {
        let n = self.len();
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
        let visible = self.len().min(view);
        let start = self
            .sel
            .saturating_sub(view - 1)
            .min(self.len().saturating_sub(visible));
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
            // The `/` rev prompt owns every key while it's open (#74).
            if let Some(buf) = self.input.as_mut() {
                match b {
                    0x1b | 0x03 => self.input = None,
                    b'\r' | b'\n' => {
                        let rev = self.input.take().unwrap_or_default();
                        self.add_typed_rev(&rev);
                    }
                    0x7f | 0x08 => {
                        buf.pop();
                    }
                    0x20..=0x7e => buf.push(b as char),
                    _ => {}
                }
                continue;
            }
            match self.esc {
                0 => match b {
                    0x1b => self.esc = 1,
                    0x03 | b'q' => return DiffAction::Cancel, // Ctrl-c / q
                    b'\r' | b'\n' => match self.mode {
                        Mode::Log => {
                            let a = self.diff_marks();
                            if !matches!(a, DiffAction::None) {
                                return a;
                            }
                        }
                        _ => {
                            let a = self.open(self.sel);
                            if !matches!(a, DiffAction::None) {
                                return a;
                            }
                        }
                    },
                    b'j' => self.scroll(1),
                    b'k' => self.scroll(-1),
                    // Descend into the commit list; Space marks endpoints there
                    // and `/` types a rev (#74).
                    b'l' if self.mode != Mode::Log => self.enter_log(),
                    b' ' if self.mode == Mode::Log => self.toggle_mark(self.sel),
                    b'/' if self.mode == Mode::Log => self.input = Some(String::new()),
                    _ => {}
                },
                1 => {
                    if b == b'[' {
                        self.esc = 2;
                    } else {
                        // A non-`[` byte right after ESC: not a sequence we
                        // know, treat the ESC as a step back.
                        self.esc = 0;
                        let a = self.back();
                        if !matches!(a, DiffAction::None) {
                            return a;
                        }
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
        // A lone Esc arrives as a single 0x1b byte: step back a level (and, at
        // the top level, dismiss).
        if self.esc == 1 {
            self.esc = 0;
            return self.back();
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

        // Top border with the summary as its title. A range says which two
        // commits it's between, so the list is never ambiguous (#74).
        let scope = match (&self.mode, &self.range) {
            (Mode::Log, _) => return self.render_log(out, rect, border, inner_w),
            (_, Some((b, h))) => format!("{}..{}", b.label, h.label),
            (_, None) => "git diff".to_string(),
        };
        let title = if self.entries.is_empty() {
            format!(" {scope} ")
        } else {
            format!(
                " {scope} · {} file{} +{tot_a} -{tot_d} ",
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
                let msg = match (&self.err, self.mode) {
                    (Some(e), _) => format!(" {e}"),
                    (None, Mode::Range) => " no changes between those commits".to_string(),
                    (None, _) => " working tree clean — nothing to diff".to_string(),
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
        let hint = match (&self.note, self.mode) {
            (Some(n), _) => format!(" {n}"),
            // Esc backs out of a range to the commit list, not straight to
            // closed (#74).
            (None, Mode::Range) => " ↑↓ move · Enter open · Esc commits".to_string(),
            // Log browse works with a clean worktree too — advertise it there,
            // where it's the only thing the panel can still do (#74).
            (None, _) if self.entries.is_empty() => " l commits · Esc close".to_string(),
            (None, _) => " ↑↓ move · Enter open · l commits · Esc close".to_string(),
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

impl DiffList {
    /// Draw the commit list (`Mode::Log`): `[x] sha subject … when`, with the
    /// picked endpoints checked (#74). Same frame as the file list so the panel
    /// doesn't visibly change shape when you descend into it.
    fn render_log(
        &self,
        out: &mut Vec<u8>,
        rect: &Rect,
        border: Color,
        inner_w: usize,
    ) -> Result<()> {
        let picked = self.marks.len();
        let title = format!(
            " commits · {} picked{} ",
            picked,
            match &self.marks[..] {
                [a] => format!(" ({})", a.label),
                [a, b] => format!(" ({}, {})", a.label, b.label),
                _ => String::new(),
            }
        );
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
        // Widest date in the window, right-aligned like the +/- column.
        let dw = self
            .commits
            .iter()
            .skip(start)
            .take(visible)
            .map(|c| c.when.chars().count())
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
                let c = &self.commits[idx];
                let selected = idx == self.sel;
                let marked = self.marks.iter().any(|m| m.sha == c.sha);
                let marker = if selected { '❯' } else { ' ' };
                let box_ = if marked { "[x]" } else { "[ ]" };
                // " ❯ [x] sha subject … when "
                let fixed = 3 + 4 + c.sha.chars().count() + 1 + dw + 1;
                let sw = inner_w.saturating_sub(fixed);
                let subject: String = clip_right(&c.subject, sw);
                let pad = sw.saturating_sub(subject.chars().count());
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
                    queue!(
                        out,
                        SetForegroundColor(if marked {
                            Color::Green
                        } else {
                            Color::DarkGrey
                        })
                    )?;
                }
                queue!(out, Print(box_), Print(" "))?;
                if !selected {
                    queue!(out, SetForegroundColor(Color::Yellow))?;
                }
                queue!(out, Print(&c.sha), Print(" "))?;
                if !selected {
                    queue!(out, SetForegroundColor(Color::White))?;
                }
                queue!(out, Print(&subject), Print(" ".repeat(pad)))?;
                if !selected {
                    queue!(out, SetForegroundColor(Color::DarkGrey))?;
                }
                let when = format!("{:>dw$} ", c.when);
                queue!(out, Print(when))?;
            } else {
                queue!(out, ResetColor, Print(" ".repeat(inner_w)))?;
            }
            queue!(out, ResetColor, SetForegroundColor(border), Print("│"))?;
            y += 1;
        }

        let hint = match (&self.input, &self.note) {
            // The rev prompt replaces the hint with what's being typed.
            (Some(buf), _) => format!(" rev: {buf}▏"),
            (None, Some(n)) => format!(" {n}"),
            (None, None) => " Space mark · Enter diff · / rev · Esc back".to_string(),
        };
        let h: String = hint.chars().take(inner_w).collect();
        queue!(
            out,
            cursor::MoveTo(rect.x, y),
            SetForegroundColor(border),
            Print("│"),
            SetForegroundColor(if self.input.is_some() {
                Color::White
            } else {
                Color::DarkGrey
            }),
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

/// Clip the tail of a subject line that doesn't fit (its start matters most).
fn clip_right(s: &str, w: usize) -> String {
    let n = s.chars().count();
    if n <= w {
        return s.to_string();
    }
    if w == 0 {
        return String::new();
    }
    let head: String = s.chars().take(w - 1).collect();
    format!("{head}…")
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

/// Repo root for `dir`, or `None` when it isn't inside a git repo.
fn repo_root(dir: &Path) -> Option<PathBuf> {
    let out = git(dir, &["rev-parse", "--show-toplevel"]).ok()?;
    Some(PathBuf::from(out.trim()))
}

/// The commit list for the panel: newest `LOG_LIMIT` commits of the current
/// branch as `(short sha, subject, relative date)` (#74).
fn load_log(root: &Path) -> Result<Vec<Commit>, String> {
    // Unit separator between fields so a subject containing anything still
    // splits cleanly.
    let out = git(
        root,
        &[
            "log",
            "--pretty=format:%h\x1f%s\x1f%cr",
            "-n",
            &LOG_LIMIT.to_string(),
        ],
    )?;
    Ok(out
        .lines()
        .filter_map(|l| {
            let mut it = l.splitn(3, '\x1f');
            let (sha, subject, when) = (it.next()?, it.next()?, it.next()?);
            Some(Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
                when: when.to_string(),
            })
        })
        .collect())
}

/// Changed files between two revisions (#74). Same shape as the working-tree
/// list, so the panel renders and opens them identically; `abs` is the path in
/// the working tree, which the diff view resolves against the two revs.
fn load_range(root: &Path, base: &str, head: &str) -> Result<Vec<Entry>, String> {
    let numstat = git(root, &["diff", "--numstat", base, head])?;
    let status = git(root, &["diff", "--name-status", base, head]).unwrap_or_default();
    let mut entries = Vec::new();
    for line in numstat.lines() {
        let mut it = line.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        // git's status letter for this path: A added, D deleted, M modified.
        let st = status
            .lines()
            .find(|l| l.split('\t').nth(1) == Some(p))
            .and_then(|l| l.chars().next())
            .unwrap_or('M');
        entries.push(Entry {
            rel: p.to_string(),
            abs: root.join(p),
            status: if st == 'A' { '?' } else { st },
            added: a.parse().ok(),
            removed: d.parse().ok(),
        });
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

    /// A list of `n` synthetic modified files, in the working-tree mode the
    /// panel starts in.
    fn fixture(n: u64) -> DiffList {
        DiffList {
            entries: (0..n)
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
            root: None,
            mode: Mode::Worktree,
            commits: Vec::new(),
            marks: Vec::new(),
            range: None,
            log_sel: 0,
            input: None,
        }
    }

    #[test]
    fn windowing_follows_selection() {
        let mut dl = fixture(20);
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
        let mut dl = fixture(3);
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
        let mut dl = fixture(3);
        assert!(matches!(dl.feed_bytes(b"\x1b[B"), DiffAction::None));
        assert_eq!(dl.sel, 1, "down arrow moves the selection");
        assert!(matches!(dl.feed_bytes(b"\x1b[A"), DiffAction::None));
        assert_eq!(dl.sel, 0, "up arrow moves it back");
        match dl.feed_bytes(b"\r") {
            DiffAction::Open { path, rev } => {
                assert_eq!(path, PathBuf::from("/tmp/f0"));
                assert!(rev.is_none(), "working-tree open carries no range");
            }
            _ => panic!("enter should open the selection"),
        }
        assert!(matches!(dl.feed_bytes(&[0x1b]), DiffAction::Cancel));
    }

    /// The commit list is modal: Space marks, Enter diffs, Esc steps back to
    /// the file list rather than closing the panel (#74).
    #[test]
    fn log_mode_marks_and_steps_back() {
        let mut dl = fixture(3);
        dl.commits = (0..4)
            .map(|i| Commit {
                sha: format!("sha{i}"),
                subject: format!("commit {i}"),
                when: "2 days ago".into(),
            })
            .collect();
        dl.mode = Mode::Log;

        assert!(matches!(dl.feed_bytes(b" "), DiffAction::None));
        assert_eq!(dl.marks.len(), 1, "space marks the selection");
        dl.feed_bytes(b"j");
        dl.feed_bytes(b" ");
        assert_eq!(dl.marks.len(), 2);
        dl.feed_bytes(b"j");
        dl.feed_bytes(b" ");
        assert_eq!(dl.marks.len(), 2, "a third mark is refused");
        assert!(dl.note.is_some(), "and says why");
        // Unmarking is the same key on an already-marked row.
        dl.feed_bytes(b"k");
        dl.feed_bytes(b" ");
        assert_eq!(dl.marks.len(), 1);

        // `/` opens the rev prompt and swallows keys until Enter/Esc.
        dl.feed_bytes(b"/");
        dl.feed_bytes(b"ma");
        assert_eq!(dl.input.as_deref(), Some("ma"));
        dl.feed_bytes(&[0x7f]);
        assert_eq!(dl.input.as_deref(), Some("m"), "backspace edits the prompt");
        dl.feed_bytes(&[0x1b]);
        assert!(dl.input.is_none(), "esc closes the prompt");

        assert!(matches!(dl.feed_bytes(&[0x1b]), DiffAction::None));
        assert!(
            matches!(dl.mode, Mode::Worktree),
            "esc steps back to the file list"
        );
        assert!(dl.marks.is_empty(), "and drops the picks");
        assert!(matches!(dl.feed_bytes(&[0x1b]), DiffAction::Cancel));
    }

    /// Windowing follows the commit list's length in log mode, not the file
    /// list's — the two lists have different row counts.
    #[test]
    fn log_mode_windows_over_commits() {
        let mut dl = fixture(2);
        dl.commits = (0..20)
            .map(|i| Commit {
                sha: format!("sha{i}"),
                subject: "s".into(),
                when: "now".into(),
            })
            .collect();
        dl.mode = Mode::Log;
        dl.scroll(99);
        assert_eq!(dl.sel, 19, "scroll clamps to the last commit");
        assert_eq!(dl.window(5), (15, 5));
    }
}
