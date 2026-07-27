//! User configuration (#5), loaded from `~/.config/codeforge/config.toml`.
//!
//! Everything has a default, so a missing or partial file still works. On first
//! run we write a commented default file the user can edit. Keys are single
//! characters pressed after the prefix; the prefix itself is written like
//! `"C-a"`.

use std::path::PathBuf;

use serde::Deserialize;

/// Top-level config.
#[derive(Debug, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Command prefix, e.g. `"C-a"` or a single char.
    pub prefix: String,
    /// Override for the projects root the picker lists.
    pub projects_root: Option<String>,
    /// Editor command line for the left pane (tokenised on spaces).
    pub editor: String,
    /// Shell command for the top-right pane; defaults to `$SHELL` when unset.
    pub shell: Option<String>,
    /// AI command line for the bottom-right pane.
    pub ai: String,
    /// Fraction of width given to the editor pane.
    pub editor_ratio: f32,
    /// Fraction of the right column given to the shell (vs the AI) pane.
    pub right_ratio: f32,
    /// Location for the status-bar temperature (empty disables it).
    pub weather: String,
    /// Editor line wrapping. When false, CodeForge starts nvim with `nowrap`.
    pub wrap: bool,
    /// Autosave edits in the editor (nvim writes on change). Default on.
    pub autosave: bool,
    /// Which panes open when a project window is first created (#17). At least
    /// one must be true; if all are false the editor is forced on.
    pub start_editor: bool,
    pub start_terminal: bool,
    pub start_ai: bool,
    /// Status-bar right side toggles (#16), left-to-right order when shown:
    /// metrics (cpu/ram/disk), weather, date, clock.
    pub status_metrics: bool,
    pub status_weather: bool,
    pub status_date: bool,
    pub status_clock: bool,
    /// Per-action keybindings (single chars, pressed after the prefix).
    pub keys: Keys,
    /// Editor (nvim) keybindings. Unlike `keys` these are full chords passed
    /// through to Neovim (e.g. `"C-p"`, `"Space e"`, `"]b"`), so the editor keys
    /// shown on the splash and used by the finder/explorer come from one place.
    pub editor_keys: EditorKeys,
}

/// Editor-side keybindings, injected into nvim via the `CODEFORGE_EDITOR_KEYS`
/// env var and read by `config/nvim/init.lua`. Tokens are chords, not single
/// chars: `"C-p"` (ctrl), `"C-S-f"` (ctrl-shift), `"Space e"` (leader), or a
/// literal like `"]b"`. Standard Vim/LSP keys (gd, gr, K, …) stay fixed in
/// init.lua and are intentionally not configurable here.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct EditorKeys {
    /// Fuzzy-open a file by name.
    pub open_file: String,
    /// Fuzzy-search within the current file.
    pub search_in_file: String,
    /// Live-grep across the repo.
    pub search_repo: String,
    /// Toggle the file explorer (oil).
    pub explorer: String,
    /// Close the current editor buffer (tab).
    pub close_tab: String,
    /// Git file history for the current file.
    pub file_history: String,
}

impl Default for EditorKeys {
    fn default() -> Self {
        EditorKeys {
            open_file: "Ctrl-p".into(),
            search_in_file: "Ctrl-f".into(),
            search_repo: "Ctrl-Shift-f".into(),
            explorer: "Space e".into(),
            close_tab: "Space b d".into(),
            file_history: "Space g h".into(),
        }
    }
}

