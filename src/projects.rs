//! Finding projects under the projects root (#122).
//!
//! Projects used to be exactly the immediate children of the root. They're now
//! grouped — `~/projects/SFA/eng/` holds the `eng` clone and its worktrees —
//! and the nesting depth varies: `GHE/infra` is one level down, `SFA/eng/eng`
//! is two. So discovery is a bounded walk rather than a `read_dir`.
//!
//! The picker, the new-worktree form's clone list and the worktree manager all
//! used to ask this question separately; they share this now.

use std::path::{Path, PathBuf};

/// How far below the root a project can be. A guard against walking something
/// huge if the root ever points somewhere unexpected — the real tree is two
/// levels deep and the full walk of it costs ~8ms cold, ~1.5ms warm.
const MAX_DEPTH: usize = 4;

/// What a project directory is, as far as git is concerned.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// `.git` is a directory: a clone, so worktrees can be branched from it.
    Clone,
    /// `.git` is a file: a linked worktree pointing back at its clone.
    Worktree,
    /// No `.git`. Still a project — plenty of useful directories aren't repos,
    /// and they were listed before this walk existed.
    Plain,
}

/// One project, with the path relative to the root that names it in a list.
#[derive(Clone, Debug)]
pub struct Project {
    pub path: PathBuf,
    /// Path relative to the root, `/`-separated: `sfaos`, or `SFA/eng/eng`.
    /// A bare name is ambiguous once nesting exists — `SFA/eng/eng` and a
    /// top-level `eng` can both be open at once.
    pub rel: String,
    pub kind: Kind,
}

/// Every project under `root`, sorted by display name.
///
/// The rule, in order:
/// 1. A directory holding `.git` is a project; don't descend into it.
/// 2. A directory without `.git` that contains projects is a *group*: descend,
///    and don't list the group itself.
/// 3. A directory without `.git` that contains no projects is itself a project.
///    This is what keeps plain non-repo directories working — several exist at
///    the root today and were listed before nesting was a thing.
pub fn find(root: &Path) -> Vec<Project> {
    let mut out = Vec::new();
    walk(root, root, 0, &mut out);
    out.sort_by_key(|p| p.rel.to_lowercase());
    out
}

/// Projects that are clones — the only ones a worktree can be branched from.
pub fn clones(root: &Path) -> Vec<Project> {
    find(root)
        .into_iter()
        .filter(|p| p.kind == Kind::Clone)
        .collect()
}

fn project(root: &Path, path: PathBuf, kind: Kind) -> Project {
    let rel = path
        .strip_prefix(root)
        .unwrap_or(&path)
        .to_string_lossy()
        .into_owned();
    Project { path, rel, kind }
}

/// `.git` decides: a directory is a clone, a file is a linked worktree, absent
/// means this isn't a repo.
fn kind_of(dir: &Path) -> Option<Kind> {
    let git = dir.join(".git");
    if git.is_dir() {
        Some(Kind::Clone)
    } else if git.is_file() {
        Some(Kind::Worktree)
    } else {
        None
    }
}

fn walk(root: &Path, dir: &Path, depth: usize, out: &mut Vec<Project>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut kids: Vec<PathBuf> = rd
        .filter_map(|e| e.ok())
        // `file_type` doesn't follow symlinks, so a symlinked directory isn't
        // walked into — that's how a loop would otherwise become infinite.
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .filter(|p| {
            !p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with('.'))
                .unwrap_or(true)
        })
        .collect();
    kids.sort();
    for path in kids {
        if let Some(kind) = kind_of(&path) {
            out.push(project(root, path, kind));
            continue;
        }
        if depth + 1 >= MAX_DEPTH {
            // Too deep to keep looking: treat it as a project rather than
            // silently dropping it.
            out.push(project(root, path, Kind::Plain));
            continue;
        }
        let before = out.len();
        walk(root, &path, depth + 1, out);
        if out.len() == before {
            // A group with nothing in it is just a directory — list it.
            out.push(project(root, path, Kind::Plain));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mkdirs(base: &Path, paths: &[&str]) {
        for p in paths {
            std::fs::create_dir_all(base.join(p)).unwrap();
        }
    }

    #[test]
    fn finds_nested_projects_without_losing_plain_directories() {
        let root = std::env::temp_dir().join(format!("cf-projects-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        mkdirs(
            &root,
            &[
                "sfaos/.git",        // top-level clone
                "SFA/eng/eng/.git",  // nested clone, two levels down
                "SFA/eng/eng-WT",    // its worktree (a .git *file*, below)
                "GHE/infra/.git",    // nested clone, one level down
                "WebForge",          // plain empty dir: still a project
                ".hidden/nope/.git", // hidden: skipped entirely
                "deep/a/b/c/d/e",    // past the depth cap
            ],
        );
        std::fs::write(root.join("SFA/eng/eng-WT/.git"), "gitdir: /elsewhere").unwrap();

        let found = find(&root);
        let names: Vec<&str> = found.iter().map(|p| p.rel.as_str()).collect();

        assert!(names.contains(&"sfaos"), "top-level clone: {names:?}");
        assert!(names.contains(&"SFA/eng/eng"), "nested clone: {names:?}");
        assert!(
            names.contains(&"SFA/eng/eng-WT"),
            "nested worktree: {names:?}"
        );
        assert!(names.contains(&"GHE/infra"), "one level down: {names:?}");
        // A plain directory with nothing in it stays a project — several exist
        // at the real root and were listed before nesting.
        assert!(names.contains(&"WebForge"), "plain dir: {names:?}");
        // Groups themselves are not projects.
        assert!(!names.contains(&"SFA"), "group listed: {names:?}");
        assert!(!names.contains(&"SFA/eng"), "group listed: {names:?}");
        assert!(!names.contains(&"GHE"), "group listed: {names:?}");
        // Hidden directories are skipped, contents and all.
        assert!(!names.iter().any(|n| n.starts_with('.')), "{names:?}");

        // The depth cap stops the walk; whatever it stopped on is still listed
        // rather than dropped.
        assert!(
            names.iter().any(|n| n.starts_with("deep")),
            "capped branch dropped: {names:?}"
        );
        assert!(
            !names.iter().any(|n| n.matches('/').count() > MAX_DEPTH),
            "walked past the cap: {names:?}"
        );

        // Kinds are what the callers filter on.
        let kind = |n: &str| found.iter().find(|p| p.rel == n).unwrap().kind;
        assert_eq!(kind("sfaos"), Kind::Clone);
        assert_eq!(kind("SFA/eng/eng"), Kind::Clone);
        assert_eq!(kind("SFA/eng/eng-WT"), Kind::Worktree);
        assert_eq!(kind("WebForge"), Kind::Plain);

        // `clones` is the worktree form's list: clones only, worktrees excluded.
        let cl: Vec<String> = clones(&root).into_iter().map(|p| p.rel).collect();
        assert!(cl.contains(&"SFA/eng/eng".to_string()));
        assert!(!cl.contains(&"SFA/eng/eng-WT".to_string()));
        assert!(!cl.contains(&"WebForge".to_string()));

        let _ = std::fs::remove_dir_all(&root);
    }
}
