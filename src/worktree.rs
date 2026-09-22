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
/// on an SFA clone a bare number becomes `SFAP-<number>` (worktree.sh's
/// default). Anything else is passed through trimmed — including a bare number
/// on a non-SFA clone (#102), where `SFAP` would be simply wrong.
pub fn normalize_ticket(input: &str, sfa: bool) -> String {
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
    if sfa && !t.is_empty() && t.chars().all(|c| c.is_ascii_digit()) {
        return format!("SFAP-{t}");
    }
    t.to_string()
}

/// The worktree directory name, which is also the branch name:
/// `<clone>-<TICKET>-<desc>`, or `<clone>-<desc>` when there's no ticket —
/// repos outside the SFA Jira projects don't have one, and keeping the segment
/// would leave a double dash (#102).
pub fn worktree_name(clone: &str, ticket: &str, desc: &str) -> String {
    if ticket.is_empty() {
        format!("{clone}-{desc}")
    } else {
        format!("{clone}-{ticket}-{desc}")
    }
}

/// `origin/<upstream>`, falling back to the clone's own default branch when the
/// field is blank. Hardcoding `master` broke every clone that uses `main` —
/// the GHE and GitLab repos (#102).
fn upstream_or_default(upstream: &str, clone_dir: &Path) -> String {
    let u = upstream.trim();
    if u.is_empty() {
        default_branch(clone_dir)
    } else {
        u.to_string()
    }
}

/// The clone's default branch, without the `origin/` prefix. Reads local refs
/// only — this runs while the form is open, so it must not touch the network:
/// `origin/HEAD` when the clone has it set, else whichever of `main`/`master`
/// exists as a remote-tracking ref (mlai has no `origin/HEAD`), else `master`.
pub fn default_branch(clone_dir: &Path) -> String {
    if let Some(head) = git_out(
        clone_dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        if let Some(b) = head.trim().strip_prefix("origin/") {
            if !b.is_empty() {
                return b.to_string();
            }
        }
    }
    for cand in ["main", "master"] {
        let r = format!("refs/remotes/origin/{cand}");
        if git_out(clone_dir, &["rev-parse", "--verify", "--quiet", &r]).is_some() {
            return cand.to_string();
        }
    }
    "master".to_string()
}

/// Whether `clone_dir` is one of the SFA repos, i.e. its origin lives under the
/// `sfa/` namespace — true for the Gerrit clones (sfaos, auto, eng, infra, qa)
/// and for `sfa/k8s` on GHE, false for `ddn/reservations`, `ddn/mlai` and the
/// GitLab repos. Drives the `SFAP-` ticket conventions and nothing else, so a
/// wrong answer only costs a prefill (#102).
pub fn is_sfa_clone(clone_dir: &Path) -> bool {
    let Some(url) = git_out(clone_dir, &["remote", "get-url", "origin"]) else {
        return false;
    };
    url.trim()
        .trim_end_matches(".git")
        .rsplit_once('/')
        .and_then(|(head, _)| head.rsplit_once(['/', ':']).map(|(_, ns)| ns))
        .is_some_and(|ns| ns == "sfa")
}

/// The repo's own directory name, from a possibly-nested relative path:
/// `SFA/eng/eng` -> `eng` (#123). A worktree is named after the repo, not after
/// the group the repo is filed under.
pub fn clone_base(clone: &str) -> &str {
    clone.rsplit('/').next().unwrap_or(clone)
}

/// Where a worktree for `clone_dir` belongs: beside its clone. This used to be
/// the projects root, which was the same thing only while every clone sat
/// directly under it — for a nested clone it scattered worktrees into the root,
/// away from the repo they belong to (#123).
pub fn worktree_dest(clone_dir: &Path, name: &str) -> PathBuf {
    clone_dir.parent().unwrap_or(clone_dir).join(name)
}

