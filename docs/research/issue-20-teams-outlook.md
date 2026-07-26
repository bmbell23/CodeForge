# Issue #20 — Teams / Outlook integration: feasibility

**Status:** research only (no feature code). Grounds in Microsoft Graph / Entra
docs and current (2025–2026) tooling.

## Verdict (read this first)

- **Outlook unread counter on the status bar is feasible** and is the right MVP —
  poll Microsoft Graph with a **delta query** on a timer, using **OAuth2
  device-code** auth. Small, self-contained, matches CodeForge's single-binary model.
- **The hard blocker is the DDN corporate tenant, not the code.** Everything below
  needs an **Entra (Azure AD) app registration in the DDN tenant** and, for Teams,
  **tenant-admin consent**. Many corporate tenants now actively **block the
  device-code flow** via Conditional Access (Microsoft's Sept-2024 anti-phishing
  guidance). Confirm with DDN IT *before* building anything — this is a go/no-go gate.
- **Don't build a mail/Teams client.** CodeForge's philosophy is to host existing
  processes in PTY panes. For mail, **launch an existing TUI** (Himalaya has a
  native Graph backend; aerc/neomutt work over IMAP+XOAUTH2). For Teams there is
  **no solid terminal client** — presence/notifications via Graph polling is the
  realistic ceiling.
- **Webhooks are out.** Graph change-notifications need a public HTTPS endpoint;
  a homelab box behind NAT can't host one sanely. Poll instead.

---

## 1. Microsoft Graph — the integration surface

All read-only, all delegated (acting as the signed-in user). Base:
`https://graph.microsoft.com/v1.0`.

### Mail (Outlook)
| Need | Endpoint | Scope |
|------|----------|-------|
| Unread count (cheap) | `GET /me/mailFolders/inbox` → `unreadItemCount` | `Mail.Read` |
| Recent messages | `GET /me/messages?$select=subject,from,receivedDateTime,isRead&$top=N` | `Mail.Read` |
| Efficient sync | `GET /me/mailFolders/inbox/messages/delta` → store `@odata.deltaLink`, replay it next tick for the diff only | `Mail.Read` |

For a status-bar badge, the single `mailFolders/inbox` call returning
`unreadItemCount` is the cheapest possible signal — one request per poll.

### Teams (chat / channels)
| Need | Endpoint | Scope |
|------|----------|-------|
| 1:1 & group chats | `GET /me/chats` | `Chat.Read` |
| Messages in a chat | `GET /me/chats/{id}/messages` | `Chat.Read` |
| Channel messages | `GET /teams/{id}/channels/{id}/messages` | `ChannelMessage.Read.All` |
| New-message signal | chat/message `delta` where supported, else poll | `Chat.Read` |

### Presence
| Need | Endpoint | Scope |
|------|----------|-------|
| Own presence | `GET /me/presence` → `availability`, `activity` | `Presence.Read` |
| Others' presence | `POST /communications/getPresencesByUserId` | `Presence.Read.All` |

**Scope summary:** `Mail.Read` and `Presence.Read` are ordinary delegated scopes
(no admin consent *inherent* to the permission). **`Chat.Read` /
`ChannelMessage.Read.All` require tenant-admin consent.** In practice a locked-down
corporate tenant often requires admin consent (or admin-only app registration) for
*all* of them regardless — see §2.

---

## 2. Auth in a terminal / headless context — and the tenant blocker

**Flow: OAuth2 device-code (`/devicecode`).** The right fit for a TTY app: no
localhost redirect/browser callback. Print a URL + short code, user authenticates
on any device, CodeForge polls `/token` until it gets access + refresh tokens.
MSAL supports it; in Rust you'd hit the endpoints directly or use a crate — but the
flow itself is simple.

**Token caching / refresh:** cache the **refresh token** encrypted at rest
(e.g. `$XDG_STATE_HOME/codeforge/`, ideally via the OS secret service). Access
tokens live ~1 hr; refresh silently. Requesting `offline_access` is what grants the
refresh token.

**The blockers (be honest with the user):**

1. **App registration in the DDN Entra tenant is mandatory.** You register a
   *public client* app, enable the device-code/"allow public client flows" option,
   and declare the delegated scopes. Many corporate tenants **disable user
   self-registration of apps** — so IT has to create it (or approve it).
2. **Admin consent.** Even where `Mail.Read`/`Presence.Read` don't strictly require
   it, tenants commonly set **"users can't consent to apps"**, which routes *every*
   scope through an admin approval. `Chat.Read` needs admin consent outright.
3. **Device-code flow is increasingly blocked.** Since Sept 2024 Microsoft ships a
   Conditional Access **"Authentication Flows"** condition and explicitly recommends
   blocking device-code flow (it was abused at scale by Storm-2372 phishing). A
   security-conscious tenant like DDN may **deny device-code entirely**, which kills
   this approach even with a valid app registration.

**Bottom line:** this is a **one-email-to-IT go/no-go** before any coding. Ask DDN
IT for: (a) an app registration (or permission to create one) with
`Mail.Read`, `Presence.Read`, `offline_access` (+ `Chat.Read` if Teams is wanted),
(b) admin consent for those scopes, (c) confirmation that device-code flow is
permitted by Conditional Access. If any is "no," the feature is dead for that tenant.

---

## 3. Notifications: push vs poll

**Push (Graph change-notifications / subscriptions / webhooks):** Graph POSTs to
**your public HTTPS URL** the moment mail arrives. It requires:
- a publicly reachable HTTPS endpoint with a valid cert,
- a validation handshake, subscription **renewal** every few days,
- responses < 10 s or Graph throttles and eventually **drops** the subscription.

