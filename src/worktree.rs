//! Native git-worktree creation (#50). Reimplements the generate flow of the
//! team's `worktree.sh` as first-class logic, driven by the picker's new-worktree
//! form instead of the shell CLI. Given a chosen clone under the projects root,
//! a Jira ticket, a short description, and an upstream branch, it creates a
//! sibling worktree `<clone>-<TICKET>-<desc>` off `origin/<upstream>` and wires
//! up the same symlinks worktree.sh does. The VS Code `.code-workspace` /
//! `code`-launch side of the script is intentionally dropped — CodeForge opens
//! the new worktree as its own window.

use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{bail, Context, Result};

/// A request to create a worktree, as gathered by the form.
pub struct WorktreeSpec {
    /// Clone directory name under the projects root, e.g. "sfaos".
    pub clone: String,
    /// Ticket as typed, e.g. "SFAP-123456" or "DDNDO-123"; normalized here.
    pub ticket: String,
    /// Short description as typed (may contain spaces); sanitized here.
    pub description: String,
    /// Bare upstream branch (no `origin/`); empty means the default, `master`.
    pub upstream: String,
    /// For an sfaos worktree, also create the accompanying `auto` one
    /// (worktree.sh's "both").
    pub also_auto: bool,
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

/// Collapse runs of whitespace to single underscores and trim, so
/// "my little bug fix" -> "my_little_bug_fix". (worktree.sh uses the raw string
/// and just tells you not to use spaces; we do the sanitizing for you, #50.)
pub fn sanitize_desc(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join("_")
}

/// Normalize the ticket: `prefix-number` upcases the prefix (SFAP-… / DDNDO-…);
/// a bare number becomes `SFAP-<number>` (worktree.sh's default). Anything else
/// is passed through trimmed.
pub fn normalize_ticket(input: &str) -> String {
    let t = input.trim();
    if let Some((prefix, num)) = t.split_once('-') {
        if !prefix.is_empty()
            && prefix.chars().all(|c| c.is_ascii_alphabetic())
            && !num.is_empty()
            && num.chars().all(|c| c.is_ascii_digit())
        {
            return format!("{}-{}", prefix.to_ascii_uppercase(), num);
        }
    }
    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return format!("SFAP-{t}");
    }
    t.to_string()
}

/// The worktree directory name, which is also the branch name:
/// `<clone>-<TICKET>-<desc>`.
pub fn worktree_name(clone: &str, ticket: &str, desc: &str) -> String {
    format!("{clone}-{ticket}-{desc}")
}

/// `origin/<upstream>`, defaulting to `master` when the field is blank.
fn upstream_or_default(upstream: &str) -> String {
    let u = upstream.trim();
    if u.is_empty() {
        "master".to_string()
    } else {
        u.to_string()
    }
}