/// The clone named `name` sitting beside `clone_dir`, falling back to one
/// directly under the root. That's how the sfaos/auto pairing finds its partner
/// once repos can be grouped: look in the group first, the root second (#123).
pub fn sibling_clone(clone_dir: &Path, projects_root: &Path, name: &str) -> PathBuf {
    let beside = clone_dir.parent().unwrap_or(clone_dir).join(name);
    if beside.join(".git").exists() {
        return beside;
    }
    projects_root.join(name)
}

/// Create one worktree from the clone at `clone_dir`, off `origin/<upstream>`,
/// beside that clone. Symlinks `/home/logs/<ticket>` if present. Returns the new
/// worktree's path.
fn create_one(
    clone_dir: &Path,
    ticket: &str,
    desc: &str,
    upstream: &str,
    progress: &dyn Fn(&str),
) -> Result<PathBuf> {
    if !clone_dir.join(".git").exists() {
        bail!("{} is not a git clone", clone_dir.display());
    }
    let clone = clone_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let clone = clone.as_str();
    let name = worktree_name(clone, ticket, desc);
    let dest = worktree_dest(clone_dir, &name);

    // Fetch the base branch so origin/<upstream> is current.
    progress(&format!("fetching {clone} origin/{upstream}…"));
    git(clone_dir, &["fetch", "origin", upstream])
        .map_err(|e| anyhow::anyhow!(e))
        .with_context(|| format!("git fetch origin {upstream} in {clone}"))?;

    // If the branch already exists but isn't checked out in a worktree, it's an
    // orphan from a prior run — delete it. If it *is* attached, refuse.
    if git(
        clone_dir,
        &[
            "show-ref",
            "--verify",
            "--quiet",
            &format!("refs/heads/{name}"),
        ],
    )
    .is_ok()
    {
        let list = git(clone_dir, &["worktree", "list"]).unwrap_or_default();
        if list.contains(&format!("[{name}]")) {
            bail!("worktree {name} already exists");
        }
        let _ = git(clone_dir, &["branch", "-D", &name]);
    }

    progress(&format!("adding worktree {name}…"));
    git(
        clone_dir,
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
    if !ticket.is_empty() && logs_src.is_dir() {
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
    let clone_dir = projects_root.join(&spec.clone);
    let ticket = normalize_ticket(&spec.ticket, is_sfa_clone(&clone_dir));
    let desc = sanitize_desc(&spec.description);
    if desc.is_empty() {
        bail!("a short description is required");
    }
    let upstream = upstream_or_default(&spec.upstream, &clone_dir);

    let primary = create_one(&clone_dir, &ticket, &desc, &upstream, progress)?;
    // The repo's own name, so the sfaos/auto conventions still match once repos
    // can live inside a group (#123).
    let base = clone_base(&spec.clone);

    // sfaos + "both": also make the auto worktree and cross-link the sfaos
    // scripts/lib to the *auto worktree's* lib (worktree.sh both-mode).
    if base == "sfaos" && spec.also_auto {
        let auto_clone = sibling_clone(&clone_dir, projects_root, "auto");
        let auto = create_one(&auto_clone, &ticket, &desc, &upstream, progress)?;
        progress("linking sfaos ↔ auto…");
        link_sfaos_lib(&primary, &auto.join("lib"));
        run_sfaos_venv(&primary, progress);
    } else if base == "sfaos" {
        // Single sfaos: link to the base auto clone's lib.
        progress("linking lib…");
        let auto_clone = sibling_clone(&clone_dir, projects_root, "auto");
        link_sfaos_lib(&primary, &auto_clone.join("lib"));
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
        assert_eq!(normalize_ticket("123456", true), "SFAP-123456"); // bare number
        assert_eq!(normalize_ticket("sfap-123", true), "SFAP-123"); // upcases prefix
        assert_eq!(normalize_ticket("DDNDO-123", true), "DDNDO-123"); // kept
        assert_eq!(normalize_ticket("  SFAP-9 ", true), "SFAP-9"); // trimmed
        assert_eq!(normalize_ticket("weird", true), "weird"); // pass-through
                                                              // Off an SFA clone a bare number is not an SFAP ticket (#102).
        assert_eq!(normalize_ticket("225", false), "225");
        assert_eq!(normalize_ticket("mlai-225", false), "MLAI-225");
    }

    /// Worktrees belong beside their clone, and are named after the repo — not
    /// after the group it's filed under (#123). Before nesting, "beside the
    /// clone" and "at the projects root" were the same place; for a nested
    /// clone they are not, and using the root scattered worktrees away from the
    /// repo they belong to.
    #[test]
    fn nested_clones_put_worktrees_beside_themselves() {
        let root = Path::new("/home/u/projects");

        // The repo's own name, however deep it's filed.
        assert_eq!(clone_base("sfaos"), "sfaos");
        assert_eq!(clone_base("SFA/eng/eng"), "eng");
        assert_eq!(clone_base(""), "");

        // Top-level clone: unchanged — beside the clone *is* the root.
        let top = root.join("sfaos");
        assert_eq!(
            worktree_dest(&top, "sfaos-SFAP-1-fix"),
            root.join("sfaos-SFAP-1-fix")
        );
        // Nested clone: beside the clone, inside its group.
        let nested = root.join("SFA/eng/eng");
        assert_eq!(
            worktree_dest(&nested, "eng-SFAP-1-fix"),
            root.join("SFA/eng/eng-SFAP-1-fix"),
            "a nested worktree must not land at the root"
        );

        // The sfaos/auto pairing looks in the group first, the root second.
        let tmp = std::env::temp_dir().join(format!("cf-sib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let grouped = tmp.join("SFA/sfa/sfaos");
        std::fs::create_dir_all(&grouped).unwrap();
        std::fs::create_dir_all(tmp.join("auto/.git")).unwrap();
        // No `auto` beside it yet: fall back to the root's.
        assert_eq!(sibling_clone(&grouped, &tmp, "auto"), tmp.join("auto"));
        // Once one exists in the group, prefer it.
        std::fs::create_dir_all(tmp.join("SFA/sfa/auto/.git")).unwrap();
        assert_eq!(
            sibling_clone(&grouped, &tmp, "auto"),
            tmp.join("SFA/sfa/auto")
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn scratch_e2e_nested_create() {
        let tmp = std::env::temp_dir().join(format!("cf-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let origin = tmp.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        let run = |d: &Path, a: &[&str]| {
            let o = Command::new("git")
                .arg("-C")
                .arg(d)
                .args(a)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "git {a:?}: {}",
                String::from_utf8_lossy(&o.stderr)
            );
        };
        run(&origin, &["init", "-q", "-b", "main"]);
        run(&origin, &["config", "user.email", "t@t"]);
        run(&origin, &["config", "user.name", "t"]);
        std::fs::write(origin.join("f"), "x").unwrap();
        run(&origin, &["add", "."]);
        run(&origin, &["commit", "-qm", "one"]);
        // Nested exactly like the real layout: <root>/SFA/eng/eng
        std::fs::create_dir_all(tmp.join("SFA/eng")).unwrap();
        let o = Command::new("git")
            .arg("-C")
            .arg(tmp.join("SFA/eng"))
            .args(["clone", "-q", origin.to_str().unwrap(), "eng"])
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));

        let spec = WorktreeSpec {
            clone: "SFA/eng/eng".into(),
            ticket: "SFAP-1".into(),
            description: "nested test".into(),
            upstream: String::new(),
            also_auto: false,
        };
        let made = create(&tmp, &spec, &|m| println!("  progress: {m}")).unwrap();
        println!("E2E created at: {}", made.display());
        println!(
            "  exists={} is_worktree={}",
            made.is_dir(),
            is_linked_worktree(&made)
        );
        println!(
            "  root polluted={:?}",
            std::fs::read_dir(&tmp)
                .unwrap()
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
        );
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn builds_names() {
        assert_eq!(
            worktree_name("sfaos", "SFAP-123456", "my_fix"),
            "sfaos-SFAP-123456-my_fix"
        );
        // No ticket: the segment goes away rather than leaving a double dash.
        assert_eq!(
            worktree_name("reservations", "", "my_fix"),
            "reservations-my_fix"
        );
    }
}

// ---------------------------------------------------------------------------
// Worktree manager (#83): discovery, status, deletion.
// ---------------------------------------------------------------------------

/// How ready a worktree is to be thrown away.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum WtState {
    /// Remote state not checked yet — the list shows these as "updating…".
    Unknown,
    /// Uncommitted changes present.
    Dirty,
    /// Committed work the remote doesn't have.
    Pending,
    /// Nothing uncommitted, nothing unpushed: safe to delete.
    Clean,
    /// Couldn't be classified (fetch failed, no upstream, …); never deletable.
    Error(&'static str),
}

/// One row of the manager.
#[derive(Clone, Debug)]
pub struct WtEntry {
    pub name: String,
    pub path: PathBuf,
    pub state: WtState,
}

impl WtEntry {
    /// Only a positively-clean worktree may be deleted. Everything else —
    /// dirty, pending, unknown, unclassifiable — is listed but refused.
    pub fn deletable(&self) -> bool {
        self.state == WtState::Clean
    }
}

/// Whether `dir` is a *linked* worktree rather than a clone's main checkout.
/// In a linked worktree git's per-worktree dir differs from the shared common
/// dir; in a clone they're the same. Clones (sfaos, k8s, auto, infra…) must
/// never be deletable, so anything that fails this test is not offered (#83).
pub fn is_linked_worktree(dir: &Path) -> bool {
    let git_dir = git_out(dir, &["rev-parse", "--absolute-git-dir"]);
    let common = git_out(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    );
    match (git_dir, common) {
        (Some(g), Some(c)) => {
            let (g, c) = (g.trim(), c.trim());
            !g.is_empty() && !c.is_empty() && g != c
        }
        _ => false,
    }
}

fn git_out(dir: &Path, args: &[&str]) -> Option<String> {
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

/// Every linked worktree directly under `root`, sorted by name. Clones are
/// skipped: they aren't linked worktrees, so they can't reach the delete path.
pub fn list_worktrees(root: &Path) -> Vec<WtEntry> {
    let mut out: Vec<WtEntry> = crate::projects::find(root)
        .into_iter()
        // `projects` classifies by whether `.git` is a file, which is cheap but
        // only a hint. `is_linked_worktree` asks git itself, and deletion is
        // irreversible — keep the authoritative check.
        .filter(|p| is_linked_worktree(&p.path))
        .map(|p| WtEntry {
            name: p.rel,
            path: p.path,
            state: WtState::Unknown,
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The clone a worktree belongs to — its shared git directory. Every worktree of
/// one clone shares this, and therefore shares one object store and one set of
/// remote-tracking refs, so fetching in any of them updates all of them (#125).
pub fn common_dir(dir: &Path) -> Option<PathBuf> {
    git_out(
        dir,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(|s| PathBuf::from(s.trim()))
}

/// One fetch per clone, for a set of worktrees. Returns the distinct clones so
/// the caller can report progress; worktrees whose clone can't be resolved fall
/// back to fetching individually.
///
/// This used to be one fetch per *worktree*: 63 of them across 8 clones on the
/// real tree, so 55 were exact duplicates of work already done (#125).
pub fn fetch_targets(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut out: Vec<PathBuf> = Vec::new();
    for d in dirs {
        match common_dir(d) {
            Some(c) => {
                if !seen.contains(&c) {
                    seen.push(c);
                    // Fetch *in the worktree*, not in the bare common dir: the
                    // remote is configured there and the refs it updates are
                    // shared anyway.
                    out.push(d.clone());
                }
            }
            // Unresolvable: don't let it silently skip its own fetch.
            None => out.push(d.clone()),
        }
    }
    out
}

/// Fetch, bounded so a hung network can't freeze the list.
pub fn fetch(dir: &Path) {
    let _ = Command::new("timeout")
        .arg("20")
        .arg("git")
        .arg("-C")
        .arg(dir)
        .args(["fetch", "--quiet"])
        .output();
}

/// Classify a worktree using the remote-tracking refs as they stand, without
/// fetching. Callers that need current remote state fetch first — once per
/// clone, via `fetch_targets` (#125).
pub fn classify_local(dir: &Path) -> WtState {
    classify_inner(dir)
}

/// Classify one worktree. Fetches first so "unpushed" is judged against current
/// remote state — `fetch` only updates remote-tracking refs, so the worktree
/// being inspected is never modified (a `pull` could conflict or fail on a
/// dirty tree, which is precisely the case we most need to classify).
pub fn classify(dir: &Path) -> WtState {
    fetch(dir);
    classify_inner(dir)
}

/// The classification itself, with no network access.
fn classify_inner(dir: &Path) -> WtState {
    match git_out(dir, &["status", "--porcelain"]) {
        Some(s) if !s.trim().is_empty() => return WtState::Dirty,
        None => return WtState::Error("status failed"),
        _ => {}
    }
    // Already merged into the mainline? Counting commits the upstream lacks
    // (below) compares SHAs, so work that landed in master rebased, squashed or
    // cherry-picked still reads as unpushed — the common case for a finished
    // ticket, which then can't be deleted (#97). `git cherry` matches by
    // patch-id instead and lists only what mainline genuinely doesn't have.
    //
    // Note a tree comparison against mainline's tip would be wrong here: these
    // worktrees are usually *behind* master, so the diff is dominated by master's
    // own newer commits and says nothing about this branch's unique work.
    let merged = mainline(dir).and_then(|m| {
        git_out(dir, &["cherry", &m, "HEAD"]).map(|out| !out.lines().any(|l| l.starts_with('+')))
    });
    if merged == Some(true) {
        return WtState::Clean;
    }

    // Otherwise the work still has to exist somewhere other than this worktree:
    // pushed to its own upstream. No upstream at all and nothing merged means it
    // exists nowhere else, which is the same risk as unpushed commits.
    match git_out(dir, &["rev-list", "--count", "@{u}..HEAD"]) {
        Some(n) if n.trim() == "0" => WtState::Clean,
        Some(_) => WtState::Pending,
        // Unmerged commits on a branch that was never pushed: real work, not an
        // error — only report Error when mainline couldn't be resolved either.
        None if merged == Some(false) => WtState::Pending,
        None => WtState::Error("no upstream"),
    }
}

/// The mainline branch to judge "already merged" against: whatever
/// `origin/HEAD` points at, falling back to origin/main then origin/master.
/// `None` when the remote has none of them, which leaves classification to the
/// upstream check alone.
fn mainline(dir: &Path) -> Option<String> {
    if let Some(s) = git_out(
        dir,
        &["symbolic-ref", "--short", "refs/remotes/origin/HEAD"],
    ) {
        let s = s.trim().to_string();
        if !s.is_empty() {
            return Some(s);
        }
    }
    ["origin/main", "origin/master"]
        .into_iter()
        .find(|r| git_out(dir, &["rev-parse", "--verify", "--quiet", r]).is_some())
        .map(String::from)
}

/// Delete a worktree: its registration, its directory, and its branch — the
/// same three steps as `wt rm`. Refuses anything that isn't a linked worktree,
/// so a clone can never be removed through this path (#83).
pub fn delete(entry: &WtEntry) -> Result<()> {
    if !is_linked_worktree(&entry.path) {
        bail!("{} is not a linked worktree — refusing", entry.name);
    }
    if !entry.deletable() {
        bail!("{} is not clean — refusing", entry.name);
    }
    let branch = git_out(&entry.path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|s| s.trim().to_string())
        .filter(|b| !b.is_empty() && b != "HEAD");
    // The common dir is where the worktree is registered; run the removal from
    // there, since the worktree's own directory is about to vanish.
    let common = git_out(
        &entry.path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )
    .map(|s| PathBuf::from(s.trim().to_string()))
    .and_then(|p| p.parent().map(|p| p.to_path_buf()))
    .context("locating the clone that owns this worktree")?;

    let out = Command::new("git")
        .arg("-C")
        .arg(&common)
        .args(["worktree", "remove", "--force"])
        .arg(&entry.path)
        .output()
        .context("git worktree remove")?;
    if !out.status.success() {
        // Match `wt rm`'s fallback: a worktree whose registration is already
        // broken still has a directory to clear.
        std::fs::remove_dir_all(&entry.path)
            .with_context(|| format!("removing {}", entry.path.display()))?;
        let _ = Command::new("git")
            .arg("-C")
            .arg(&common)
            .args(["worktree", "prune"])
            .output();
    }
    if let Some(branch) = branch {
        // `-d` not `-D`: the worktree was verified clean, so the branch is
        // merged/pushed and this succeeds. If it somehow refuses, that's a
        // signal worth surfacing rather than forcing past.
        let b = Command::new("git")
            .arg("-C")
            .arg(&common)
            .args(["branch", "-d", &branch])
            .output()
            .context("git branch -d")?;
        if !b.status.success() {
            bail!(
                "worktree removed, but branch {branch} kept: {}",
                String::from_utf8_lossy(&b.stderr).trim()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod manager_tests {
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

    /// Worktrees of one clone share an object store and remote-tracking refs, so
    /// one fetch covers all of them (#125). On the real tree that's 63 worktrees
    /// over 8 clones — 55 of the fetches were exact duplicates.
    #[test]
    fn one_fetch_per_clone_not_per_worktree() {
        let tmp = std::env::temp_dir().join(format!("cf-fetch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let origin = tmp.join("origin");
        std::fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "main"]);
        git_in(&origin, &["config", "user.email", "t@t"]);
        git_in(&origin, &["config", "user.name", "t"]);
        std::fs::write(origin.join("f"), "x").unwrap();
        git_in(&origin, &["add", "."]);
        git_in(&origin, &["commit", "-qm", "one"]);
        git_in(&tmp, &["clone", "-q", origin.to_str().unwrap(), "clone"]);
        let clone = tmp.join("clone");

        // Three worktrees of one clone, plus a second clone with one.
        let mut dirs = Vec::new();
        for n in ["a", "b", "c"] {
            let d = tmp.join(format!("wt-{n}"));
            git_in(
                &clone,
                &["worktree", "add", "-q", d.to_str().unwrap(), "-b", n],
            );
            dirs.push(d);
        }
        git_in(&tmp, &["clone", "-q", origin.to_str().unwrap(), "clone2"]);
        let clone2 = tmp.join("clone2");
        let d2 = tmp.join("wt-z");
        git_in(
            &clone2,
            &["worktree", "add", "-q", d2.to_str().unwrap(), "-b", "z"],
        );
        dirs.push(d2);

        // All three of the first clone's worktrees report the same common dir.
        let commons: Vec<_> = dirs.iter().filter_map(|d| common_dir(d)).collect();
        assert_eq!(commons.len(), 4);
        assert_eq!(commons[0], commons[1]);
        assert_eq!(commons[1], commons[2]);
        assert_ne!(
            commons[0], commons[3],
            "a different clone is a different target"
        );

        // So four worktrees collapse to two fetches, one per clone.
        let targets = fetch_targets(&dirs);
        assert_eq!(targets.len(), 2, "got {targets:?}");
        assert!(targets.contains(&dirs[0]));
        assert!(targets.contains(&dirs[3]));

        // A directory git can't resolve still gets its own fetch rather than
        // being silently skipped.
        let bogus = tmp.join("not-a-repo");
        std::fs::create_dir_all(&bogus).unwrap();
        let targets = fetch_targets(&[bogus.clone()]);
        assert_eq!(targets, vec![bogus]);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A clone on `main` must not be told its base branch is `master` (#102),
    /// including when it has no `origin/HEAD` — the local-refs fallback covers
    /// mlai, which doesn't. The clone's namespace, not its host, decides
    /// whether the SFA ticket conventions apply.
    #[test]
    fn default_branch_and_sfa_namespace() {
        let tmp = std::env::temp_dir().join(format!("cf-wt-default-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (origin, clone) = (tmp.join("origin"), tmp.join("clone"));
        std::fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "main"]);
        git_in(&origin, &["config", "user.email", "t@t"]);
        git_in(&origin, &["config", "user.name", "t"]);
        std::fs::write(origin.join("a.txt"), "one\n").unwrap();
        git_in(&origin, &["add", "."]);
        git_in(&origin, &["commit", "-qm", "first"]);
        git_in(&tmp, &["clone", "-q", origin.to_str().unwrap(), "clone"]);

        assert_eq!(default_branch(&clone), "main"); // from origin/HEAD
        git_in(&clone, &["remote", "set-head", "origin", "-d"]);
        assert_eq!(default_branch(&clone), "main"); // from the tracking refs
                                                    // An unknown directory can't be probed at all; master stays the floor.
        assert_eq!(default_branch(&tmp.join("nope")), "master");

        for (url, sfa) in [
            ("git@github.red.datadirectnet.com:sfa/k8s.git", true),
            (
                "ssh://bbell@cos-scm-00.colorado.datadirectnet.com:29418/sfa/qa",
                true,
            ),
            ("git@github.red.datadirectnet.com:ddn/mlai.git", false),
            (
                "ssh://git@gitlab.co-es.datadirectnet.com:31031/ddn-tools/stonehenge.git",
                false,
            ),
        ] {
            git_in(&clone, &["remote", "set-url", "origin", url]);
            assert_eq!(is_sfa_clone(&clone), sfa, "{url}");
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Work that already landed in mainline under a *different* SHA (rebased,
    /// squashed, cherry-picked) is finished, so its worktree is deletable — the
    /// SHA-counting check called it Pending forever (#97). Work mainline really
    /// doesn't have must still never be Clean: the delete is irreversible.
    #[test]
    fn merged_under_another_sha_is_clean_but_unique_work_is_not() {
        let tmp = std::env::temp_dir().join(format!("cf-wt-merged-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (origin, clone) = (tmp.join("origin"), tmp.join("clone"));
        std::fs::create_dir_all(&origin).unwrap();
        git_in(&origin, &["init", "-q", "-b", "master"]);
        git_in(&origin, &["config", "user.email", "t@t"]);
        git_in(&origin, &["config", "user.name", "t"]);
        std::fs::write(origin.join("a.txt"), "one\n").unwrap();
        git_in(&origin, &["add", "."]);
        git_in(&origin, &["commit", "-qm", "first"]);

        git_in(&tmp, &["clone", "-q", origin.to_str().unwrap(), "clone"]);
        git_in(&clone, &["config", "user.email", "t@t"]);
        git_in(&clone, &["config", "user.name", "t"]);

        // A worktree whose commit is applied to origin/master as a new commit —
        // same patch, different SHA, exactly what a merged ticket looks like.
        let done = tmp.join("clone-DONE");
        git_in(
            &clone,
            &[
                "worktree",
                "add",
                "-q",
                done.to_str().unwrap(),
                "-b",
                "done",
            ],
        );
        std::fs::write(done.join("b.txt"), "shared\n").unwrap();
        git_in(&done, &["add", "."]);
        git_in(&done, &["commit", "-qm", "the feature"]);
        let patch = Command::new("git")
            .arg("-C")
            .arg(&done)
            .args(["format-patch", "-1", "--stdout"])
            .output()
            .unwrap();
        std::fs::write(tmp.join("p.patch"), &patch.stdout).unwrap();
        git_in(
            &origin,
            &["am", "-q", tmp.join("p.patch").to_str().unwrap()],
        );

        // And one with work that exists nowhere else.
        let wip = tmp.join("clone-WIP");
        git_in(
            &clone,
            &["worktree", "add", "-q", wip.to_str().unwrap(), "-b", "wip"],
        );
        std::fs::write(wip.join("c.txt"), "unique\n").unwrap();
        git_in(&wip, &["add", "."]);
        git_in(&wip, &["commit", "-qm", "not upstream anywhere"]);

        assert_eq!(
            classify(&done),
            WtState::Clean,
            "patch already in mainline: deletable"
        );
        assert_eq!(
            classify(&wip),
            WtState::Pending,
            "work that exists nowhere else must never be Clean"
        );

        // The safety property the whole feature rests on.
        let (mut d, mut w) = (
            WtEntry {
                name: "done".into(),
                path: done.clone(),
                state: classify(&done),
            },
            WtEntry {
                name: "wip".into(),
                path: wip.clone(),
                state: classify(&wip),
            },
        );
        assert!(d.deletable(), "merged work can be cleaned up");
        assert!(!w.deletable(), "unique work is protected");
        d.state = WtState::Dirty;
        w.state = WtState::Unknown;
        assert!(!d.deletable() && !w.deletable());

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// A clone is never a linked worktree, so it can't reach the delete path —
    /// and `delete` refuses it outright even if asked directly (#83).
    #[test]
    fn clones_are_never_deletable() {
        let tmp = std::env::temp_dir().join(format!("cf-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let (clone, wt) = (tmp.join("clone"), tmp.join("clone-TICKET-x"));
        std::fs::create_dir_all(&clone).unwrap();
        git_in(&clone, &["init", "-q"]);
        git_in(&clone, &["config", "user.email", "t@t"]);
        git_in(&clone, &["config", "user.name", "t"]);
        std::fs::write(clone.join("a.txt"), "x\n").unwrap();
        git_in(&clone, &["add", "."]);
        git_in(&clone, &["commit", "-qm", "first"]);
        git_in(
            &clone,
            &["worktree", "add", "-q", wt.to_str().unwrap(), "-b", "side"],
        );

        assert!(
            !is_linked_worktree(&clone),
            "a clone is not a linked worktree"
        );
        assert!(is_linked_worktree(&wt), "the worktree is");

        // Discovery under the projects root offers the worktree, never the clone.
        let found = list_worktrees(&tmp);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "clone-TICKET-x");

        // Even handed the clone directly, delete refuses it.
        let forged = WtEntry {
            name: "clone".into(),
            path: clone.clone(),
            state: WtState::Clean,
        };
        let err = delete(&forged).unwrap_err().to_string();
        assert!(err.contains("not a linked worktree"), "got {err}");
        assert!(clone.join("a.txt").exists(), "the clone is untouched");

        // A worktree that isn't clean is refused too, whatever the caller says.
        let dirty = WtEntry {
            name: "clone-TICKET-x".into(),
            path: wt.clone(),
            state: WtState::Dirty,
        };
        assert!(delete(&dirty)
            .unwrap_err()
            .to_string()
            .contains("not clean"));
        assert!(wt.exists(), "and survives");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Uncommitted changes classify as dirty — the check that keeps real work
    /// out of the deletable set.
    #[test]
    fn dirty_worktrees_are_not_clean() {
        let tmp = std::env::temp_dir().join(format!("cf-wt2-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        git_in(&tmp, &["init", "-q"]);
        git_in(&tmp, &["config", "user.email", "t@t"]);
        git_in(&tmp, &["config", "user.name", "t"]);
        std::fs::write(tmp.join("a.txt"), "x\n").unwrap();
        git_in(&tmp, &["add", "."]);
        git_in(&tmp, &["commit", "-qm", "first"]);
        std::fs::write(tmp.join("a.txt"), "changed\n").unwrap();
        assert_eq!(classify(&tmp), WtState::Dirty);
        let _ = std::fs::remove_dir_all(&tmp);
    }
}
