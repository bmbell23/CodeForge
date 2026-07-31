//! Favorite files (#80), shared across every worktree and clone of a repo.
//!
//! Paths differ per checkout, so favorites are keyed by the repo's **root
//! commit** — identical in every clone and worktree, and available with no
//! remote configured. Entries are stored repo-relative, so favouriting
//! `src/main.rs` in one worktree lights it up in all the others.
//!
//! The store is a user-level file (`~/.local/share/codeforge/favorites`), one
//! `<repo-id>\t<relative path>` line per entry: nothing is written inside the
//! repo, which would dirty the working tree and wouldn't cross clones anyway.

use std::path::{Path, PathBuf};
use std::process::Command;

/// One favorite as shown in the list.
pub struct Favorite {
    /// Repo-relative path, as stored.
    pub rel: String,
    /// Where it lives in *this* checkout.
    pub abs: PathBuf,
    /// Whether it exists here — a favorite can be missing on this branch or in
    /// this worktree, and is listed dimmed rather than silently dropped.
    pub exists: bool,
}

/// The store file, honouring `XDG_DATA_HOME`.
fn store_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/share")
        });
    base.join("codeforge").join("favorites")
}

fn git(dir: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Repo root of `dir`'s checkout.
pub fn repo_root(dir: &Path) -> Option<PathBuf> {
    Some(PathBuf::from(
        git(dir, &["rev-parse", "--show-toplevel"])?.trim(),
    ))
}

/// Stable identity for the repo containing `dir`: its root commit. A repo with
/// several root commits (merged histories) picks the lexicographically smallest
/// so every checkout agrees on the same one.
pub fn repo_id(dir: &Path) -> Option<String> {
    let out = git(dir, &["rev-list", "--max-parents=0", "HEAD"])?;
    out.split_whitespace().min().map(|s| s.to_string())
}

/// Every stored line, as `(repo id, relative path)`.
fn read_all() -> Vec<(String, String)> {
    let Ok(text) = std::fs::read_to_string(store_path()) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| l.split_once('\t'))
        .map(|(id, rel)| (id.to_string(), rel.to_string()))
        .collect()
}

fn write_all(entries: &[(String, String)]) -> std::io::Result<()> {
    let path = store_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut body = String::new();
    for (id, rel) in entries {
        body.push_str(id);
        body.push('\t');
        body.push_str(rel);
        body.push('\n');
    }
    std::fs::write(path, body)
}

/// `file` as a repo-relative path, if it's inside `dir`'s repo.
fn relative(dir: &Path, file: &Path) -> Option<(PathBuf, String)> {
    let root = repo_root(dir)?;
    let rel = file.strip_prefix(&root).ok()?;
    Some((root, rel.to_string_lossy().into_owned()))
}

/// Toggle `file`'s favorite status for its repo. Returns whether it is now a
/// favorite, or `None` when the file isn't inside a git repo.
pub fn toggle(dir: &Path, file: &Path) -> Option<bool> {
    let id = repo_id(dir)?;
    let (_, rel) = relative(dir, file)?;
    let mut entries = read_all();
    let existing = entries.iter().position(|(i, r)| *i == id && *r == rel);
    let now_favorite = match existing {
        Some(pos) => {
            entries.remove(pos);
            false
        }
        None => {
            entries.push((id, rel));
            true
        }
    };
    write_all(&entries).ok()?;
    Some(now_favorite)
}

/// Favorites for the repo containing `dir`, resolved against *this* checkout
/// and sorted by path so the list is stable between openings.
pub fn list(dir: &Path) -> Vec<Favorite> {
    let (Some(id), Some(root)) = (repo_id(dir), repo_root(dir)) else {
        return Vec::new();
    };
    let mut favs: Vec<Favorite> = read_all()
        .into_iter()
        .filter(|(i, _)| *i == id)
        .map(|(_, rel)| {
            let abs = root.join(&rel);
            Favorite {
                exists: abs.exists(),
                rel,
                abs,
            }
        })
        .collect();
    favs.sort_by(|a, b| a.rel.cmp(&b.rel));
    favs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn git_in(dir: &Path, args: &[&str]) {
        let out = Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {args:?}");
    }

    /// A favorite marked in one worktree is a favorite in another checkout of
    /// the same repo, because both resolve to the same root commit (#80).
    #[test]
    fn favorites_follow_the_repo_across_checkouts() {
        let tmp = std::env::temp_dir().join(format!("cf-fav-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (main, wt) = (tmp.join("main"), tmp.join("wt"));
        std::fs::create_dir_all(&main).unwrap();
        // Keep the test's writes out of the real store.
        std::env::set_var("XDG_DATA_HOME", tmp.join("data"));

        git_in(&main, &["init", "-q"]);
        git_in(&main, &["config", "user.email", "t@t"]);
        git_in(&main, &["config", "user.name", "t"]);
        std::fs::write(main.join("a.txt"), "x\n").unwrap();
        git_in(&main, &["add", "."]);
        git_in(&main, &["commit", "-qm", "first"]);
        git_in(
            &main,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "side"],
        );

        // Both checkouts agree on the repo's identity.
        assert_eq!(repo_id(&main), repo_id(&wt));
        assert!(repo_id(&main).is_some());

        assert_eq!(toggle(&main, &main.join("a.txt")), Some(true));
        assert_eq!(list(&main).len(), 1);

        // The other worktree sees it, resolved against its own path.
        let from_wt = list(&wt);
        assert_eq!(from_wt.len(), 1, "the favorite crosses checkouts");
        assert_eq!(from_wt[0].rel, "a.txt");
        assert_eq!(from_wt[0].abs, wt.join("a.txt"));
        assert!(from_wt[0].exists);

        // Toggling from the *other* worktree clears it for both.
        assert_eq!(toggle(&wt, &wt.join("a.txt")), Some(false));
        assert!(list(&main).is_empty());

        // A favorite missing in this checkout is listed, marked absent.
        toggle(&main, &main.join("gone.txt"));
        let l = list(&wt);
        assert_eq!(l.len(), 1);
        assert!(!l[0].exists, "missing files are shown dimmed, not dropped");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
