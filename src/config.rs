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
    /// Switch to the next window.
    pub win_next: char,
    /// Detach the client (leaves the server running in the background).
    pub detach: char,
    /// Reload: restart the server on the latest build, reopening the same
    /// project windows (pane contents reset).
    pub reload: char,
    /// Forget the saved session so the next `forge` starts fresh (the picker).
    pub fresh: char,
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
            toggle_ai: 'a',
            picker: 'p',
            help: '?',
            quit: 'q',
            win_new: 'c',
            win_close: 'X',
            win_next: 'n',
            detach: 'd',
            reload: 'r',
            fresh: 'F',
        }
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
ai = "claude"
# shell = "/bin/bash"   # defaults to $SHELL

# Layout ratios.
editor_ratio = 0.5   # editor width fraction (left column)
right_ratio  = 0.5   # terminal height fraction of the right column

# Keybindings — single characters pressed after the prefix.
[keys]
focus_left = "h"
focus_down = "j"
focus_up   = "k"
focus_right = "l"
cycle = "o"
toggle_editor = "e"   # show/hide the editor pane
toggle_shell  = "t"   # show/hide the terminal pane
toggle_ai     = "a"   # show/hide the Claude pane
picker = "p"
help = "?"
quit = "q"
win_new = "c"    # new window (choose its project)
win_close = "X"  # close the current window (kills its panes)
win_next = "n"   # switch to next window
detach = "d"     # detach client; server keeps running (reattach: forge)
reload = "r"     # restart server on latest build, reopen same windows
fresh = "F"      # forget saved session (next forge starts from the picker)
# also: prefix + 1..9 jumps to that window
"#;