impl EditorKeys {
    /// Serialize as newline-separated `name=token` lines for the env var that
    /// init.lua parses. Values are simple chords with no `=` or newline, so this
    /// needs no escaping.
    pub fn env_string(&self) -> String {
        [
            ("open_file", &self.open_file),
            ("search_in_file", &self.search_in_file),
            ("search_repo", &self.search_repo),
            ("explorer", &self.explorer),
            ("close_tab", &self.close_tab),
            ("file_history", &self.file_history),
        ]
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// Current token bound to `field`, if `field` names an editor action.
    pub fn get(&self, field: &str) -> Option<&str> {
        Some(match field {
            "open_file" => &self.open_file,
            "search_in_file" => &self.search_in_file,
            "search_repo" => &self.search_repo,
            "explorer" => &self.explorer,
            "close_tab" => &self.close_tab,
            "file_history" => &self.file_history,
            _ => return None,
        })
    }

    /// Set `field`'s token. Returns false if `field` isn't a known editor action.
    pub fn set(&mut self, field: &str, tok: String) -> bool {
        match field {
            "open_file" => self.open_file = tok,
            "search_in_file" => self.search_in_file = tok,
            "search_repo" => self.search_repo = tok,
            "explorer" => self.explorer = tok,
            "close_tab" => self.close_tab = tok,
            "file_history" => self.file_history = tok,
            _ => return false,
        }
        true
    }
}

/// Editor-key actions, in the order shown in the `Ctrl-a ?` overlay:
/// `(config field, human label)`. Edited via a typed token (persisted to the
/// `[editor_keys]` block) rather than a single char, and applied on reload.
pub const EDITOR_EDITABLE: [(&str, &str); 6] = [
    ("open_file", "open file"),
    ("search_in_file", "search in file"),
    ("search_repo", "search repo (grep)"),
    ("explorer", "file explorer"),
    ("close_tab", "close editor tab"),
    ("file_history", "git file history"),
];

/// Validate an editor-key token typed in the overlay; returns the trimmed token.
/// Kept permissive (chords like `C-p`, `Space e`, or literals like `]b`) but
/// rejects characters that would break the TOML string or the env line we emit.
pub fn validate_editor_token(tok: &str) -> Result<String, String> {
    let t = tok.trim();
    if t.is_empty() {
        return Err("empty — type a key like C-p or Space e".into());
    }
    if t.chars().count() > 16 {
        return Err("too long".into());
    }
    if t.contains(['"', '\\', '=', '\n', '\r']) {
        return Err("no \" \\ = or newline".into());
    }
    Ok(t.to_string())
}

/// Human display for a key token: `"C-p"` -> `"Ctrl-p"`, `"C-S-f"` ->
/// `"Ctrl-Shift-f"`, `"Space e"` and literals like `"]b"` unchanged.
pub fn disp_token(tok: &str) -> String {
    tok.replace("C-", "Ctrl-")
        .replace("S-", "Shift-")
        .replace("A-", "Alt-")
        .replace("M-", "Meta-")
}

/// Single-character keybindings pressed after the prefix.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(default)]
pub struct Keys {
    pub focus_left: char,
    pub focus_down: char,
    pub focus_up: char,
    pub focus_right: char,
    pub cycle: char,
    /// Show/hide the editor, terminal, and AI panes.
    pub toggle_editor: char,
    pub toggle_shell: char,
    pub toggle_ai: char,
    pub picker: char,
    pub help: char,
    pub quit: char,
    /// New window (opens the picker to choose its project).
    pub win_new: char,
    /// Close the current window (kills all its panes).
    pub win_close: char,
    /// Detach the client (leaves the server running in the background).
    pub detach: char,
    /// Reload: restart the server on the latest build, reopening the same
    /// project windows (pane contents reset).
    pub reload: char,
    /// Forget the saved session so the next `forge` starts fresh (the picker).
    pub fresh: char,
    /// New child in the focused slot (another terminal / Claude session).
    pub tab_new: char,
    /// Cycle to the next / previous child in the focused slot.
    pub tab_next: char,
    pub tab_prev: char,
    /// Close the active child in the focused slot.
    pub tab_close: char,
    /// Enter copy/scroll mode on the focused pane (scroll its scrollback, select
    /// text, copy). Esc/q exits.
    pub copy: char,
    /// Toggle the git-diff list (#18): changed files with +/- counts; picking
    /// one opens a full-window side-by-side diff in the editor.
    pub git_diff: char,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            prefix: "C-a".into(),
            projects_root: None,
            editor: "nvim".into(),
            shell: None,
            ai: "claude".into(),
            editor_ratio: 0.5,
            right_ratio: 0.5,
            weather: "Colorado Springs".into(),
            wrap: true,
            autosave: true,
            start_editor: true,
            start_terminal: true,
            start_ai: true,
            status_metrics: true,
            status_weather: true,
            status_date: true,
            status_clock: true,
            keys: Keys::default(),
            editor_keys: EditorKeys::default(),
        }
    }
}