/// Create one worktree from `clone` off `origin/<upstream>` at
/// `<projects_root>/<name>`, symlinking `/home/logs/<ticket>` if present.
/// Returns the new worktree's path.
fn create_one(
    projects_root: &Path,
    clone: &str,
    ticket: &str,
    desc: &str,
    upstream: &str,
    progress: &dyn Fn(&str),
) -> Result<PathBuf> {
    let clone_dir = projects_root.join(clone);
    if !clone_dir.join(".git").exists() {
        bail!("{} is not a git clone", clone_dir.display());
    }
    let name = worktree_name(clone, ticket, desc);
    let dest = projects_root.join(&name);

    // Fetch the base branch so origin/<upstream> is current.
    progress(&format!("fetching {clone} origin/{upstream}…"));
    git(&clone_dir, &["fetch", "origin", upstream])
        .map_err(|e| anyhow::anyhow!(e))
        .with_context(|| format!("git fetch origin {upstream} in {clone}"))?;

    // If the branch already exists but isn't checked out in a worktree, it's an
    // orphan from a prior run — delete it. If it *is* attached, refuse.
    if git(
        &clone_dir,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
    {
        let list = git(&clone_dir, &["worktree", "list"]).unwrap_or_default();
        if list.contains(&format!("[{name}]")) {
            bail!("worktree {name} already exists");
        }
        let _ = git(&clone_dir, &["branch", "-D", &name]);
    }

    progress(&format!("adding worktree {name}…"));
    git(
        &clone_dir,
        &[
            "worktree",
            "add",
            "-b",
            &name,
            &dest.to_string_lossy(),
            &format!("origin/{upstream}"),
        ],
    )
    .map_err(|e| anyhow::anyhow!(e))
    .with_context(|| format!("git worktree add {name}"))?;

    // Logs symlink: <wt>/logs -> /home/logs/<ticket>, only if the source dir
    // exists (worktree.sh convention).
    let logs_src = PathBuf::from("/home/logs").join(ticket);
    if logs_src.is_dir() {
        progress("linking logs…");
        let _ = std::os::unix::fs::symlink(&logs_src, dest.join("logs"));
    }

    Ok(dest)
}

/// Link `<worktree>/janus/test/scripts/lib -> <lib_target>` when the scripts
/// dir exists (sfaos layout). No-op otherwise.
fn link_sfaos_lib(worktree: &Path, lib_target: &Path) {
    let scripts = worktree.join("janus/test/scripts");
    if scripts.is_dir() {
        let _ = std::os::unix::fs::symlink(lib_target, scripts.join("lib"));
    }
}

/// Run `env/venv.sh` in `<worktree>/janus/test/monty` when it exists (sfaos
/// layout). Best-effort — a venv failure doesn't undo the worktree.
fn run_sfaos_venv(worktree: &Path, progress: &dyn Fn(&str)) {
    let monty = worktree.join("janus/test/monty");
    if monty.is_dir() {
        progress("setting up python venv…");
        let _ = Command::new("env/venv.sh").current_dir(&monty).status();
    }
}

/// Create the worktree(s) for `spec` and return the primary one's path (the
/// path CodeForge should open as a window). `progress` is called with a short
/// human status at each step so the form can show live progress (#50).
pub fn create(
    projects_root: &Path,
    spec: &WorktreeSpec,
    progress: &dyn Fn(&str),
) -> Result<PathBuf> {
    let ticket = normalize_ticket(&spec.ticket);
    let desc = sanitize_desc(&spec.description);
    if desc.is_empty() {
        bail!("a short description is required");
    }
    let upstream = upstream_or_default(&spec.upstream);

    let primary = create_one(
        projects_root,
        &spec.clone,
        &ticket,
        &desc,
        &upstream,
        progress,
    )?;

    // sfaos + "both": also make the auto worktree and cross-link the sfaos
    // scripts/lib to the *auto worktree's* lib (worktree.sh both-mode).
    if spec.clone == "sfaos" && spec.also_auto {
        let auto = create_one(projects_root, "auto", &ticket, &desc, &upstream, progress)?;
        progress("linking sfaos ↔ auto…");
        link_sfaos_lib(&primary, &auto.join("lib"));
        run_sfaos_venv(&primary, progress);
    } else if spec.clone == "sfaos" {
        // Single sfaos: link to the base auto clone's lib.
        progress("linking lib…");
        link_sfaos_lib(&primary, &projects_root.join("auto/lib"));
        run_sfaos_venv(&primary, progress);
    }

    progress("opening…");
    Ok(primary)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_description() {
        assert_eq!(sanitize_desc("my little bug fix"), "my_little_bug_fix");
        assert_eq!(sanitize_desc("  padded  spaces "), "padded_spaces");
        assert_eq!(sanitize_desc("already_ok"), "already_ok");
        assert_eq!(sanitize_desc("tabs\tand\nnewlines"), "tabs_and_newlines");
    }

    #[test]
    fn normalizes_ticket() {
        assert_eq!(normalize_ticket("123456"), "SFAP-123456"); // bare number
        assert_eq!(normalize_ticket("sfap-123"), "SFAP-123"); // upcases prefix
        assert_eq!(normalize_ticket("DDNDO-123"), "DDNDO-123"); // kept
        assert_eq!(normalize_ticket("  SFAP-9 "), "SFAP-9"); // trimmed
        assert_eq!(normalize_ticket("weird"), "weird"); // pass-through
    }

    #[test]
    fn builds_names_and_upstream() {
        assert_eq!(
            worktree_name("sfaos", "SFAP-123456", "my_fix"),
            "sfaos-SFAP-123456-my_fix"
        );
        assert_eq!(upstream_or_default(""), "master");
        assert_eq!(upstream_or_default("  "), "master");
        assert_eq!(upstream_or_default("12.9"), "12.9");
    }
}