On a **homelab box behind NAT** this means punching in inbound ports / reverse
tunnels / a hosted relay — operationally absurd for a status-bar counter, and a
security surface. **Reject webhooks.**

**Poll (delta query):** on a timer, replay the stored `@odata.deltaLink` (or just
re-read `unreadItemCount`). Outbound HTTPS only — works fine behind NAT.
- Graph limit is ~**10,000 requests / 10 min / app** plus per-user limits; 429s
  carry `Retry-After`. One request every **30–60 s** is nowhere near that.
- Delta returns only changes → cheap and throttle-safe.

**Recommendation: poll.** ~60 s interval for the unread badge; back off on 429.

---

## 4. Rendering inside the app

**Path (a) — CodeForge natively renders mail/Teams via Graph.** This is building a
mail client (list/read/threading/compose, MIME, attachments, search). Large, ongoing
maintenance, and off-philosophy. **Don't.** The *only* native piece worth doing is
the non-interactive **status-bar counter** (§5).

**Path (b) — host an existing TUI in a PTY pane.** This is exactly what CodeForge
already does for nvim/shell/claude. Survey:

**Mail — viable:**
- **Himalaya** (Rust CLI, `pimalaya`) — **has a native Microsoft Graph backend**
  (drop in an OAuth2 bearer token instead of IMAP/SMTP). Best philosophical fit
  (Rust, single binary, envelope/read/write from terminal). Note: primarily a
  **CLI**; a `himalaya-tui` exists but is early. Good for scripted/paned use today.
- **aerc** — mature TUI, documented **Office365 via IMAP + XOAUTH2** (needs an
  OAuth2 token helper). Runs cleanly in a pane. Solid choice for an interactive
  mail pane.
- **neomutt** — works with O365 via IMAP+XOAUTH2 (`mutt_oauth2.py` device-code
  helper) or a local `mbsync`+`cyrus-sasl-xoauth2` mirror. Powerful but the OAuth
  setup is the classic "yak-shave."
- **meli** (Rust TUI) — IMAP; OAuth2 story weaker than aerc/neomutt.

All of the above still depend on the **same tenant OAuth grant** from §2 (IMAP+OAuth
must be enabled tenant-side; Microsoft killed basic auth).

**Teams — not viable in a PTY (be plain about this):**
- `teams-for-linux` (IsmaelMartinez) is the usual Linux client but it's **Electron**
  wrapping the web app — **not a TUI**, can't live in a pane.
- Terminal clients exist but are **unofficial, reverse-engineered, and fragile**:
  `fossteams/teams-cli` (Go/tview), `nospor/teams-tui` (Rust), `ttyms` (Rust,
  presence + chat). They break when Microsoft changes internal APIs and are risky
  to point at a corporate tenant. **Do not depend on these.**
- Realistic Teams ceiling for CodeForge: **read-only presence + unread signal via
  Graph polling** on the status bar — not an in-pane Teams client.

---

## 5. Recommendation — phased plan

**Phase 0 — Unblock (do first, ~0 code).** Email DDN IT: app registration + admin
consent for `Mail.Read`/`Presence.Read` (`+Chat.Read` if Teams) + confirm
device-code flow is allowed by Conditional Access. **Everything else is blocked on
this.**

**Phase 1 — MVP: Outlook unread badge (S–M).** Device-code auth + encrypted
refresh-token cache; poll `GET /me/mailFolders/inbox` every ~60 s; render
`✉ N` on the status bar (the status bar is already on the roadmap). Config in the
planned TOML. ~1 module + a small OAuth helper. **This is the whole ask (a).**

**Phase 2 — Mail in a pane (S, integration not code).** Add a launcher entry that
opens **Himalaya** (or **aerc**) in a terminal pane, reusing the Phase-1 token.
Almost entirely config/plumbing since PTY panes already exist — delivers ask (b)
without CodeForge becoming a mail client.

**Phase 3 — Teams presence / signal (M, stretch, gated on admin consent).** Poll
`/me/presence` (own status) and optionally a `/me/chats` unread signal; show on the
status bar next to mail. **No in-pane Teams client** — the terminal options are too
fragile for a corporate tenant.

**Hard blockers to keep flagged:** (1) tenant admin consent + app registration,
(2) device-code possibly blocked by Conditional Access, (3) webhooks impractical
behind NAT → poll only, (4) no trustworthy terminal Teams client.

---

## Sources
- [Graph permissions reference](https://learn.microsoft.com/en-us/graph/permissions-reference)
- [OAuth 2.0 device authorization grant (Entra)](https://learn.microsoft.com/en-us/entra/identity-platform/v2-oauth2-device-code)
- [Delta query overview](https://learn.microsoft.com/en-us/graph/delta-query-overview)
- [Change notifications overview](https://learn.microsoft.com/en-us/graph/change-notifications-overview)
- [Authentication flows as a Conditional Access condition](https://learn.microsoft.com/en-us/entra/identity/conditional-access/concept-authentication-flows)
- [Himalaya (pimalaya) — CLI to manage emails](https://github.com/pimalaya/himalaya)
- [aerc — Microsoft Office365 provider guide](https://man.sr.ht/~rjarry/aerc/providers/microsofto365.md)
- [teams-for-linux (unofficial Electron client)](https://github.com/IsmaelMartinez/teams-for-linux)
- [fossteams/teams-cli](https://github.com/fossteams/teams-cli) · [nospor/teams-tui](https://github.com/nospor/teams-tui) · [ttyms](https://lib.rs/crates/ttyms)