impl Default for Keys {
    fn default() -> Self {
        Keys {
            focus_left: 'h',
            focus_down: 'j',
            focus_up: 'k',
            focus_right: 'l',
            cycle: 'o',
            toggle_editor: 'e',
            toggle_shell: 't',
            toggle_ai: 'c',
            picker: 'p',
            help: '?',
            quit: 'q',
            win_new: 'n',
            win_close: 'X',
            detach: 'd',
            reload: 'r',
            fresh: 'F',
            tab_new: 's',
            tab_next: ']',
            tab_prev: '[',
            tab_close: 'w',
            copy: 'v',
            git_diff: 'g',
        }
    }
}

/// The rebindable actions, in the order shown in the `Ctrl-a ?` editor:
/// `(config field, human label)`. Focus keys are listed individually so each
/// can be rebound. `1..9` (window jump) and mouse aren't rebindable.
pub const EDITABLE: [(&str, &str); 22] = [
    ("toggle_editor", "show/hide editor"),
    ("toggle_shell", "show/hide terminal"),
    ("toggle_ai", "show/hide Claude"),
    ("git_diff", "git diff (changed files)"),
    ("tab_new", "new terminal/Claude tab"),
    ("tab_next", "next tab (focused slot)"),
    ("tab_prev", "prev tab (focused slot)"),
    ("tab_close", "close tab (focused slot)"),
    ("copy", "copy/scroll mode"),
    ("focus_left", "focus left"),
    ("focus_down", "focus down"),
    ("focus_up", "focus up"),
    ("focus_right", "focus right"),
    ("cycle", "cycle focus"),
    ("picker", "switch project"),
    ("win_new", "new window"),
    ("win_close", "close window"),
    ("detach", "detach (stays alive)"),
    ("reload", "reload (new build)"),
    ("fresh", "forget saved session"),
    ("quit", "quit (ends session)"),
    ("help", "toggle this help"),
];

impl Keys {
    /// Current key bound to `field`, if `field` names an action.
    pub fn get(&self, field: &str) -> Option<char> {
        Some(match field {
            "focus_left" => self.focus_left,
            "focus_down" => self.focus_down,
            "focus_up" => self.focus_up,
            "focus_right" => self.focus_right,
            "cycle" => self.cycle,
            "toggle_editor" => self.toggle_editor,
            "toggle_shell" => self.toggle_shell,
            "toggle_ai" => self.toggle_ai,
            "picker" => self.picker,
            "help" => self.help,
            "quit" => self.quit,
            "win_new" => self.win_new,
            "win_close" => self.win_close,
            "detach" => self.detach,
            "reload" => self.reload,
            "fresh" => self.fresh,
            "tab_new" => self.tab_new,
            "tab_next" => self.tab_next,
            "tab_prev" => self.tab_prev,
            "tab_close" => self.tab_close,
            "copy" => self.copy,
            "git_diff" => self.git_diff,
            _ => return None,
        })
    }

