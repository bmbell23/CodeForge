//! Jira ticket status for worktrees (#135).
//!
//! Worktrees are named `<repo>-<TICKET>-<desc>`, so the ticket a worktree
//! belongs to is already in its name. This fetches that ticket's status so the
//! manager can show it — a clean worktree whose ticket is closed is obviously
//! finished with, and a clean one whose ticket is still open is worth a look.
//!
//! **Advisory only.** Nothing here may authorise a delete. A closed ticket does
//! not prove a worktree holds nothing of value; `WtEntry::deletable()` — clean,
//! and pushed or merged — is what proves that, and it stays the only gate.
//!
//! Credentials come from the environment, which is where the DDN tooling
//! already puts them (`baoenv` exports them from OpenBao). The request is made
//! with `curl`, matching how CodeForge runs everything else, so there's no HTTP
//! stack to carry.

use std::path::Path;
use std::process::Command;

/// Jira instance. Overridable for anyone pointed at a different tenant.
fn base_url() -> String {
    std::env::var("JIRA_URL")
        .unwrap_or_else(|_| "https://ime-ddn.atlassian.net".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// `(email, token)` from the environment, or `None` when they aren't there.
///
/// Two spellings are accepted: `JIRA_EMAIL`/`JIRA_API_TOKEN`, which is what
/// `~/.config/ddn-mcp/env` exports here, and `JIRA_IME_*`, which is what the
/// request named. Whichever is set wins, so neither spelling is wrong.
pub fn credentials() -> Option<(String, String)> {
    let pick = |a: &str, b: &str| {
        std::env::var(a)
            .ok()
            .or_else(|| std::env::var(b).ok())
            .filter(|v| !v.trim().is_empty())
    };
    let email = pick("JIRA_EMAIL", "JIRA_IME_EMAIL")?;
    let token = pick("JIRA_API_TOKEN", "JIRA_IME_API_TOKEN")?;
    Some((email, token))
}

/// The ticket key embedded in a worktree's directory name, if there is one.
///
/// Worktrees are `<repo>-<KEY-NUMBER>-<desc>`, e.g.
/// `SFA/infra/infra-SFAP-108563-jqlens` -> `SFAP-108563`. The `SFAP-NONE`
/// convention (a worktree with no ticket) yields nothing, since there is no
/// issue to ask about.
pub fn ticket_of(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_str()?;
    let parts: Vec<&str> = name.split('-').collect();
    // Look for PROJECT followed by digits, anywhere after the repo name.
    parts.windows(2).find_map(|w| {
        let (key, num) = (w[0], w[1]);
        let key_ok = !key.is_empty()
            && key
                .chars()
                .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
            && key.chars().any(|c| c.is_ascii_alphabetic());
        let num_ok = !num.is_empty() && num.chars().all(|c| c.is_ascii_digit());
        (key_ok && num_ok).then(|| format!("{key}-{num}"))
    })
}

/// A ticket's status name (`In Progress`, `Closed`, …), or `None` when it can't
/// be determined — no credentials, no network, unknown issue. Bounded so a slow
/// Jira can't wedge the caller.
pub fn status(ticket: &str) -> Option<String> {
    let (email, token) = credentials()?;
    // The key goes into a URL; anything that isn't a key shape is not ours to
    // send anywhere.
    if !ticket
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return None;
    }
    let url = format!("{}/rest/api/3/issue/{ticket}?fields=status", base_url());
    let out = Command::new("curl")
        .args(["-s", "--max-time", "15", "-H", "Accept: application/json"])
        .arg("-u")
        .arg(format!("{email}:{token}"))
        .arg(url)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    status_from_json(&out.stdout)
}

/// Pull `.fields.status.name` out of an issue response. Separated from the
/// request so the parsing is testable without a network or credentials — the
/// live call could not be exercised here (OpenBao wasn't logged in), so this
/// half at least is pinned down.
pub fn status_from_json(body: &[u8]) -> Option<String> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let name = v.get("fields")?.get("status")?.get("name")?.as_str()?;
    (!name.is_empty()).then(|| name.to_string())
}

/// Whether a status means the ticket is finished with. The complement of
/// Pulse's `ACTIVE_STATUSES` isn't safe to treat as "done" — "Info Needed" is
/// neither active nor closed — so this matches the closed end explicitly and
/// calls everything else open.
pub fn is_closed(status: &str) -> bool {
    let s = status.trim().to_ascii_lowercase();
    matches!(
        s.as_str(),
        "closed" | "done" | "resolved" | "complete" | "completed" | "cancelled" | "canceled"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn reads_the_ticket_out_of_a_worktree_name() {
        let t = |p: &str| ticket_of(&PathBuf::from(p));
        assert_eq!(
            t("/p/SFA/infra/infra-SFAP-108563-jqlens"),
            Some("SFAP-108563".into())
        );
        assert_eq!(
            t("/p/auto-SFAP-107316-rc_build"),
            Some("SFAP-107316".into())
        );
        assert_eq!(t("/p/SFA/eng/eng-DDNDO-43-netbox"), Some("DDNDO-43".into()));
        // A worktree explicitly filed under no ticket has nothing to ask about.
        assert_eq!(t("/p/infra-SFAP-NONE-pulse-orphan"), None);
        // Plain clones and non-ticket directories.
        assert_eq!(t("/p/sfaos"), None);
        assert_eq!(t("/p/BMB/Notes/Notes"), None);
        assert_eq!(t("/p/dotfiles"), None);
    }

    #[test]
    fn parses_the_status_name_and_survives_junk() {
        let body = br#"{"fields":{"status":{"name":"In Progress","id":"3"}}}"#;
        assert_eq!(status_from_json(body), Some("In Progress".into()));
        // Anything unexpected yields nothing rather than a wrong answer: this
        // only ever advises, so "unknown" is the correct failure.
        assert_eq!(status_from_json(b"{}"), None);
        assert_eq!(status_from_json(b"not json"), None);
        assert_eq!(status_from_json(br#"{"fields":{"status":{}}}"#), None);
        assert_eq!(status_from_json(br#"{"errorMessages":["nope"]}"#), None);
        assert_eq!(
            status_from_json(br#"{"fields":{"status":{"name":""}}}"#),
            None
        );
    }

    #[test]
    fn closed_is_matched_explicitly_not_by_exclusion() {
        for s in ["Closed", "done", "Resolved", "COMPLETE", "Cancelled"] {
            assert!(is_closed(s), "{s} should read as closed");
        }
        // "Info Needed" is neither active nor closed; treating the complement of
        // "active" as done would call it finished, which it isn't.
        for s in [
            "In Progress",
            "Info Needed",
            "Test Ready",
            "In Code Review",
            "",
        ] {
            assert!(!is_closed(s), "{s} should not read as closed");
        }
    }
}
