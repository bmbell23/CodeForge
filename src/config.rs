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
        }
    }
}

/// The rebindable actions, in the order shown in the `Ctrl-a ?` editor:
/// `(config field, human label)`. Focus keys are listed individually so each
/// can be rebound. `1..9` (window jump) and mouse aren't rebindable.
pub const EDITABLE: [(&str, &str); 21] = [
    ("toggle_editor", "show/hide editor"),
    ("toggle_shell", "show/hide terminal"),
    ("toggle_ai", "show/hide Claude"),
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
            _ => return None,
        })
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
    fn bindings(&self) -> [(&'static str, char); 21] {
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
                Ok(c) => (c, None),
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

    /// The prefix as a raw byte (e.g. `"C-a"` -> 0x01).
    pub fn prefix_byte(&self) -> u8 {
        parse_prefix(&self.prefix)
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
    let path = config_path();
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let mut out = String::with_capacity(text.len() + 8);
    let mut done = false;
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
                    out.push(ch);
                    out.push_str("\"\n");
                    done = true;
                    continue;
                }
            }
        }
        out.push_str(line);
        out.push('\n');
    }
    let _ = std::fs::write(&path, out);
}

/// Parse a prefix spec: `"C-<c>"` -> Ctrl-<c>, or a single char literally.
fn parse_prefix(s: &str) -> u8 {
    let s = s.trim();
    if let Some(rest) = s.strip_prefix("C-").or_else(|| s.strip_prefix("c-")) {
        if let Some(c) = rest.chars().next() {
            // Ctrl masks bit 6/7: 'a' (0x61) -> 0x01.
            return (c as u8) & 0x1f;
        }
    }
    s.bytes().next().unwrap_or(0x01)
}

/// Written to `config.toml` on first run.
const DEFAULT_TOML: &str = r#"# CodeForge configuration.
# Edit and restart forge. Every field is optional.

# Command prefix (tmux-style). Examples: "C-a", "C-b", "C-space".
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
# also: prefix + 1..9 jumps to that window
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persist_rewrites_one_key_and_reloads() {
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

        let _ = std::fs::remove_dir_all(&dir);
    }
}