    /// All bindings as newline-separated `action=char` lines, for the env var
    /// the splash reads so its prefix-key cheatsheet stays live after a rebind
    /// (#28).
    pub fn env_string(&self) -> String {
        self.bindings()
            .iter()
            .map(|(name, ch)| format!("{name}={ch}"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Set `field`'s key. Returns false if `field` isn't a known action.
    pub fn set(&mut self, field: &str, ch: char) -> bool {
        match field {
            "focus_left" => self.focus_left = ch,
            "focus_down" => self.focus_down = ch,
            "focus_up" => self.focus_up = ch,
            "focus_right" => self.focus_right = ch,
            "cycle" => self.cycle = ch,
            "toggle_editor" => self.toggle_editor = ch,
            "toggle_shell" => self.toggle_shell = ch,
            "toggle_ai" => self.toggle_ai = ch,
            "picker" => self.picker = ch,
            "help" => self.help = ch,
            "quit" => self.quit = ch,
            "win_new" => self.win_new = ch,
            "win_close" => self.win_close = ch,
            "detach" => self.detach = ch,
            "reload" => self.reload = ch,
            "fresh" => self.fresh = ch,
            "tab_new" => self.tab_new = ch,
            "tab_next" => self.tab_next = ch,
            "tab_prev" => self.tab_prev = ch,
            "tab_close" => self.tab_close = ch,
            "copy" => self.copy = ch,
            "git_diff" => self.git_diff = ch,
            _ => return false,
        }
        true
    }

    /// Validate binding `field` -> `ch` and return the resulting key set.
    /// Rejects reserved/unsafe keys and anything that would collide with
    /// another action (which would leave one of them dead).
    pub fn with_bind(&self, field: &str, ch: char) -> Result<Keys, String> {
        if ch.is_ascii_digit() {
            return Err("digits 1-9 are reserved for window switching".into());
        }
        // `"` and `\` would break the TOML string we persist; space is invisible.
        if !ch.is_ascii_graphic() || ch == '"' || ch == '\\' {
            return Err("pick a visible key (no space, quote, or backslash)".into());
        }
        let mut k = *self;
        if !k.set(field, ch) {
            return Err(format!("unknown action '{field}'"));
        }
        for (name, c) in k.bindings() {
            if name != field && c == ch {
                return Err(format!("'{ch}' is already bound to {name}"));
            }
        }
        Ok(k)
    }

    /// All (action, key) bindings, for help display and conflict checking.
    fn bindings(&self) -> [(&'static str, char); 22] {
        [
            ("focus_left", self.focus_left),
            ("focus_down", self.focus_down),
            ("focus_up", self.focus_up),
            ("focus_right", self.focus_right),
            ("cycle", self.cycle),
            ("toggle_editor", self.toggle_editor),
            ("toggle_shell", self.toggle_shell),
            ("toggle_ai", self.toggle_ai),
            ("picker", self.picker),
            ("help", self.help),
            ("quit", self.quit),
            ("win_new", self.win_new),
            ("win_close", self.win_close),
            ("detach", self.detach),
            ("reload", self.reload),
            ("fresh", self.fresh),
            ("tab_new", self.tab_new),
            ("tab_next", self.tab_next),
            ("tab_prev", self.tab_prev),
            ("tab_close", self.tab_close),
            ("copy", self.copy),
            ("git_diff", self.git_diff),
        ]
    }

    /// Human-readable warnings about the keybindings: two actions on the same
    /// key (one would be dead), or a digit key (reserved for window switching).
    pub fn conflicts(&self) -> Vec<String> {
        let b = self.bindings();
        let mut msgs = Vec::new();
        for i in 0..b.len() {
            for j in (i + 1)..b.len() {
                if b[i].1 == b[j].1 {
                    msgs.push(format!(
                        "keys '{}' and '{}' both use '{}' (one will be ignored)",
                        b[i].0, b[j].0, b[i].1
                    ));
                }
            }
        }
        for (name, ch) in b {
            if ch.is_ascii_digit() {
                msgs.push(format!(
                    "key '{name}' uses digit '{ch}', reserved for window switching (prefix 1..9)"
                ));
            }
        }
        msgs
    }
}

impl Config {
    /// Load config, returning it plus an optional warning to print (e.g. a parse
    /// error). A missing file yields defaults with no warning, and writes a
    /// commented default file for the user to edit.
    pub fn load() -> (Config, Option<String>) {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(s) => match toml::from_str::<Config>(&s) {
                Ok(c) => {
                    // Guard the prefix: a spec that resolves to a flow-control or
                    // NUL byte (e.g. "C-s", "C-space") would freeze or dead-key
                    // the whole session with no way to recover from inside it.
                    // Warn and fall back to Ctrl-a rather than lock the user out.
                    let warn = parse_prefix(&c.prefix)
                        .err()
                        .map(|e| format!("prefix \"{}\": {e}; using Ctrl-a instead", c.prefix));
                    // Older/partial configs are missing keys added since they were
                    // written (e.g. tab_close, the whole [editor_keys] block). A
                    // rebind of a key with no line on disk can't be persisted and
                    // silently reverts to its default on reload. Append any missing
                    // fields now so every key exists on disk and stays rebindable.
                    ensure_config_complete(&c);
                    (c, warn)
                }
                Err(e) => (
                    Config::default(),
                    Some(format!(
                        "config error in {}: {e} (using defaults)",
                        path.display()
                    )),
                ),
            },
            Err(_) => {
                // No file: write a commented default, best-effort.
                if let Some(dir) = path.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&path, DEFAULT_TOML);
                (Config::default(), None)
            }
        }
    }

    /// The prefix as a raw byte (e.g. `"C-a"` -> 0x01). An unusable spec (see
    /// `parse_prefix`) falls back to Ctrl-a so the session is never un-drivable.
    pub fn prefix_byte(&self) -> u8 {
        parse_prefix(&self.prefix).unwrap_or(0x01)
    }
}

/// Append any `[keys]`/`[editor_keys]` fields missing from the on-disk config,
/// with their current (loaded-or-default) values, so every key has a line and
/// stays rebindable. Present fields (and the user's values/comments) are left
/// untouched. Best-effort; a one-time upgrade for configs written by older
/// versions.
fn ensure_config_complete(c: &Config) {
    let Ok(text) = std::fs::read_to_string(config_path()) else {
        return;
    };
    let has = |field: &str| {
        text.lines().any(|l| {
            l.trim_start()
                .strip_prefix(field)
                .is_some_and(|r| r.trim_start().starts_with('='))
        })
    };
    for (field, ch) in c.keys.bindings() {
        if !has(field) {
            persist_keybind(field, ch);
        }
    }
    for (field, _) in EDITOR_EDITABLE.iter() {
        if !has(field) {
            persist_editor_key(field, c.editor_keys.get(field).unwrap_or(""));
        }
    }
}

/// `~/.config/codeforge/config.toml`, honoring `$XDG_CONFIG_HOME`.
pub fn config_path() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        });
    base.join("codeforge").join("config.toml")
}

