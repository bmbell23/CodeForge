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
    /// Commits that touched one file (#92) — the history of what the editor is
    /// showing. Enter opens that commit's *full* changed-file set, so the diff
    /// steps like any other range.
    FileLog,
}

/// One row of the commit list.
pub struct Commit {
    sha: String,
    subject: String,
    /// Relative author date (`2 days ago`).
    when: String,
    /// Author name — shown in `Mode::FileLog`'s "who touched it" column (#92).
    author: String,
    /// What this commit did to the tracked file in `Mode::FileLog`: 'M', 'A',
    /// 'D', 'R'. `' '` in the repo-wide log, which isn't about one file.
    fstatus: char,
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
    /// 'M' modified, 'D' deleted, 'A' added (untracked, or added in a range).
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
    /// The file `Mode::FileLog` is tracking, repo-relative (#92).
    file_rel: Option<String>,
    /// Which list Range was entered from, so Esc returns there — the commit
    /// list, or a file's history (#92).
    range_from: Mode,
    /// The single commit Range is showing, when it is one commit rather than a
    /// span: its message is unambiguous, so the header can name it (#93).
    range_commit: Option<Commit>,
    /// The full commit message, while the overlay is open (#93).
    message: Option<Vec<String>>,
}

/// Repo-wide `(+added, -removed)` for the footer (#91), counted over every
/// entry rather than the visible window. `added`/`removed` are `None` for
/// binaries and for untracked files too big to count; those set a trailing `?`
/// instead of folding in as 0, which would quietly under-report the total.
/// `None` when there is nothing to total.
fn totals(entries: &[Entry]) -> Option<(String, String)> {
    if entries.is_empty() {
        return None;
    }
    let (mut add, mut rem) = (0u64, 0u64);
    let (mut unk_a, mut unk_r) = (false, false);
    for e in entries {
        match e.added {
            Some(n) => add += n,
            None => unk_a = true,
        }
        match e.removed {
            Some(n) => rem += n,
            None => unk_r = true,
        }
    }
    Some((
        format!("+{add}{}", if unk_a { "?" } else { "" }),
        format!("-{rem}{}", if unk_r { "?" } else { "" }),
    ))
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
            file_rel: None,
            range_from: Mode::Log,
            range_commit: None,
            message: None,
        }
    }

    /// Open straight onto one file's history (#92): the commits that touched
    /// `file`, newest first. Entered from the editor rather than by descending
    /// through the panel, so Esc closes instead of walking back to a list the
    /// user never saw.
    pub fn new_file_log(dir: &Path, file: &Path) -> DiffList {
        let mut d = DiffList::new(dir);
        let Some(root) = d.root.clone() else {
            d.err = Some("not a git repository".into());
            return d;
        };
        let rel = match file.strip_prefix(&root) {
            Ok(r) => r.to_string_lossy().to_string(),
            Err(_) => {
                d.err = Some("file is outside this repository".into());
                return d;
            }
        };
        match load_file_log(&root, &rel) {
            Ok(c) if c.is_empty() => d.err = Some(format!("no commits touch {rel}")),
            Ok(c) => {
                d.commits = c;
                d.mode = Mode::FileLog;
                d.file_rel = Some(rel);
                d.entries = Vec::new();
            }
            Err(e) => d.err = Some(e),
        }
        d
    }

    /// Rows in the current mode's list — what windowing, hit-testing and
    /// selection all measure against.
    fn len(&self) -> usize {
        match self.mode {
            Mode::Log | Mode::FileLog => self.commits.len(),
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
        // The message overlay is the innermost level: Esc closes it first (#93).
        if self.message.is_some() {
            self.message = None;
            return DiffAction::None;
        }
        match self.mode {
            Mode::Range => {
                // Back to whichever list opened this range — the commit list, or
                // a file's history (#92).
                self.mode = self.range_from;
                self.sel = self.log_sel.min(self.commits.len().saturating_sub(1));
                self.range = None;
                self.range_commit = None;
                self.entries = Vec::new();
                self.note = None;
                DiffAction::None
            }
            // Opened straight from the editor, so there's no list underneath to
            // walk back to (#92).
            Mode::FileLog => DiffAction::Cancel,
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

    /// Make the selected commit the only mark (#92). A file's history has no
    /// two-endpoint picking — Enter means "this commit", so any earlier mark is
    /// replaced rather than accumulating into a span.
    fn mark_only_selected(&mut self) {
        let Some(c) = self.commits.get(self.sel) else {
            return;
        };
        self.marks = vec![Mark {
            sha: c.sha.clone(),
            label: c.sha.clone(),
        }];
    }

    /// Show or hide the full commit message (#93). Loaded on demand — the log
    /// only carries subjects, and a body can be long.
    fn toggle_message(&mut self) {
        if self.message.is_some() {
            self.message = None;
            return;
        }
        let (Some(root), Some(c)) = (self.root.clone(), self.range_commit.as_ref()) else {
            return;
        };
        match commit_message(&root, &c.sha) {
            Ok(lines) => self.message = Some(lines),
            Err(e) => self.note = Some(e),
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
        // One mark is one commit, so its message is unambiguous — remember it so
        // the header can name it and `m` can show the body (#93). A span of two
        // endpoints can cover many commits and gets no single message.
        let single = (self.marks.len() == 1).then(|| {
            let m = &self.marks[0];
            self.commits
                .iter()
                .find(|c| c.sha == m.sha)
                .map(|c| Commit {
                    sha: c.sha.clone(),
                    subject: c.subject.clone(),
                    when: c.when.clone(),
                    author: c.author.clone(),
                    fstatus: c.fstatus,
                })
                .unwrap_or_else(|| Commit {
                    sha: m.sha.clone(),
                    subject: m.label.clone(),
                    when: String::new(),
                    author: String::new(),
                    fstatus: ' ',
                })
        });
        match load_range(&root, &base.sha, &head.sha) {
            Ok(entries) => {
                self.log_sel = self.sel;
                self.entries = entries;
                self.range = Some((base, head));
                self.range_from = self.mode;
                self.range_commit = single;
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

    /// Re-read the list currently on screen, keeping the mode and the picked
    /// range. Used when the full-screen diff closes (#74): rebuilding the panel
    /// from scratch there would drop you back on the working-tree list instead
    /// of the commit range you were browsing.
    pub fn refresh(&mut self) {
        let Some(root) = self.root.clone() else {
            return;
        };
        match (self.mode, &self.range) {
            (Mode::Range, Some((b, h))) => {
                if let Ok(e) = load_range(&root, &b.sha, &h.sha) {
                    self.entries = e;
                }
            }
            (Mode::Log, _) => {
                if let Ok(c) = load_log(&root) {
                    self.commits = c;
                }
            }
            _ => {
                if let Ok(e) = load(&root) {
                    self.entries = e;
                }
            }
        }
        self.sel = self.sel.min(self.len().saturating_sub(1));
        self.note = None;
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
                        // Enter on a file's history opens that commit's whole
                        // changed-file set, not just this file's diff (#92).
                        Mode::FileLog => {
                            self.mark_only_selected();
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
                    b'l' if !matches!(self.mode, Mode::Log | Mode::FileLog) => self.enter_log(),
                    b' ' if self.mode == Mode::Log => self.toggle_mark(self.sel),
                    b'/' if self.mode == Mode::Log => self.input = Some(String::new()),
                    // The commit message, while a single commit is on show (#93).
                    b'm' if self.mode == Mode::Range && self.range_commit.is_some() => {
                        self.toggle_message()
                    }
                    _ => {}
                },
                1 => {
                    // Both encodings of an arrow key: CSI (`ESC [ A`) and SS3
                    // (`ESC O A`), which terminals send in application-cursor
                    // mode — treating SS3's `O` as "not a sequence" made arrows
                    // read as Esc and step the panel back instead (#74).
                    if b == b'[' || b == b'O' {
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
                    // Parameters (`ESC [ 1;5 A`) come before the final byte, so
                    // skip them rather than treating the first digit as the end
                    // of the sequence — that made modified arrows do nothing
                    // and dropped the rest of the sequence into the list as
                    // stray keys (#74).
                    if b.is_ascii_digit() || b == b';' {
                        continue;
                    }
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
            (Mode::Log | Mode::FileLog, _) => return self.render_log(out, rect, border, inner_w),
            // A single commit names itself in the header (#93) — `..` between a
            // parent and its child says nothing a subject wouldn't say better.
            (_, Some(_)) if self.range_commit.is_some() => {
                let c = self.range_commit.as_ref().unwrap();
                format!("{} · {}", c.sha, c.subject)
            }
            (_, Some((b, h))) => format!("{}..{}", b.label, h.label),
            (_, None) => "git diff".to_string(),
        };
        let title = if self.entries.is_empty() {
            format!(" {scope} ")
        } else if self.range_commit.is_some() {
            // The subject already fills the bar; the counts live in the footer.
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
            // The message overlay takes over the list rows while it's open
            // (#93); Esc or `m` closes it and the files come back.
            if let Some(msg) = &self.message {
                let line = msg.get(vis).map(String::as_str).unwrap_or("");
                let l: String = line.chars().take(inner_w.saturating_sub(1)).collect();
                queue!(
                    out,
                    ResetColor,
                    // The subject is the first line; the rest is body/meta.
                    SetForegroundColor(if vis == 0 {
                        Color::White
                    } else {
                        Color::DarkGrey
                    }),
                    Print(" "),
                    Print(&l),
                    Print(" ".repeat(inner_w.saturating_sub(1 + l.chars().count()))),
                    ResetColor,
                    SetForegroundColor(border),
                    Print("│"),
                )?;
                y += 1;
                continue;
            }
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
                        'A' => Color::Green,
                        'R' => Color::Blue,
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

        // Repo-wide totals, right-aligned in the footer so they land under the
        // per-file +/- columns (#91).
        let totals = totals(&self.entries);
        // Width the footer hint must leave free: counts plus their spacing.
        let tw = totals
            .as_ref()
            .map(|(p, m)| p.chars().count() + 1 + m.chars().count() + 1)
            .unwrap_or(0);

        // Footer hint row + bottom border.
        let hint = match (&self.note, self.mode) {
            (Some(n), _) => format!(" {n}"),
            // Esc backs out of a range to the commit list, not straight to
            // closed (#74). With one commit on show, `m` reads its message (#93).
            (None, Mode::Range) if self.range_commit.is_some() => {
                " ↑↓ move · Enter open · m message · Esc back".to_string()
            }
            (None, Mode::Range) => " ↑↓ move · Enter open · Esc commits".to_string(),
            // Log browse works with a clean worktree too — advertise it there,
            // where it's the only thing the panel can still do (#74).
            (None, _) if self.entries.is_empty() => " l commits · Esc close".to_string(),
            (None, _) => " ↑↓ move · Enter open · l commits · Esc close".to_string(),
        };
        let h: String = hint.chars().take(inner_w.saturating_sub(tw)).collect();
        queue!(
            out,
            cursor::MoveTo(rect.x, y),
            SetForegroundColor(border),
            Print("│"),
            SetForegroundColor(Color::DarkGrey),
            Print(&h),
            Print(" ".repeat(inner_w.saturating_sub(tw).saturating_sub(h.chars().count()))),
        )?;
        if let Some((plus, minus)) = &totals {
            queue!(
                out,
                SetForegroundColor(Color::Green),
                Print(plus),
                Print(" "),
                SetForegroundColor(Color::Red),
                Print(minus),
                Print(" "),
            )?;
        }
        queue!(
            out,
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
        // A file's history names the file and its depth; the repo-wide log says
        // how many endpoints are picked, which is what drives its Space/Enter.
        let title = match (self.mode, &self.file_rel) {
            (Mode::FileLog, Some(rel)) => format!(
                " {rel} · {} commit{} ",
                self.commits.len(),
                if self.commits.len() == 1 { "" } else { "s" },
            ),
            _ => format!(
                " commits · {} picked{} ",
                picked,
                match &self.marks[..] {
                    [a] => format!(" ({})", a.label),
                    [a, b] => format!(" ({}, {})", a.label, b.label),
                    _ => String::new(),
                }
            ),
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
        // Widest date in the window, right-aligned like the +/- column.
        let dw = self
            .commits
            .iter()
            .skip(start)
            .take(visible)
            .map(|c| c.when.chars().count())
            .max()
            .unwrap_or(0);
        // A file's history swaps the mark checkbox — which only the repo-wide
        // log's two-endpoint picking needs — for who touched it and how (#92).
        let file_log = self.mode == Mode::FileLog;
        let aw = if file_log {
            self.commits
                .iter()
                .skip(start)
                .take(visible)
                .map(|c| c.author.chars().count())
                .max()
                .unwrap_or(0)
                .min(14) // a long name shouldn't crowd out the subject
        } else {
            0
        };
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
                // Log:     " ❯ [x] sha subject … when "
                // FileLog: " ❯ M sha author subject … when "
                let lead = if file_log { 2 + aw + 1 } else { 4 };
                let fixed = 3 + lead + c.sha.chars().count() + 1 + dw + 1;
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
                if file_log {
                    // What this commit did to the file, coloured like the
                    // changed-file list so the letters read the same way.
                    if !selected {
                        queue!(
                            out,
                            SetForegroundColor(match c.fstatus {
                                'D' => Color::Red,
                                'A' => Color::Green,
                                'R' => Color::Blue,
                                _ => Color::Yellow,
                            })
                        )?;
                    }
                    queue!(out, Print(c.fstatus), Print(" "))?;
                } else {
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
                }
                if !selected {
                    queue!(out, SetForegroundColor(Color::Yellow))?;
                }
                queue!(out, Print(&c.sha), Print(" "))?;
                if file_log {
                    if !selected {
                        queue!(out, SetForegroundColor(Color::Blue))?;
                    }
                    let author = clip_right(&c.author, aw);
                    let apad = aw.saturating_sub(author.chars().count());
                    queue!(out, Print(&author), Print(" ".repeat(apad)), Print(" "))?;
                }
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
            // A file's history has no endpoint picking: Enter is "open this
            // commit", and Esc closes because nothing opened it (#92).
            (None, None) if file_log => " ↑↓ move · Enter open commit · Esc close".to_string(),
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
    let numstat = git(&root, &["diff", "HEAD", "--numstat", "-z", "-M"])
        .or_else(|_| git(&root, &["diff", "--numstat", "-z", "-M"]))?;
    for ns in parse_numstat_z(&numstat) {
        let abs = root.join(&ns.path);
        entries.push(Entry {
            // A rename names both ends, so the move itself is visible (#94).
            rel: match &ns.from {
                Some(from) => format!("{from} → {}", ns.path),
                None => ns.path.clone(),
            },
            status: match (&ns.from, abs.exists()) {
                (Some(_), _) => 'R',
                (None, true) => 'M',
                (None, false) => 'D',
            },
            abs,
            added: ns.added,
            removed: ns.removed,
        });
    }
    if let Ok(unt) = git(&root, &["ls-files", "--others", "--exclude-standard"]) {
        for p in unt.lines().filter(|l| !l.is_empty()) {
            let abs = root.join(p);
            entries.push(Entry {
                rel: p.to_string(),
                added: count_lines(&abs),
                abs,
                // Untracked reads as "added" here (#90); git's own `??` code is
                // an implementation detail of `status --porcelain`, not a label.
                status: 'A',
                removed: Some(0),
            });
        }
    }
    Ok(entries)
}

/// One parsed `--numstat -z` record: counts, the path to act on, and — for a
/// rename — the name it came from (#94).
struct NumStat {
    added: Option<u64>,
    removed: Option<u64>,
    path: String,
    from: Option<String>,
}

/// Parse `git diff --numstat -z`. The tab-delimited form can't express a
/// rename; with `-z` the pathname field is emitted NUL-terminated, and a rename
/// leaves that field empty and follows with two more NUL-terminated names:
/// `0\t0\t\0old\0new\0`. So renames are only visible in this encoding.
fn parse_numstat_z(raw: &str) -> Vec<NumStat> {
    let mut out = Vec::new();
    let mut it = raw.split('\0');
    while let Some(rec) = it.next() {
        if rec.is_empty() {
            continue;
        }
        let mut f = rec.splitn(3, '\t');
        let (Some(a), Some(d), Some(p)) = (f.next(), f.next(), f.next()) else {
            continue;
        };
        let (added, removed) = (a.parse().ok(), d.parse().ok());
        if p.is_empty() {
            // Rename or copy: the two names follow as their own records.
            let (Some(from), Some(to)) = (it.next(), it.next()) else {
                continue;
            };
            out.push(NumStat {
                added,
                removed,
                path: to.to_string(),
                from: Some(from.to_string()),
            });
        } else {
            out.push(NumStat {
                added,
                removed,
                path: p.to_string(),
                from: None,
            });
        }
    }
    out
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
                author: String::new(),
                fstatus: ' ',
            })
        })
        .collect())
}

/// Commits that touched `rel`, newest first (#92). `--follow` keeps the history
/// going across renames; the per-commit `--name-status` letter says what that
/// commit did to the file. Records are separated by RS and fields by US so a
/// subject or an author name containing anything still splits cleanly.
fn load_file_log(root: &Path, rel: &str) -> Result<Vec<Commit>, String> {
    let out = git(
        root,
        &[
            "log",
            "--follow",
            "--name-status",
            "--pretty=format:\x1e%h\x1f%an\x1f%s\x1f%cr",
            "-n",
            &LOG_LIMIT.to_string(),
            "--",
            rel,
        ],
    )?;
    Ok(out
        .split('\x1e')
        .filter(|r| !r.trim().is_empty())
        .filter_map(|rec| {
            let mut lines = rec.lines();
            let mut it = lines.next()?.splitn(4, '\x1f');
            let (sha, author, subject, when) = (it.next()?, it.next()?, it.next()?, it.next()?);
            // The name-status lines follow the header; with --follow a rename
            // reads `R100\told\tnew`, so take the letter only.
            let fstatus = lines
                .find(|l| !l.trim().is_empty())
                .and_then(|l| l.chars().next())
                .unwrap_or(' ');
            Some(Commit {
                sha: sha.to_string(),
                subject: subject.to_string(),
                when: when.to_string(),
                author: author.to_string(),
                fstatus,
            })
        })
        .collect())
}

/// The full message of `sha` for the overlay (#93): subject, body, author and
/// date, as display lines.
fn commit_message(root: &Path, sha: &str) -> Result<Vec<String>, String> {
    let out = git(
        root,
        &[
            "show",
            "-s",
            "--format=%s%n%n%an · %ad%n%n%b",
            "--date=short",
            sha,
        ],
    )?;
    Ok(out.lines().map(|l| l.to_string()).collect())
}

/// Changed files between two revisions (#74). Same shape as the working-tree
/// list, so the panel renders and opens them identically; `abs` is the path in
/// the working tree, which the diff view resolves against the two revs.
fn load_range(root: &Path, base: &str, head: &str) -> Result<Vec<Entry>, String> {
    let numstat = git(root, &["diff", "--numstat", "-z", "-M", base, head])?;
    let status = git(root, &["diff", "--name-status", base, head]).unwrap_or_default();
    let mut entries = Vec::new();
    for ns in parse_numstat_z(&numstat) {
        // A rename is already known from the `-z` encoding (#94); for the rest,
        // git's status letter says A added, D deleted, M modified.
        let st = match &ns.from {
            Some(_) => 'R',
            None => status
                .lines()
                .find(|l| l.split('\t').nth(1) == Some(ns.path.as_str()))
                .and_then(|l| l.chars().next())
                .unwrap_or('M'),
        };
        entries.push(Entry {
            rel: match &ns.from {
                Some(from) => format!("{from} → {}", ns.path),
                None => ns.path.clone(),
            },
            abs: root.join(&ns.path),
            status: st,
            added: ns.added,
            removed: ns.removed,
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
            file_rel: None,
            range_from: Mode::Log,
            range_commit: None,
            message: None,
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

    /// Terminals in application-cursor mode send SS3 arrows (`ESC O A`), not
    /// CSI. Treating the `O` as an unknown byte made every arrow press read as
    /// Esc and step the panel back, so only j/k appeared to work (#74).
    /// An arrow carrying a modifier parameter still moves the selection, and
    /// its digits don't leak into the list as keystrokes (#74).
    #[test]
    fn modified_arrows_move_the_selection() {
        let mut dl = fixture(3);
        assert!(matches!(dl.feed_bytes(b"\x1b[1;29B"), DiffAction::None));
        assert_eq!(dl.sel, 1);
        assert!(matches!(dl.feed_bytes(b"\x1b[1;5A"), DiffAction::None));
        assert_eq!(dl.sel, 0);
        assert!(matches!(dl.mode, Mode::Worktree), "and never dismisses");
    }

    #[test]
    fn ss3_arrows_move_the_selection() {
        let mut dl = fixture(3);
        assert!(matches!(dl.feed_bytes(b"\x1bOB"), DiffAction::None));
        assert_eq!(dl.sel, 1, "SS3 down moves the selection");
        assert!(matches!(dl.feed_bytes(b"\x1bOA"), DiffAction::None));
        assert_eq!(dl.sel, 0, "SS3 up moves it back");
        // The panel is still open — an arrow must not dismiss it.
        assert!(matches!(dl.mode, Mode::Worktree));

        // Same in the commit list.
        dl.commits = (0..3)
            .map(|i| Commit {
                sha: format!("sha{i}"),
                subject: "s".into(),
                when: "now".into(),
                author: "someone".into(),
                fstatus: 'M',
            })
            .collect();
        dl.mode = Mode::Log;
        dl.feed_bytes(b"\x1bOB");
        assert_eq!(dl.sel, 1);
        assert!(matches!(dl.mode, Mode::Log), "arrows don't step back");
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
                author: "someone".into(),
                fstatus: 'M',
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
                author: "someone".into(),
                fstatus: 'M',
            })
            .collect();
        dl.mode = Mode::Log;
        dl.scroll(99);
        assert_eq!(dl.sel, 19, "scroll clamps to the last commit");
        assert_eq!(dl.window(5), (15, 5));
    }
}

#[cfg(test)]
mod totals_tests {
    use super::*;

    fn e(added: Option<u64>, removed: Option<u64>) -> Entry {
        Entry {
            rel: "f".into(),
            abs: PathBuf::from("/f"),
            status: 'M',
            added,
            removed,
        }
    }

    #[test]
    fn totals_sum_every_entry_not_just_the_visible_ones() {
        let entries = vec![
            e(Some(10), Some(3)),
            e(Some(1), Some(0)),
            e(Some(4), Some(7)),
        ];
        let (plus, minus) = totals(&entries).unwrap();
        assert_eq!(plus, "+15");
        assert_eq!(minus, "-10");
    }

    /// A binary or too-large-to-count file must not read as 0 — the total says
    /// `?` so an under-report is visible rather than silent.
    #[test]
    fn uncounted_entries_mark_the_total_rather_than_counting_as_zero() {
        let entries = vec![e(Some(5), Some(2)), e(None, None)];
        let (plus, minus) = totals(&entries).unwrap();
        assert_eq!(plus, "+5?");
        assert_eq!(minus, "-2?");
    }

    /// The two sides are marked independently: a deleted-only unknown shouldn't
    /// cast doubt on the added total.
    #[test]
    fn the_unknown_marker_is_per_side() {
        let entries = vec![e(Some(9), None)];
        let (plus, minus) = totals(&entries).unwrap();
        assert_eq!(plus, "+9");
        assert_eq!(minus, "-0?");
    }

    #[test]
    fn no_totals_when_there_is_nothing_to_total() {
        assert!(totals(&[]).is_none());
    }

    /// The `-z` encoding is the only one that can express a rename: the path
    /// field is empty and the two names follow as their own records (#94).
    #[test]
    fn numstat_z_reads_renames_and_ordinary_paths() {
        let raw = "1\t2\tsrc/a.rs\0".to_string()
            + "0\t0\t\0old/name.rs\0new/name.rs\0"
            + "5\t0\tsrc/b.rs\0";
        let recs = parse_numstat_z(&raw);
        assert_eq!(recs.len(), 3);
        assert_eq!(recs[0].path, "src/a.rs");
        assert!(recs[0].from.is_none());
        assert_eq!(recs[1].path, "new/name.rs", "the rename's destination");
        assert_eq!(recs[1].from.as_deref(), Some("old/name.rs"));
        assert_eq!(recs[2].path, "src/b.rs", "parsing resyncs after a rename");
        assert_eq!(recs[2].added, Some(5));
    }

    /// A binary file's counts are `-`, which must read as unknown, not 0.
    #[test]
    fn numstat_z_keeps_binary_counts_unknown() {
        let recs = parse_numstat_z("-\t-\timg.png\0");
        assert_eq!(recs[0].added, None);
        assert_eq!(recs[0].removed, None);
    }
}

#[cfg(test)]
mod range_tests {
    use super::*;
    use std::process::Command as C;

    fn git_in(dir: &Path, args: &[&str]) {
        let out = C::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }

    /// A staged rename reads as R and names both ends (#94). An unstaged move
    /// can't: git compares against the index, so it is a delete plus an
    /// untracked file until staged — asserted here so the limit is deliberate.
    #[test]
    fn a_staged_rename_shows_as_r_an_unstaged_move_does_not() {
        let dir = std::env::temp_dir().join(format!("cf-rename-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q"]);
        git_in(&dir, &["config", "user.email", "t@t"]);
        git_in(&dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-qm", "first"]);

        git_in(&dir, &["mv", "a.txt", "b.txt"]);
        let e = load(&dir).unwrap();
        assert_eq!(
            e.len(),
            1,
            "a rename is one entry, not a delete plus an add"
        );
        assert_eq!(e[0].status, 'R');
        assert_eq!(e[0].rel, "a.txt → b.txt", "both ends are named");
        assert_eq!(e[0].abs, dir.join("b.txt"), "opening it uses the new path");

        // Unstaged: git sees a delete and an unrelated untracked file.
        git_in(&dir, &["commit", "-qm", "rename"]);
        std::fs::rename(dir.join("b.txt"), dir.join("c.txt")).unwrap();
        let e = load(&dir).unwrap();
        let mut got: Vec<char> = e.iter().map(|x| x.status).collect();
        got.sort_unstable();
        assert_eq!(got, vec!['A', 'D'], "unstaged move is D + untracked");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A file's history (#92): only the commits touching that file, newest
    /// first, each carrying who did it and what it did — then Enter opens that
    /// commit's *whole* change, not just this file, and Esc closes outright
    /// because nothing opened the list underneath.
    #[test]
    fn file_history_lists_only_that_files_commits_and_opens_the_whole_commit() {
        let dir = std::env::temp_dir().join(format!("cf-flog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q"]);
        git_in(&dir, &["config", "user.email", "t@t"]);
        git_in(&dir, &["config", "user.name", "Ada"]);
        // a.txt is touched twice; b.txt once, in a commit that also touches a.
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-qm", "add a"]);
        std::fs::write(dir.join("c.txt"), "c\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-qm", "add c only"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        std::fs::write(dir.join("b.txt"), "b\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-qm", "touch a and b"]);

        let mut dl = DiffList::new_file_log(&dir, &dir.join("a.txt"));
        assert!(matches!(dl.mode, Mode::FileLog), "opens on the file's log");
        assert_eq!(dl.commits.len(), 2, "the c-only commit is not a's history");
        assert_eq!(dl.commits[0].subject, "touch a and b", "newest first");
        assert_eq!(dl.commits[0].author, "Ada", "who touched it");
        assert_eq!(dl.commits[1].fstatus, 'A', "the commit that added it");

        // Enter opens that commit vs its parent — every file it changed.
        dl.feed_bytes(b"\r");
        assert!(matches!(dl.mode, Mode::Range));
        let mut rels: Vec<&str> = dl.entries.iter().map(|e| e.rel.as_str()).collect();
        rels.sort();
        assert_eq!(rels, vec!["a.txt", "b.txt"], "the whole commit, not just a");
        assert!(dl.range_commit.is_some(), "one commit, so it has a message");

        // Esc walks back to the file's history, then closes from there.
        assert!(matches!(dl.feed_bytes(b"\x1b"), DiffAction::None));
        assert!(matches!(dl.mode, Mode::FileLog), "back to the history");
        assert!(matches!(dl.feed_bytes(b"\x1b"), DiffAction::Cancel));

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `m` shows the message only when the range is a single commit (#93); a
    /// two-endpoint span has no one message to show.
    #[test]
    fn the_message_overlay_needs_a_single_commit() {
        let dir = std::env::temp_dir().join(format!("cf-msg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q"]);
        git_in(&dir, &["config", "user.email", "t@t"]);
        git_in(&dir, &["config", "user.name", "Ada"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-qm", "first"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        git_in(&dir, &["commit", "-qam", "second subject\n\nthe body"]);

        // One marked commit: the message is unambiguous.
        let mut dl = DiffList::new(&dir);
        dl.feed_bytes(b"l");
        dl.feed_bytes(b" ");
        dl.feed_bytes(b"\r");
        assert!(matches!(dl.mode, Mode::Range));
        dl.feed_bytes(b"m");
        let msg = dl.message.as_ref().expect("m opens the message");
        assert_eq!(msg[0], "second subject");
        assert!(msg.iter().any(|l| l.contains("Ada")), "author is shown");
        assert!(msg.iter().any(|l| l == "the body"), "body is shown");
        // Esc closes the overlay first, leaving the range up.
        dl.feed_bytes(b"\x1b");
        assert!(dl.message.is_none());
        assert!(
            matches!(dl.mode, Mode::Range),
            "esc closed only the overlay"
        );

        // A two-endpoint span spans commits, so there is no single message.
        let mut dl = DiffList::new(&dir);
        dl.feed_bytes(b"l");
        dl.feed_bytes(b" ");
        dl.feed_bytes(b"j");
        dl.feed_bytes(b" ");
        dl.feed_bytes(b"\r");
        assert!(dl.range_commit.is_none(), "a span names no single commit");
        dl.feed_bytes(b"m");
        assert!(dl.message.is_none(), "m does nothing for a span");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Driving the panel the way a user does: `l` to the commit list, mark two
    /// commits, Enter to diff them, then Enter on a file must hand back an Open
    /// carrying the range (#74).
    #[test]
    fn enter_on_a_file_in_a_range_opens_it() {
        let dir = std::env::temp_dir().join(format!("cf-range-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        git_in(&dir, &["init", "-q"]);
        git_in(&dir, &["config", "user.email", "t@t"]);
        git_in(&dir, &["config", "user.name", "t"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git_in(&dir, &["add", "."]);
        git_in(&dir, &["commit", "-qm", "first"]);
        std::fs::write(dir.join("a.txt"), "two\n").unwrap();
        git_in(&dir, &["commit", "-qam", "second"]);

        let mut dl = DiffList::new(&dir);
        dl.feed_bytes(b"l");
        assert!(matches!(dl.mode, Mode::Log), "l enters the commit list");
        assert_eq!(dl.commits.len(), 2);
        dl.feed_bytes(b" ");
        dl.feed_bytes(b"j");
        dl.feed_bytes(b" ");
        assert_eq!(dl.marks.len(), 2);
        dl.feed_bytes(b"\r");
        assert!(
            matches!(dl.mode, Mode::Range),
            "enter diffs the marks: {:?}",
            dl.note
        );
        assert_eq!(dl.entries.len(), 1, "a.txt changed between them");

        match dl.feed_bytes(b"\r") {
            DiffAction::Open { path, rev } => {
                assert_eq!(path, dir.join("a.txt"));
                assert!(rev.is_some(), "the open carries the range");
            }
            _ => panic!("enter on a file must open it; note={:?}", dl.note),
        }

        // Closing the full-screen diff refreshes the panel in place: you come
        // back to the range you were browsing, not the working-tree list (#74).
        dl.refresh();
        assert!(matches!(dl.mode, Mode::Range), "still on the range");
        assert_eq!(dl.entries.len(), 1);
        assert!(dl.range.is_some(), "the picked commits survive");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