/// Rewrite one `field = "x"` line in `config.toml` so a live rebind survives a
/// restart. Best-effort: preserves the rest of the file (other keys, comments);
/// the edited line loses any inline comment. No-op if the file or line is
/// absent. `ch` is pre-validated (`Keys::with_bind`) to be TOML-safe.
pub fn persist_keybind(field: &str, ch: char) {
    persist_kv(field, &ch.to_string(), "[keys]");
}

/// Persist an editor keybinding into the `[editor_keys]` block, creating the
/// block if an older config predates it (#28).
pub fn persist_editor_key(field: &str, token: &str) {
    persist_kv(field, token, "[editor_keys]");
}

/// Rewrite `field = "value"` in config.toml. If the field's line is absent,
/// insert it under `section` (creating the section at EOF when missing). Field
/// names are unique across `[keys]`/`[editor_keys]`, so matching the first
/// occurrence is safe. Any trailing comment on the rewritten line is dropped.
fn persist_kv(field: &str, value: &str, section: &str) {
    let path = config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let mut out = String::with_capacity(text.len() + 32);
    let mut done = false;
    // Byte offset in `out` just past the section header line, for insertion when
    // the field itself is missing.
    let mut section_at: Option<usize> = None;
    for line in text.lines() {
        if !done {
            let trimmed = line.trim_start();
            // Match the assignment line for exactly this field: `field <ws> =`.
            if let Some(rest) = trimmed.strip_prefix(field) {
                if rest.trim_start().starts_with('=') {
                    let indent = &line[..line.len() - trimmed.len()];
                    out.push_str(indent);
                    out.push_str(field);
                    out.push_str(" = \"");
                    out.push_str(value);
                    out.push_str("\"\n");
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
        if line.trim() == section {
            section_at = Some(out.len());
        }
    }
    if !done {
        let assign = format!("{field} = \"{value}\"\n");
        match section_at {
            Some(pos) => out.insert_str(pos, &assign),
            None => {
                if !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str(section);
                out.push('\n');
                out.push_str(&assign);
            }
        }
    }
    let _ = std::fs::write(&path, out);
}

/// Parse a prefix spec into the raw byte the terminal sends.
///
/// `"C-a"` -> 0x01, `"C-b"` -> 0x02; the literal `"space"` -> 0x20; a bare
/// single char is taken literally. Returns `Err(reason)` for a spec that would
/// make the session un-drivable, so the caller can warn and fall back to Ctrl-a:
///   - `Ctrl-S` / `Ctrl-Q` (0x13 / 0x11) are terminal flow-control — they freeze
///     and unfreeze screen output, so using one as a prefix hangs the terminal.
///   - anything resolving to NUL (0x00), e.g. `"C-space"`, which most terminals
///     don't send (and the input parser treats NUL as "no prefix" anyway).
fn parse_prefix(s: &str) -> Result<u8, String> {
    let s = s.trim();
    let byte = if let Some(rest) = s.strip_prefix("C-").or_else(|| s.strip_prefix("c-")) {
        let rest = rest.trim();
        let base = if rest.eq_ignore_ascii_case("space") {
            b' '
        } else if let Some(c) = rest.bytes().next() {
            c
        } else {
            return Err("nothing after \"C-\"".into());
        };
        // Ctrl masks bits 6/7: 'a' (0x61) -> 0x01; Space (0x20) -> 0x00.
        base & 0x1f
    } else if s.eq_ignore_ascii_case("space") {
        b' '
    } else if let Some(c) = s.bytes().next() {
        c
    } else {
        return Err("empty prefix".into());
    };
    match byte {
        0x00 => Err("resolves to NUL, which terminals don't reliably send".into()),
        0x11 | 0x13 => Err("Ctrl-S/Ctrl-Q are terminal flow-control and freeze the screen".into()),
        b => Ok(b),
    }
}

/// Written to `config.toml` on first run.
const DEFAULT_TOML: &str = r#"# CodeForge configuration.
# Edit and restart forge. Every field is optional.

# Command prefix (tmux-style). Examples: "C-a", "C-b", "C-o".
# Avoid Ctrl-S / Ctrl-Q (terminal flow-control — they freeze the screen) and
# Ctrl-Space (sends NUL); those fall back to Ctrl-a with a startup warning.
prefix = "C-a"

# Where the project picker looks. Defaults to $DDN_PROJECTS, then ~/projects.
# projects_root = "/home/you/projects"

# Startup panes. `editor` and `ai` are command lines (split on spaces).
# The editor opens the project dir (or, on restore, your previously open files).
editor = "nvim"
ai = "claude"           # any AI CLI, e.g. "augment"; claude gets --continue/--resume
# shell = "/bin/bash"   # defaults to $SHELL

# Which panes open when a project window is first created. At least one true.
start_editor = true
start_terminal = true
start_ai = true

# Autosave editor changes (nvim writes on edit). false = save manually (:w).
autosave = true

# Layout ratios.
editor_ratio = 0.5   # editor width fraction (left column)
right_ratio  = 0.5   # terminal height fraction of the right column

# Status-bar temperature location (empty "" disables it).
weather = "Colorado Springs"

# Status-bar right side (#16). Shown left-to-right: metrics, weather, date, clock.
status_metrics = true   # cpu / ram / disk usage
status_weather = true
status_date = true
status_clock = true

# Editor line wrapping. true wraps long lines (default); false starts nvim with
# `nowrap` so long lines run off-screen instead.
wrap = true

# Keybindings — single characters pressed after the prefix. Each must be unique;
# digits 1-9 are reserved for switching windows. Warnings print on startup.
[keys]
focus_left = "h"
focus_down = "j"
focus_up   = "k"
focus_right = "l"
cycle = "o"
toggle_editor = "e"   # show/hide the editor pane
toggle_shell  = "t"   # show/hide the terminal pane
toggle_ai     = "c"   # show/hide the Claude pane
picker = "p"
help = "?"
quit = "q"
win_new = "n"    # new window (choose its project)
win_close = "X"  # close the current window (kills its panes)
detach = "d"     # detach client; server keeps running (reattach: forge)
reload = "r"     # restart server on latest build, reopen same windows
fresh = "F"      # forget saved session (next forge starts from the picker)
tab_new = "s"    # new terminal / Claude tab in the focused slot
tab_next = "]"   # next tab in the focused slot
tab_prev = "["   # prev tab in the focused slot
tab_close = "w"  # close the active tab in the focused slot
copy = "v"       # copy/scroll mode on the focused pane (scroll, select, copy)
git_diff = "g"   # git diff list; pick a file for a side-by-side editable diff
# also: prefix + 1..9 jumps to that window

# Editor (nvim) keybindings — full chords passed through to Neovim, so the
# finder/explorer/tab keys and the splash cheatsheet all read from here.
# Notation: a modifier joins its key with "-" (Ctrl-p, Ctrl-Shift-f); keys
# pressed in sequence are separated by a space ("Space e" = leader then e,
# "Space b d" = leader then b then d). Reload (prefix + reload key) after editing.
# (gd/gr/K and other standard Vim/LSP keys stay fixed and aren't listed here.)
[editor_keys]
open_file      = "Ctrl-p"       # fuzzy-open a file by name
search_in_file = "Ctrl-f"       # fuzzy-search in the current file
search_repo    = "Ctrl-Shift-f" # live-grep across the repo
explorer       = "Space e"      # toggle the file explorer
close_tab      = "Space b d"    # close the current editor buffer/tab
file_history   = "Space g h"    # git history of the current file
# next/prev editor tab use the prefix keys (Ctrl-a ] / Ctrl-a [).
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    // Serialize tests that set the process-global XDG_CONFIG_HOME env var.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn persist_rewrites_one_key_and_reloads() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Isolate config to a temp dir via XDG_CONFIG_HOME.
        let dir = std::env::temp_dir().join(format!("cf-cfgtest-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);

        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, DEFAULT_TOML).unwrap();

        persist_keybind("toggle_ai", 'x');

        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("toggle_ai = \"x\""));
        // Other keys and the file structure are preserved.
        assert!(text.contains("[keys]"));
        assert!(text.contains("win_new = \"n\"") || text.contains("win_new   = \"n\""));
        // Reloads cleanly with the new value.
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.keys.toggle_ai, 'x');

        // Editor keys rewrite in place within [editor_keys], preserving the token.
        persist_editor_key("open_file", "C-o");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("open_file = \"C-o\""));
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.editor_keys.open_file, "C-o");

        // When the block is absent (older config), it's created and the key set.
        let bare = "prefix = \"C-a\"\n[keys]\ntoggle_ai = \"c\"\n";
        std::fs::write(&path, bare).unwrap();
        persist_editor_key("explorer", "Space o");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("[editor_keys]"));
        let cfg: Config = toml::from_str(&text).unwrap();
        assert_eq!(cfg.editor_keys.explorer, "Space o");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn partial_config_completes_and_tab_close_rebind_sticks() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        // Mirror a real older config: [keys] present but missing tab_close et al.,
        // WASD focus (focus_up = "w"), and no [editor_keys] block.
        let dir = std::env::temp_dir().join(format!("cf-partial-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        let path = config_path();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "prefix = \"C-a\"\n[keys]\nfocus_up = \"w\"\nwin_close = \"X\"\n",
        )
        .unwrap();

        // Load upgrades the file in place: every key + [editor_keys] now on disk.
        let (_c, _w) = Config::load();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("tab_close ="), "tab_close line added");
        assert!(text.contains("[editor_keys]"), "editor_keys block added");
        // User's own values are preserved.
        assert!(text.contains("focus_up = \"w\""));

        // Rebinding tab_close to a free key now persists and survives reload
        // (previously it reverted to the default because the line didn't exist).
        persist_keybind("tab_close", 'x');
        let cfg: Config = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(cfg.keys.tab_close, 'x');
        assert_eq!(cfg.keys.focus_up, 'w');
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn prefix_parsing_and_guards() {
        assert_eq!(parse_prefix("C-a"), Ok(0x01));
        assert_eq!(parse_prefix("C-b"), Ok(0x02));
        assert_eq!(parse_prefix("space"), Ok(0x20));
        assert_eq!(parse_prefix("x"), Ok(b'x'));
        // Unusable specs are rejected (never returned as a live byte).
        assert!(parse_prefix("C-s").is_err()); // XOFF: froze the terminal
        assert!(parse_prefix("C-q").is_err()); // XON
        assert!(parse_prefix("C-space").is_err()); // NUL
                                                   // prefix_byte falls back to Ctrl-a for any rejected spec.
        let c = Config {
            prefix: "C-space".into(),
            ..Config::default()
        };
        assert_eq!(c.prefix_byte(), 0x01);
    }
}
