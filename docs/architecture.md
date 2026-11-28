# claude-sync: architecture

This document describes claude-sync 2.0.0 as built. 2.0.0 is a from-scratch rewrite of 0.1.x (called v1 below); the repo formats, the user-facing texts and the command line are the same, the code and the local state are new (§24 to §26). The 0.1.x code stays on the `main` history before the rewrite and is not in the tree.

## 1. What it is

A small desktop app (Windows and macOS first; Linux ships as a beta, untested until a runtime check) that lives in the tray, starts at login and keeps Claude Code setups in sync:

- **Groups.** A group is a set of friends who share Claude Code settings: `settings.json` keys, skills, subagents, slash commands, output styles and `CLAUDE.md` instructions. One person **hosts** the group: the group repo lives on their GitHub account. Everyone else **joins**. Only the host changes the group: shared settings, skills, `CLAUDE.md` and the member list. A member's edits stay on their machine; they propose a change as a pull request to the group repo, and the host merges it (§5a).
- **Devices.** Each person can run the app on as many PCs as they like. Every device signs in with the person's own GitHub account and has full access.
- **Personal memory.** A person's own `CLAUDE.md` text and Claude's memories follow them to their other devices through a private repo of their own, never through a group (§16).
- **Live.** The app watches the remote for changes and applies them within about a minute. It watches the local files it manages, and when you edit one it asks whether to share the change with the group.
- **Updates.** The app checks for new versions and updates itself.
- **Feedback.** A feedback form sends a description, and optionally the app's recent logs and a screenshot. The relay opens a public issue on this repo under the user's GitHub account, put into the nearest sprint, and stores the logs and screenshot in a private repo. A triage script the maintainer runs locally has Claude Code write up the report; it is posted as a comment by the triage GitHub App bot, not by the maintainer (§12).

It costs nothing to run. GitHub (free private repos, OAuth device flow) holds all the data. A free Vercel project (the **relay**) does the few things that need a secret: serving updates from this repo's releases and filing feedback issues.

## 2. Components

```
crates/engine/      Pure Rust library: formats, planner, merges, settings, CLAUDE.md review, clashes, secret scan. No I/O.
crates/github/      The one GitHub client (REST, ETag cache, rate limits, request counts), device flow, token storage, the engine's `Remote`.
crates/store/       SQLite store (`claude-sync.db`), its migrations and the one-time import of the v1 state files.
crates/fs/          The machine: `Disk` on real folders, the executor and its journal, apply, watcher, sync pass, OS tasks, SSH peers, logging.
crates/actions/     Every user action, the actor that runs them, the snapshot, the schedule and the v1 hand-over. One implementation for both shells.
crates/cli/         The command tree and `run`; the headless binary `claude-sync-cli` (daemon, systemd/cron service, token file).
app/src-tauri/      Tauri 2 shell: tray, window, autostart, notifications, updater, screen capture. The same binary answers the CLI words.
app/ui/             The window: React 19 and TypeScript with Vite; `src/generated/` holds the Rust types (ts-rs, `scripts/gen-types.sh`).
relay/              Vercel project (TypeScript functions): /api/update, /api/download, /api/feedback, /api/health, /install
scripts/            Local checks, the version bump, the latest.json builder, the local Windows release script
templates/user-repo Template of a group repo
```

Dependencies point one way: `engine` has none of ours; `github`, `store` and `fs` depend on `engine`; `actions` on all four; `cli` and the app on `actions`. The engine talks to the outside only through the traits in `crates/engine/src/traits.rs` (`Remote`, `Store`, `Disk`, `Clock`), so every rule is tested with fakes and a temp folder.

GitHub Actions run on GitHub-hosted runners only (free for a public repo): `ci.yml` runs the checks of `scripts/check.ps1` on Ubuntu, Windows and macOS for every PR and push to `main`, `bench.yml` holds the budgets (installer 15 MB, UI JavaScript 200 KB, 424 idle requests an hour, §6), and `release.yml` builds releases (§13). No self-hosted runners, no AI jobs.

## 3. Identity and auth

- The app signs in with the **GitHub OAuth device flow** (no client secret in the app). It needs a GitHub OAuth App with device flow enabled. Its client ID is public and is compiled in (`CLAUDE_SYNC_GITHUB_CLIENT_ID`).
- Scope: `repo` (needed to read and write private repos and to manage collaborators and invitations) and `read:user`. This is broad. A GitHub App with per-repo access is in the backlog.
- **Every call works with an App user token** (docs.github.com, "Permissions required for GitHub Apps"): `POST /user/repos` (Administration write + the account permission "Repository creation" write, user tokens only), collaborators and invitations (Administration write; `GET collaborators` Metadata read), rulesets (Administration write; list needs Metadata read), transfer (Administration write, user token only), `PATCH /user/repository_invitations/{id}` accept and `GET /user/repository_invitations` (Administration write / read, user token only), topics (Administration write), contents, git blobs/trees/refs and pull merge (Contents write), reviews (Pull requests write), `GET /user/repos` (Metadata read, only repos with the App installed). The one catch is coverage: a repo created after the install is invisible to the token (404) unless the installation is on "All repositories". With "Only select repositories" the user must add each new group repo (and the personal repo `<login>/claude-sync-personal`) to the installation, so the recommended install is **All repositories**, which still removes the OAuth `repo` scope's reach into code (the App holds no Contents access beyond what we list, and can be limited per account), and shows in the coverage check. Accepting an invitation to someone else's repo gives the invitee access, but the App must also be installed on the *owner's* account for that repo to be reachable: an invitee's token cannot see a repo whose owner never installed the App. That is the hybrid limit: a group is usable with App auth only when its host installed the App on the repo, which the coverage check reports as missing.
- The tokens are stored in the OS keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service such as GNOME Keyring or KWallet; Linux is beta) through the `keyring` crate, under service `claude-sync`, account = GitHub login, as one entry: a JSON object with the access token and, when GitHub sends them, the refresh token (`ghr_…`) and both expiry times. An entry saved by 0.1.1 or earlier is the bare access token and still loads. Tokens are never written to a file or a log.
- **Token refresh.** When the OAuth App has expiring user tokens on, the access token lasts 8 hours and the refresh token 6 months. The app refreshes 5 minutes before the access token expires, and again after a `401`, with `POST https://github.com/login/oauth/access_token` (`grant_type=refresh_token`, `client_id`, `refresh_token`; no client secret, as in the device flow), then saves the new pair to the keychain and retries the call once. All GitHub calls of a sign-in share one session (`github::Session`), so concurrent calls wait for a single refresh instead of each spending the refresh token (GitHub rotates it on every use).
- **Expired sign-in.** A `401` that a refresh cannot cure (no refresh token, or GitHub refuses it, e.g. `bad_refresh_token`) and that `GET /user` confirms means GitHub no longer accepts the sign-in. The app then signs out as *expired*: background GitHub calls (sync loop, update check, feedback outbox) stop, the dead tokens are removed from the keychain, the store and the groups stay, one notification says so, and the window shows "Your GitHub sign-in expired" with a **Sign in again** button instead of an error every minute. The tray says "Sign-in expired". Signing in again clears it and syncing resumes where it left off. A network error or a GitHub 5xx during a refresh is not an expiry: the call fails and the next pass tries again.
- Each device signs in on its own. Signing out on one device does not affect the others.

## 4. Group repo

A private repo on the host's account, created by the app. Default name `claude-sync-data` (for example `<owner>/claude-sync-data`). It gets the repo topic `claude-sync-group`, which is how the app recognises group repos.

The data repo is separate from this code repo. `MehdiMamas/claude-sync` is the app's public code repo; members are invited only to their group's data repo. The app reaches this repo only through the relay (updates, feedback).

```
claude-sync.json        group file (below)
README.md               what this is and how to install the app (generated)
CLAUDE.md               shared instructions, imported into each member's ~/.claude/CLAUDE.md
settings.shared.json    keys merged into each member's ~/.claude/settings.json
skills/<name>/...       shared skills
agents/*.md             shared subagents
commands/*.md           shared slash commands
output-styles/*.md      shared output styles
plugins/<marketplace>/  shared plugin marketplaces (below)
schedules.json          scheduled Claude Code jobs (§17); repo only, never placed in ~/.claude
```

`claude-sync.json`:

```json
{
  "schema": 1,
  "name": "Alice & friends",
  "members": ["alice", "friend-login"],
  "sync": { "settings": true, "claudeMd": true, "skills": true, "agents": true, "commands": true, "outputStyles": true, "plugins": true },
  "autoAcceptClaudeMd": true
}
```

- The host is whoever owns the repo. It is not stored in the file, so a repo transfer moves the host role without edits (§5b).
- `autoAcceptClaudeMd` (default `true`; absent means true) is the group's "Auto-accept CLAUDE.md changes" (§7a). Only the host changes it.
- `members` is the list of GitHub logins that should have access. Logins compare case-insensitively.
- Unknown keys are kept on rewrite. A `schema` newer than the app knows means read-only until the app updates.

### Sharing your CLAUDE.md

A person's own `~/.claude/CLAUDE.md` often holds the very instructions a group wants. The app offers it in two places:

- **Create a group.** The form has "Share my CLAUDE.md with the group" (checked when there is text) and an editable preview. The group repo's first `CLAUDE.md` is the text in the preview; unchecked or blank, it is `# <group name>`.
- **A group you host.** "Share my CLAUDE.md" on the group screen, next to "Share one of your skills", opens the same preview. The text is appended under a `## From <login>` heading, committed as `<login>@<device>: share CLAUDE.md`, and pushed like any other change (a concurrent push means fetch and try again, §6). Text the group's `CLAUDE.md` already has is not appended again, and the app says so. A group with `sync.claudeMd` off refuses it.

**Privacy.** The preview is the user's own text: everything outside the managed block (§7), without group import lines, the same text personal memory syncs (§16). Secrets are redacted with the §9 patterns before it is shown and again before it is committed, and the commit still goes through the secret scan. One line above the preview says: "Everyone in the group will see this text, so remove personal details such as emails, names and tokens." What is committed is what the preview holds after the user's edits.

**No double instructions.** Once the group has the text, the managed block imports it, so Claude would read it twice. "Remove it from my own CLAUDE.md (it now comes from the group)" is on by default. It removes the shared lines (blank lines aside, trailing spaces ignored, each shared line once) from the user's own text only; the managed block and import lines stay. It runs only when this device applies the group's `CLAUDE.md` and the file is backed up to `~/.claude-sync/backups/<UTC time>/` first. With personal memory on, the shorter text reaches the person's other devices too; they get the instructions back from the group once they join it.

**Clashes.** Under the preview, the app lists the lines that would say the same thing as, or the opposite of, a line Claude already reads from that group or from the user's other groups (§7b), with the group's name and its line ("\"Always answer in English\" contradicts Acme team: \"Never answer in English\""). It is a warning: the user can still share.

### Copying to another group

A group's page has "Copy to another group" when the user is in at least two groups. It copies from the group on screen (A) to the group the user picks (B):

- **`CLAUDE.md`**: A's text (redacted with the §9 patterns) is cut into sections at its headings and into lines (a fenced code block is one item). The user ticks whole sections or single lines; the text B will get is an editable preview built from the ticks. It is appended to B's `CLAUDE.md` under `## From <A's name>`, and the preview lists the lines that would clash with B or the user's other groups (§7b; A itself is left out).
- **Skills and agents**: one box per skill folder and agent file. One B already has the same is shown as there already; one B has in another version is refused ("Acme team already has a different skill named deploy"), never overwritten.
- **Settings**: one box per value of A's `settings.shared.json` (JSON pointer). Arrays are merged into B's array (union); a plain value B already sets differently is refused and shown with B's value.
- Kinds B does not sync (`sync.*` off) are shown as such and not offered.

Everything goes to B in one commit, `<login>@<device>: copy from <A>: CLAUDE.md, skills/x, agents/y.md, 2 settings`, through the same path as sharing the user's own content: redaction, the secret scan, and fetch and try again on a concurrent push (§6). B's other members get the new `CLAUDE.md` lines as changes to review (§7a). Text B already has is not appended again.

**Write access.** Copying needs B to be a group the user hosts, because only the host changes a group; B's picker lists only those groups. The preview also asks GitHub (`permissions.push` on the repo); when the user can only read B, the card says so plainly ("You can't push to Acme team: your GitHub account can read it but not write to it. Ask its host (acme-bot) for write access.") and offers no Copy button. A push GitHub refuses with 403 gets the same message.

### Plugins

A group can bundle Claude Code plugin marketplaces (issue #30). `plugins/<marketplace>/` is a marketplace folder exactly as Claude Code expects it: `.claude-plugin/marketplace.json` plus the plugins it lists.

- **Valid marketplace.** The folder name matches `[a-z0-9][a-z0-9-]*`, and `plugins/<marketplace>/.claude-plugin/marketplace.json` exists with `"name"` equal to the folder name. Otherwise the marketplace is not installed and the sync report says why.
- **Allowlist.** Any path below `plugins/<marketplace>/` syncs, dot-files included (`.claude-plugin/`, `.gitignore`), except `.git`, `.DS_Store`, `.env` / `.env.*` and anything under `node_modules`. The usual path checks still apply (no `..`, no Windows-reserved names).
- **Generated and ignored.** `.gitignore` files inside a group tree are honoured when looking for local edits, and Claude Code's generated mod files (`.claude-plugin/types/**`, `mods/<name>/tsconfig.json`) are never synced.
- **Size cap.** A plugin file over 1 MiB is neither placed nor shared. It is reported, and Share refuses it.
- **Trust.** Plugins run code (hooks, MCP servers, scripts) on every member's machine. The join screens and the group README say so: "Plugins in this group run code on every member's machine; only join groups whose members you trust."
- Where the files go and how Claude Code finds them is in §7 and §8. Which plugins are on stays in `settings.shared.json` (`enabledPlugins`).

## 5. Membership as code

Only the host changes the member list, even though the list lives in `claude-sync.json` in the group repo:

1. The host adds or removes a login in the app. The app commits the change to `members` in `claude-sync.json`. A member has no add or remove control.
2. The host's app reconciles on every sync: it invites (`PUT /repos/{o}/{r}/collaborators/{login}`, permission `pull`, read-only) every listed login that is not a collaborator and has no pending invitation, and removes collaborators and cancels pending invitations for logins no longer listed. The host is never removed. If the host's app is offline, reconciliation happens the next time it runs.
3. GitHub sends the invitation by email and notification. The invitee's app (if installed) lists pending invitations to repos with the `claude-sync-group` topic (`GET /user/repository_invitations`) and offers **Accept**. When access appears, the app sets the group up on that device without further questions: snapshot, apply, start watching.
4. A member whose access disappears gets the group's managed content removed from their devices (with a backup) and is told why.

### 5a. Only the host changes a group

- **The app.** Members are invited with read (pull) access, and only the host's app pushes to a group repo. A member's local edit to a managed file is never pushed: the notice offers **Undo** but no **Share with group**, the group screen shows "Only <host> can change this group. Your edits stay on this machine.", and the core refuses a member's change with `Error::HostOnly` ("Only <host> can change <group>. Your change stays on this machine; ask <host>, or propose it as a pull request."). A member who wants a change opens a pull request on the group repo; the host reviews and merges it.
- **GitHub.** The host's app creates a repository ruleset on the default branch that blocks update, deletion and non-fast-forward pushes for everyone except repository admins. If GitHub refuses (for example a private repo on a free plan), the app still only pushes from the host's machine, and the host sees "GitHub can't lock this repo on your plan; the app still only sends changes from your machine." (`GroupStatus.hostLock` is `on`, `unavailable`, or null for members.)
- **Proposals.** A group can opt into proposal mode, which lets members send changes through the host's review (§5c).

### 5b. Transfer and rename

- **Transfer (host only).** The group screen has "Hand over the group": pick a current member and confirm. The app calls `POST /repos/{o}/{r}/transfer {new_owner}` (`Engine::transfer_group`, refused for non-members, for yourself and for non-hosts). GitHub emails the new owner, who must accept; until then nothing changes. Afterwards the new owner is the host and you are a member. Before asking, the app makes sure the former host's login is in `members`, so the new host's reconcile (§5) keeps their read access. GitHub also keeps the former owner as a collaborator of a transferred repo; the member list is what the app relies on.
- **Following a move.** GitHub redirects a renamed or transferred repo. On every sync pass, before the groups are synced, the engine asks each joined repo for its current `full_name` (`GET /repos/{o}/{r}`, `crates/github/src/remote.rs`). If it differs from the stored id (case-insensitive), `actions::groups` moves every local trace to the new id: `groups/<old>/` and `conflicts/<old>/` (renamed, never deleted) and, in one store transaction, every row keyed by the repo (`groups`, `bases`, `base_files`, `review_items`, `jobs`, `notifications`, `conflicts`), where the id is replaced in place so the group order is kept (job tasks are registered again on the next job reconcile). Writes of the repo (commits, transfer, reconcile) always use the resolved new name, never the stale one.
- **Resumable.** The steps are idempotent and the store transaction is committed last, so a run interrupted halfway (folder moved, store not yet updated) finishes on the next sync; an id that is already migrated is a no-op. The host role is recomputed on the next sync from the repo owner and the signed-in login, so it flips on its own. Other groups' stored collisions that name this group's owner are recomputed by their next sync.
- **Notice.** The app shows one notification: "<old> moved to <new>; claude-sync follows it." (`SyncReport.moved`).

### 5c. Proposals

- **Setting.** A group has an optional `proposalMode` flag, stored in `claude-sync.json` in the group repo. Only the host can toggle it. Members, settings and the toggle itself stay direct host changes.
- **Mode off.** Nothing changes: members get `HostOnly` (§5a).
- **Mode on.** Any change, from a member or the host, is pushed to a branch `proposal/<login>-<UTC yyyymmddHHMMSS>` and a pull request against the default branch is opened, titled `login@device: summary`. The ruleset (§5a) protects only the default branch, so members can push these branches.
- **Proposals section.** The app lists open proposals with the author, the changed files with their diff, and the approvals. Each has **Approve** and **Decline**.
  - A member's **Approve** is a GitHub review approval.
  - The host's **Approve** on a member's proposal squash-merges it.
  - **Decline** closes the PR with a comment.
- **Host's own proposals.** These need one approval from another group member, and the host's app merges them on its next sync.
- **Stale proposals.** If a file in a proposal changed on the default branch since the proposal branched, the proposal is "needs an update" and is never merged.
- **Local edits.** After proposing, the edit stays a local edit and is not proposed again on each sync. Sharing a skill as a proposal keeps the local folder (no backup move).

## 6. Sync engine

The engine talks to GitHub's REST API only. It never needs `git` on the machine. The planner (`crates/engine/src/plan.rs`) is pure: it takes the base, the local files and the remote files and returns what to write, delete, push and report; `crates/fs/src/exec.rs` carries that out (§7).

**Base snapshot.** For each group the store keeps the last synced state: the commit sha, the head ETag, and per file its git blob sha and the hash of the bytes placed locally (`bases`, `base_files`, §25). The bytes of the base are kept in `base_files.bytes` (the personal repo's in `personal_base_files`) and are the common ancestor for three-way merges. A base that came from a v1 import has hashes only; its bytes are read from the v1 `groups/<id>/base/` folder until the first v2 pass saves them (§26).

**One pass** (`crates/fs/src/pass.rs`; `actions::sync` calls it). Under `<state>/sync.lock` (an exclusive file lock shared by the app and the CLI; a background pass that finds it taken is skipped, `sync` by hand waits up to 60 s):

1. Each joined group in rank order (§10): ask for the head with `If-None-Match`; a `304` ends the group's pass for free. On a new head read the tree, take every file whose blob sha equals the base's from the base, download only the other blobs, then plan, execute and save the base last (a crash before that repeats the pass, never half-applies it).
2. The `settings.shared.json` merge (§8) and the managed CLAUDE.md block (§7, §7a).
3. The personal repo (§16), then SSH peers (§21).
4. Notes, conflicts and the request counts are written to the store.

Pushing is only by **Share** (§5a): a pass never sends a local edit.

**Merge, per file** (base B, local L, remote R):
- L == B: take R. R == B: keep L (a local edit). L == R: nothing to do.
- All three differ: if both are JSON objects (`settings.shared.json`, `claude-sync.json`), merge key by key three-way. A key both sides changed differently: remote wins. Otherwise it is a **conflict**: remote wins, the local version is saved to `~/.claude-sync/conflicts/<group>/<UTC time>/<path>` (a row in `conflicts`), and the app shows it.
- Deletes follow the same table, with "missing" as a value.

**Pushing.** Create blobs, a tree on the parent's tree, a commit whose parent is the remote head, then `PATCH` the ref with `force: false`. A `422` (someone else pushed) means fetch, merge and try again, at most 3 times. Never force. Before any commit the secret scan (§9) runs on every added or changed file; a hit blocks the commit, names the file, line and pattern, and never prints the value. Commit messages are `<login>@<device>: <summary>`.

**Reverting a commit (host only, §5a).** `Command::RevertCommit` undoes one commit of the activity feed with a NEW commit on the default branch (history is never rewritten). Each allowlisted file the commit touched is put back to its content at the commit's parent. If any of those files changed again since, the whole revert is refused and nothing is committed; the error names each file. Merge commits and commits of 300 or more files are refused.

### GitHub caching and request budget

- **One pipeline.** Every GitHub call goes through `crates/github/src/client.rs`. It retries connect failures, GET timeouts and GET 502/503/504 three times with a 0.5 s doubling wait, never retries a non-GET, and on a `401` renews the token once and asks `GET /user` before declaring the sign-in expired (§3).
- **ETag cache.** An in-memory map of URL to ETag and body (256 entries, a new URL on a full cache clears it). A repeat GET sends `If-None-Match`; a `304` returns the cached body and costs nothing against the rate limit. Non-GET calls and answers without an ETag are never cached. The head ETag of each repo is also stored (`bases.etag`), so the first check after a restart can still get a `304`.
- **Request counts.** Each request is counted by caller, endpoint (ids and names turned into placeholders) and result (`200`, `304`, `other`). Every core caller names itself with `with_caller("name", ...)`, else it counts as `other`. The shell flushes the counts into the `requests` table about once a minute; `claude-sync status --json` (`requests`) shows them.
- **Rate limits.** Kept per resource (`core`, `graphql`), read from `x-ratelimit-resource`. A `403` or `429` with `Retry-After`, a `403` naming the secondary limit (60 s), a `200` that used the last request (`x-ratelimit-remaining: 0`) set one "limited until" time for that resource. While it is ahead no request goes out; the schedule (§24) waits until the reset and the status carries `rateLimitedUntil`. The error reads "GitHub limit reached, resumes at <local time>".
- **Budget.** The background tick checks the head of each group repo and the personal repo (conditional GET) and the open pull requests of each group. An idle hour at a one-minute tick with 3 groups and the personal repo costs 424 requests, all answered `304` (0 primary points). `crates/github/tests/idle.rs` and `bench.yml` fail when it is more. Slower checks: the rename check (`GET /repos/{o}/{r}`) once a day per group, pending invitations every 15 minutes, the member reconcile after a pull or push.
- **Fetching only what changed.** Blobs are kept in memory by sha; only paths whose blob sha changed are downloaded. After a push the saved base is the pushed head, so the next tick does not read its own commit. The host's proposal check (§5c) reads a PR's files and reviews only when its head sha or `updated_at` changed.

## 7. Apply layer

Moves the merged group state into `~/.claude` without touching anything it does not own.

- **Managed files are copies, not links.** Copies work the same on every OS, need no admin rights, and let the app see local edits. The store (`base_files.local_hash`, §25) records every file the app wrote: path, group, SHA-256.
- Targets: `skills/<name>/**` → `~/.claude/skills/<name>/**`; `agents/*.md`, `commands/*.md`, `output-styles/*.md` → the same folder under `~/.claude`; `plugins/<marketplace>/**` → `~/.claude-sync/marketplaces/<marketplace>/**` (outside `~/.claude`, so Claude Code's own plugin cache is never touched).
- **Ownership rules.** The app writes or deletes a path only if the manifest says it owns it. A path that exists but is not in the manifest belongs to the user: the group file is skipped and reported as a collision. Two groups that ship the same path: the group higher in the list (§10) wins and the other is reported.
- **Marketplaces.** The folder under `~/.claude-sync/marketplaces/` is the app's, so a stray file in the way is backed up and replaced, not reported. A marketplace belongs to the group highest in the list that ships it: a lower group gets a collision on `plugins/<marketplace>` and none of its files for it are placed. When the owner leaves, or another group is moved above it (§10), the next sync of the other group takes it over. The manifest records the owner of each marketplace and the `settings.json` entry the app set for it (§8).
- **Reload.** Claude Code reads plugins when a session starts. When an apply, join or leave changes a plugin file or a marketplace entry, the sync report carries `pluginsChanged` and the app says: "Plugins changed. Run /reload-plugins in open Claude Code sessions, or restart them."
- **CLAUDE.md.** Each device keeps its own copy of the group's `CLAUDE.md` at `~/.claude-sync/groups/<owner>__<repo>/CLAUDE.md`. Changes other members make reach it only when the user accepts them (§7a). `~/.claude/CLAUDE.md` gets one managed block at the top, with one import line per group:
  ```
  <!-- claude-sync:begin (managed, do not edit inside) -->
  @~/.claude-sync/groups/<owner>__claude-sync-data/CLAUDE.md
  <!-- claude-sync:end -->
  ```
  Everything outside the block is the user's. Lines that say the same thing as, or the opposite of, a line in another group or the user's own text are listed on the group page (§7b).
- **Local edits.** A file watcher (debounced, 2 s) on managed paths compares hashes against the manifest. A changed managed file shows a notice: **Share with group** (commit it; the host only, §5a), or **Undo** (restore the group's version). There is no per-device opt-out. A manifest from an older version that lists paths this device had opted out of is migrated on its first apply: each such path follows the group again, the local copy goes to `~/.claude-sync/backups/<UTC time>/`, and one notice lists them. A new local skill can be shared from the app (host only). The watcher also covers `~/.claude-sync/marketplaces/`: an edited plugin file, or a new file inside a marketplace folder a group placed, gets the same notice, and Share commits it under `plugins/<marketplace>/`. Undo of a new file removes it (a copy goes to the backups).
- Every write goes through write-to-temp-then-rename. Before the first change to an existing user file (`CLAUDE.md`, `settings.json`) the app copies it to `~/.claude-sync/backups/<UTC time>/`.

## 7a. Reviewing changes to a group's CLAUDE.md

A person can be in several groups, and a line from one group's `CLAUDE.md` may be copied into another. What other people do to a group's `CLAUDE.md` must never silently change what this user's Claude reads. The group decides how that goes (the switch below). Where review is on, every line someone else adds or removes waits until the user accepts or declines it; the default, auto-accept, applies other members' changes at once, so a group of people who trust each other does not click through every line.

**Items.** When a sync finds that the group's `CLAUDE.md` changed, the change is split into items: one block of added lines, or one block of removed lines, from one commit. Each item records its author (the GitHub login of the commit), the commit, its message and time, and up to two lines of context on each side. Items are applied by content, not by line number: a removal finds its lines, an addition goes next to the lines it followed. An item that has nothing left to do (someone else already made the same change) is dropped.

**Whose change.** The app reads the commits between the last head it synced and the new one (`GET /compare`).
- Every commit is this account's (another of the user's devices, or a clone pushing with the same login): the change applies at once.
- Every commit is one other member's: all of it waits for review, attributed to them.
- A mix: the app reads `CLAUDE.md` at each commit (at most 20) and splits the change per commit, so the user's own steps apply and the others' wait. Past 20 commits, or when one of them cannot be read, the whole change is attributed to the last other member and waits.
- The history cannot be read at all: the change waits as "Someone"'s. Nothing applies unasked.

**State.** Per group in `~/.claude-sync/groups/<owner>__<repo>/claude-md-review.json` (`format: 2`, written atomically): `group` (the group's text as last seen), `accepted` (the group's text with this user's decisions applied), the pending items, the declined items (at most 500 kept), and this device's `review` choice (`null`: follow the group; format 1 files, which could not tell a choice from the old default, load as `null`). The copy that Claude Code imports holds `accepted`. The file watcher ignores the review file, and local edits (§7) are measured against `accepted`, so a declined line never shows as a local edit.

**Accept and decline.** Accept patches the item into `accepted` and the copy. Decline leaves both alone and remembers the item: the app never asks about it again, and the next change in the same place is a new item. A deleted line the user declined stays in their copy. A line the user declined to add stays out.

**Declined differences are never pushed.** When the user shares an edit of their copy, the edit is rebased onto the group's current text: only what the user changed by hand goes to the group, not the lines they declined. Declining is private; it never undoes another member's change for the rest of the group.

**The user's own edits.** An unshared edit of the copy stays; an accepted item is patched into it. Undo (§7) returns the copy to `accepted`, not to the group's text. **Use the group's version** (group page, shown when the copy differs from the group's text and nothing is pending) sets `accepted` and the copy to the group's text and forgets the declined items.

**The switch.** "Auto-accept CLAUDE.md changes" on the group page is the group's setting, `autoAcceptClaudeMd` in `claude-sync.json`, on by default (an absent key counts as on, so existing groups get it as members update). Only the host can change it (`Error::HostOnly` for a member); it is committed as `<login>@<device>: auto-accept CLAUDE.md on` or `off` (fetch and try again on a concurrent push, §6). On: every device applies other members' changes to its copy at once. Off: each member reviews them first, as above. Each device can override this on the group page ("On this device": follow the group, review first, or auto-accept); the choice is kept only on that device. A device reviews when its choice says so, else when the group's setting is off; if the group file cannot be read, auto-accept applies. When review turns off for a device, by its own choice or because the group turned on auto-accept, the items pending are accepted in that sync (the app asks first when the user changes their own choice). Past declines stay declined.

**Telling the user.** A sync that finds new items sends one notification per group ("ana changed Friends's CLAUDE.md: 1 addition, 1 removal to review"). The tray tooltip and the nav show the number waiting, the Groups list tags the group, and the group page lists each item with its author, time, commit message and lines, with Accept and Decline (and Accept all / Decline all).

**Other groups.** Review state is per group. Accepting, declining or deleting a line in one group never touches another group's copy, even when the same line is in both. Settling a clash between groups (§7b) uses the same decline: the line left out is declined in that group's copy and saved in `choices` (`droppedLines`), so your other devices leave it out too.

**Scope.** Only `CLAUDE.md` is reviewed. Skills, agents, commands and output styles still follow §6 and §7 (the group's version applies; a local edit gets the Share / Undo notice). Settings merge per key (§8).

**Mods-canary trials.** A mods-canary trial sets `enabledPlugins["<mod>@claude-mods"]` to `false` and adds a path ending in `mods/<mod>` to `env.CLAUDE_CODE_PLUGIN_DIRS` (split on `:` or `;`). While both hold (`settings::canary_mods`), the app treats that one `enabledPlugins` value as a local-only override: it is not listed as a local edit, not pushed to the group (`settings::reconstruct` leaves it out), and not overwritten when the group's settings are applied. When the path is removed from the env, the normal rules apply again.

## 7b. Groups that must not conflict

Claude reads every group's `CLAUDE.md` and the user's own text together, and every group's settings are merged into one `settings.json`. Two groups, or a group and the user, can say the same thing twice or the opposite. The app finds these clashes and lets the user settle each one on this device.

**What counts as a clash.** Deterministic checks, no model involved:

- Each `CLAUDE.md` Claude reads here (each group's copy as accepted, §7a, and the user's own text outside the managed block) is cut into rule lines. Headings, code blocks, import lines, HTML comments, table rows and lines of fewer than two words are left out.
- **Duplicate:** two lines from different sources that are equal once case, list markers, punctuation and spacing are ignored, or equal apart from words that only set strength or direction ("always", "do", "must", "please"), with the same sense ("Never push to main" and "Do not push to main").
- **Contradiction:** two such lines that are equal apart from those words, where one is negated and the other is not ("Always use tabs" and "Never use tabs").
- **Setting:** two groups' `settings.shared.json` set the same plain value (JSON pointer) to different values. The group higher in the list wins on apply (§8), so its side is marked "applies now". Arrays are merged by union and never clash.

At most 200 clashes are listed. Each has an id that stays the same while both sides do.

**The Conflicts card.** On each group page: the clashes with a side in that group, both sides with the group name (or "Your CLAUDE.md") and the line or value. The card is hidden when there are none. Per item:

- **Keep A's / Keep B's** for a line: the other line is taken out of what Claude reads on all the person's devices. In a group's copy it is declined like a change under review (§7a), so "Use the group's version" brings it back; it never changes the group for anyone else. In the user's own `CLAUDE.md` the line is removed after the file is backed up. For a duplicate, this is "keep one".
- **Edit** a line of the group on screen: the new text (redacted and scanned, §9) replaces the line in the group's `CLAUDE.md` and is committed as `edit CLAUDE.md`; the group's other members get it to review (§7a). Empty text removes the line.
- A setting clash has no button: the earlier group's value is used and the screen says so. To use the other group's value, move that group up in the list. There are no per-device choices; a value lives in the group, in your personal settings or in a project.

**Warnings before sharing.** Sharing the user's `CLAUDE.md` with a group and copying lines to another group (§4) run the same check on the text in the preview against the target group and the user's other groups, and list the clashes under it. The source of the text (the user's own file, or the group copied from) is left out of the check.

**Not built: "Check with Claude".** A semantic check (rules that clash in meaning but not in words) could run `claude -p` the way scheduled jobs do (§17). It is not implemented; the checks above compare words only.

## 8. Settings merge

`settings.shared.json` is merged into `~/.claude/settings.json`, with ownership so removals work (code: `crates/engine/src/settings.rs`):

- Objects merge key by key. Arrays get the shared items they lack, appended after local items. For plain values, the shared value wins.
- `~/.claude-sync/settings-owned.json` records, per group, every value the app set (JSON pointer and value) and every array item it added.
- A key or array item the group no longer has is removed locally **only if** the local value still equals what the app set. If the user changed it, it stays, and the app offers to share the user's value with the group.
- Two groups that set the same plain value differently: the group higher in the list wins (§10) and the clash is listed on both group pages (§7b), where the user can pick the other group's value instead.
- If either file is not one JSON object, nothing is merged and the app says so. If the result equals the current file, nothing is written.
- Never shared, even if present in `settings.shared.json`: `apiKeyHelper`, `env` values that the secret scan flags, and `hooks` whose commands contain absolute paths (reported, not applied).

Plugins and marketplaces:

- A marketplace bundled under `plugins/<marketplace>/` (§4) is registered by the app, not by `settings.shared.json`: apply sets `extraKnownMarketplaces.<marketplace>` in `~/.claude/settings.json` to `{"source": {"source": "directory", "path": "<absolute path of ~/.claude-sync/marketplaces/<marketplace>>"}}`. The path is this machine's, so the value never goes through `settings.shared.json` and is never offered for sharing.
- That value is the app's. It replaces a different local value for the same name (for example a `directory` source pointing at a local checkout); `settings.json` is backed up first and the report says so. It also wins over the same key in any group's `settings.shared.json`, which is reported and not applied. It is removed when the marketplace leaves the group or the group is removed, but only while it still holds the value the app set.
- `enabledPlugins` (and any marketplace not bundled in the repo) stay in `settings.shared.json` and merge as above. A marketplace that is not bundled needs a source that works on every machine, such as `github`; a `directory` source there is a path on one machine.

## 9. Secret scan

Patterns in `crates/engine/src/secrets.rs`: `github_pat_…`, `sk-ant-…`, `ghp_…`, `gh[orsu]_…` (OAuth, refresh, server and user tokens), `AKIA…`, `xox[abpr]-…`, private key headers, `"access_token"`/`"refresh_token"` fields. A line containing `claude-sync: allow-secret` is skipped. The same redaction patterns clean logs before they are written and before feedback is sent.

## 10. Local state

```
~/.claude-sync/
  claude-sync.db         the store: groups and their order, bases, review items, jobs, notifications,
                         conflicts, request counts, feedback queue, settings rows (§25)
  groups/<owner>__<repo>/CLAUDE.md   this device's accepted copy of the group's CLAUDE.md (§7, §7a)
  marketplaces/<name>/   plugin marketplaces bundled in a group repo (§4, §7), each with a COMMIT file
  machines/<id>.json     this device's SSH record (§21)
  jobs/<id>/             run lock and last-run.json of a job (§17)
  conflicts/  backups/   saved local versions and files backed up before a change
  logs/                  claude-sync.<UTC date>.log (redacted, 7 kept); logs/jobs/<id>/<UTC stamp>.log is job output
  sync.lock              one sync at a time, app and CLI (§6)
  daemon.lock, daemon.owner   one daemon per machine (§26)
  token                  the CLI's sign-in file, mode 0600 (headless only, §3)
```

The v1 files (`state.json`, `manifest.json`, `groups/<id>/base/`, ...) are never deleted by v2 (§26).

**Group order.** `groups.order_idx` is the priority list: new groups are added at the end, and when two groups set the same file, settings value or marketplace, the one higher in the list wins (§7, §8). The user changes the order with the up and down buttons on the Groups screen (`Command::ReorderGroups`). It refuses a list that is not exactly the joined groups, saves the order, then syncs every group again in the new order, so the new winner takes over the files, the settings values it was losing and the marketplace, and the old winner is reported as a collision.

**Settings that are rows.** The `settings` table holds one JSON value per key: `choices` (§16), `app` (first-run autostart, desktop notifications on/off, update state), `packs` (§23), `plugins` (§18), `ssh`, `activity` and the v1 values kept whole under `v1/<path>` (§26).

v0 (the bash CLI) is no longer migrated: a `~/.claude-sync` that is a git clone is not touched, and the user installs 2.0.0 and joins their groups again.

## 11. App

- **Tauri 2**, Rust shell plus a React and TypeScript window (`app/ui`). Tray icon with: status, Sync now, Open, Notifications (N) while N are unread, Pause, Send feedback, Quit. Single instance. Starts at login (autostart plugin, on by default, can be turned off).
- Closing the window destroys the webview, so the idle app is just the Rust process (target: under 30 MB RAM idle, under 15 MB installer).
- Screens: Sign in (device code), Groups (list in priority order with up/down buttons, status, last sync; the note "Groups higher in the list win when two groups set the same thing." shows when there are two or more), Group (what the group gives this device, plugins that failed to install (§18), scheduled jobs (§17), members, invite by GitHub login (host only; members see a line saying only the host can change the group), pending invitations, recent changes as an activity feed parsed from commit messages in `app/ui/src/activity.ts`, a **Revert** button on each feed entry (host only; opens a confirm step listing the files and why a revert is blocked, §6), conflicts, leave), Notifications, Settings (device name, poll interval, start at login, pause, desktop notifications, personal memory §16), Feedback.
- **Notifications.** Everything the app tells the user goes through one function, `notify::show(app, kind, group, title, body)` (`app/src-tauri/src`, `mac_notify.rs` on macOS); new features call it rather than the notification plugin. It:
  1. records the notification in the store (`notifications` table, §25): id, UTC time, title, body, kind (`update`, `group`, `plugins`, `local-edit`, `job`, `personal`, `other`), the group it is about if any, and read/unread. Newest first, at most 200; read ones older than 30 days are dropped. A missing or corrupt file is an empty history (a corrupt one is kept as `notifications.json.bad`).
  2. shows the desktop notification, unless **Show desktop notifications** is off in Settings. The history is kept either way.
  3. emits `notifications-changed` (`{unread}`), which updates the bell's badge in the window and the tray's **Notifications (N)** item.

  The bell next to the app name opens the Notifications screen: newest first, relative time (full time on hover), kind, and the group's name, which links to the group. Opening the screen marks everything read; notifications that arrive while it is open are marked read once the window has focus. **Mark all as read** and **Clear** are there too. Clicking a desktop notification opens that screen (Windows: only while the toast is on screen, not from the Action Center; macOS: from the banner or Notification Center).

  Windows shows toasts under the app's AppUserModelID, which is the bundle identifier `com.mehdimamas.claudesync`; the NSIS installer's Start menu shortcut carries the same id, so the toasts are named and iconed as claude-sync. A dev build (`target/debug`) has no such shortcut, and the plugin falls back to PowerShell's id, so dev toasts show as "Windows PowerShell". On macOS the plugin posts through `NSUserNotificationCenter` as the signed bundle; dev builds post as Terminal.
- Logs: `tracing` to `~/.claude-sync/logs/claude-sync.log`, rotated daily, 7 kept, redacted (§9).

## 12. Relay (Vercel, free tier)

Plain TypeScript functions, no framework. Env:

| Name | What |
|---|---|
| `RELAY_GITHUB_TOKEN` | Fine-grained PAT on `RELEASES_REPO`, `FEEDBACK_REPO` if set, and `FEEDBACK_ATTACHMENTS_REPO`: Contents read/write, Issues read/write, Metadata read |
| `RELEASES_REPO` | `MehdiMamas/claude-sync` |
| `FEEDBACK_REPO` | Where feedback issues go; defaults to `RELEASES_REPO` |
| `FEEDBACK_ATTACHMENTS_REPO` | Private repo for feedback screenshots and logs; defaults to `<FEEDBACK_REPO>-feedback` |
| `BLOCKED_GITHUB_USERS` | Comma-separated logins refused with 403 (abuse) |
| `GITHUB_OAUTH_CLIENT_ID`, `GITHUB_OAUTH_CLIENT_SECRET`, `INSTALL_SESSION_SECRET` | The install page's GitHub sign-in (web flow of the same OAuth App the app uses; its callback URL is `<relay>/api/install/callback`) and the key that signs its session cookie. Unset: the page says sign-in is not configured |

**Auth.** Every call (except `/api/health`) carries the user's GitHub token: `Authorization: Bearer <token>`. The relay calls `GET /user` to get the login. Any GitHub account may use the relay unless its login is in `BLOCKED_GITHUB_USERS`. The login is cached in memory for 10 minutes. `/api/feedback` also uses the token to open the issue as the user. The user token is never logged or stored. Each login gets 120 calls per hour across all endpoints (`429` with `Retry-After` past that).

**`GET /api/update?target=<windows|darwin|linux>&arch=<x86_64|aarch64>&current_version=<semver>`.** Reads the latest non-draft release of `RELEASES_REPO`. Returns `204` if not newer, else the Tauri updater JSON (`version`, `notes`, `pub_date`, `url`, `signature`). `signature` is the contents of the matching `.sig` asset. `url` is `/api/download/<asset-id>` on the relay. The latest release and each `.sig` are cached in memory for 60 s per instance; after that a conditional GET (ETag) is sent, and a `304` reuses the cached body. If GitHub answers `403`/`429` on the relay token, the last good answer is served (and logged). The `200`/`204` answers carry `cache-control: public, s-maxage=60, stale-while-revalidate=300` with `vary: authorization`, so GitHub calls stay about flat as users grow; `requireCaller` still runs first on every request that reaches the relay.

**`GET /api/download/<asset-id>`.** Asks GitHub for the asset with `Accept: application/octet-stream` and `redirect: manual`, then returns that `302` to the short-lived signed URL. Bytes never pass through Vercel.

**`GET /install`.** A self-contained HTML page. Signed out: **Sign in with GitHub** (`/api/install/login` → GitHub OAuth web flow, scope `read:user`, random `state` in a 10-minute HttpOnly cookie; `/api/install/callback` checks it, exchanges the code, reads the login with `GET /user`, discards the token and sets a 1-day session cookie signed with HMAC-SHA256 and `INSTALL_SESSION_SECRET`). Signed in and not in `BLOCKED_GITHUB_USERS`: the version of the latest release, buttons for its Windows `-setup.exe` and macOS `.dmg` (served by `/api/download/<asset-id>`, which accepts the session cookie as well as a bearer token) and the macOS first-run steps (Done, then Privacy & Security, Open Anyway; fallback `xattr -dr com.apple.quarantine /Applications/claude-sync.app`). A blocked login is told it isn't on the list yet.

**`POST /api/feedback`.** JSON `{ title, description, kind: "bug"|"idea", appVersion, os, logs, screenshot? (base64 PNG or JPEG, ≤ 3 MB) }`.
1. Redact `logs` again (§9) and cut them to the last 200 KB.
2. Create the issue **with the user's token**, so GitHub shows the user as its author: the title, and a body with the description, kind, app version and OS. Nothing from the user's disk goes into it, and no labels yet.
3. Commit the screenshot, the logs and a `report.json` (issue number and URL, login, kind, app version, OS, time) to the private `FEEDBACK_ATTACHMENTS_REPO` at `issues/<number>/`, in one commit on its default branch (Git Data API, with the relay token: users have no push access). Best effort: on a failure the issue body says the attachments could not be stored. Storing after the create means a refused issue leaves no files behind.
4. GitHub drops labels and milestone from someone without push access, so the relay then sets labels `feedback` and `type:bug` or `type:idea` and the **nearest sprint** with its own token: the open milestone titled `Sprint N` with the earliest `due_on` that is today or later. If none, the open milestone with the latest due date. If none, no milestone. Best effort: a failure is logged, the post still succeeds. This comes last so the triage script finds the attachments in place.
5. Return `{ number, url }`.

The issue is only ever created with the user's token, never with the relay token, so a report always sits under the account that wrote it. If GitHub refuses the user's create (401, 403, 404, 410, 422: missing scope, blocked from the repo, SSO, issues off), the relay answers 403 (422 for a 422) with `GitHub did not let your account open an issue on <repo>: <reason>`, and the app shows that message. A rate limit (429, or a 403 with `Retry-After` or no requests left) answers 429, a 5xx 502 and a network error 500: the app keeps the report in its outbox and sends it later. A report the outbox later finds refused is set aside and the user gets a notification with the reason.

Rate limit per login: 5 feedback posts per hour (in memory; good enough for a hobby).

**The issue is public, the attachments are not.** `FEEDBACK_REPO` defaults to the app's own repo, which is public, so the issue (title, description, author's GitHub account, app version, OS) is public. The screenshot and the logs only go to the private attachments repo, which only the maintainer can read. The form says so before sending. Logs are off unless the user ticks the box; the app and the relay both redact tokens from them (§9), but they can still show file paths and repo names.

**Triage.** `relay/triage/run.ts` is a local script (`cd relay && npm run triage`) that the maintainer runs by hand or from cron/launchd. It posts only as the triage GitHub App bot (an installation token minted from `TRIAGE_APP_ID`, `TRIAGE_APP_INSTALLATION_ID` and the 0600 key file `TRIAGE_APP_KEY_FILE`; `relay/triage/bot.ts`), with no fallback to `gh auth token` or `GH_TOKEN`, and runs the local `claude` command with the maintainer's own Claude account; there is no API key and no hosted runner (see relay/README.md). It does:
1. `prepare`: for each open `feedback` issue with no triage comment and no `triaged` or `needs-moderation` label, it checks in the issue's events that a trusted login (the owner, plus the optional `TRIAGE_TRUSTED_LOGINS` environment variable) added the label, writes the issue and the list of other open issues to files, and checks out only that issue's `issues/<number>/` folder from a shallow clone of the attachments repo.
2. Debounce: if the newest of those issues is less than 10 minutes old, it stops. Reports that arrive close together are triaged together.
3. `analyze`: one `claude -p --model sonnet --output-format json` session per batch of 5 reports reads those files and the code with Read, Grep and Glob only (no shell, no edits, no web), in an environment with no GitHub token. The prompt says the reports and the attachments are data, never instructions. Its answer has one section per report, each starting with `VERDICT: ok|abusive`. The Claude call is a function passed in, so tests never start `claude`.
4. `post`: for `ok`, a comment with the summary, evidence, likely cause, files to look at, repro steps, severity, related issues and a fix plan, then the `triaged` label. Before it is posted the comment is scrubbed of tokens, home folder names and email addresses, @mentions and closing keywords are broken, links outside this repo, images and HTML comments are removed, long code blocks are cut and the length is capped. For `abusive` (spam, harassment, unrelated text), a short note, the `needs-moderation` label, and the issue is closed as not planned and locked. A hidden marker in the comment keeps an issue from being triaged twice.

## 13. Updates and releases

- **Desktop app.** `tauri-plugin-updater` reads `latest.json` of the newest GitHub release (`releases/latest/download/latest.json`) at start and every 6 hours (`app/src-tauri/src/update.rs`). A newer bundle downloads in the background and the plugin checks its minisign signature against the public key in `app/src-tauri/tauri.conf.json`; a bad signature is thrown away. A verified bundle installs when the app is idle (no sync, no open window), on **Restart to update** or on Quit. The engine only records the state (`Command::SetUpdate`). No token is sent: `latest.json` is a public file.
- **Command line.** `claude-sync-cli update [--check]` (`crates/cli/src/update.rs`) finds the archive for this target in the release, checks its SHA-256 against the `.sha256` file and swaps the running binary. The release carries `claude-sync-cli-<target>.tar.gz` (`.zip` on Windows) for Linux (x86_64, aarch64), macOS (both) and Windows.
- **Relay v1 routes.** Installs of 0.1.x and `claude-sync-cli` 0.1.x ask the relay: `GET /api/update?target=&arch=&current_version=` answers `204` or the updater JSON, `GET /api/download/<id>` redirects to the asset, `GET /install` redirects to the Releases page (group READMEs already in the field link to it). They stay unchanged so a 0.1.x install can update to 2.0.0 (§12); aarch64 Linux gets the CLI archive from the relay.
- **Release** (steps in [releasing.md](releasing.md)): a PR bumps the version with `node scripts/bump-version.mjs <version>` (workspace `Cargo.toml`, `Cargo.lock`, `app/package.json`, `app/ui/package.json`; `tauri.conf.json` takes the Cargo version) and `ci.yml` checks it. After the merge the maintainer pushes the tag `v<version>`. `.github/workflows/release.yml` checks that the tag equals the version in those files, builds on GitHub-hosted runners in the environment `release` (required reviewers; it holds the signing secrets), signs the update bundles, builds `latest.json` with `scripts/make-latest-json.mjs`, checks the installer size (`scripts/check-installer-size.mjs`) and the signature (`scripts/verify-updater-sig.mjs`), and creates or updates a **pre-release**. Nothing reaches users until the maintainer marks it latest.
- `scripts/release.ps1` builds, stages and verifies the Windows installer locally (`-DryRun` for a test build, `-Publish` to upload it to the draft). It calls `scripts/bump-version.mjs` for the version files.
- A release build needs `CLAUDE_SYNC_RELAY_URL` and `CLAUDE_SYNC_GITHUB_CLIENT_ID` at compile time. The updater key has an empty password (`tauri build --ci`); locally it comes from `TAURI_SIGNING_PRIVATE_KEY` or `~/.claude-sync-keys/updater.key`.
- The relay's update path is tested without GitHub or Vercel: `relay/dev/harness.ts` runs its handlers against a stub GitHub (`relay/test/e2e.test.ts`).
- Code signing: `release.yml` signs the macOS app with Developer ID and notarizes it when the `APPLE_*` secrets are set; without them macOS needs right-click, Open the first time. Windows is not Authenticode-signed: SmartScreen shows once.
- Linux (beta): `.deb` and `.AppImage` on `ubuntu-22.04`. The keychain uses the Secret Service backend (`keyring` with `sync-secret-service`, the `secret-service` feature of `crates/github`), so a session needs GNOME Keyring or KWallet. The headless CLI builds without that feature (`default-features = false`) and keeps its token in a file.

## 14. First install for a friend

1. The host adds the friend's login in the app. The friend gets GitHub's invitation email.
2. The group repo's README (generated) links to the relay's install page (`<relay>/install`, §12): the friend signs in with GitHub there and downloads the Windows or macOS build.
3. The friend installs, signs in with GitHub, sees the pending invitation, clicks **Accept**. The app does the rest.

## 15. Testing

`scripts/check.ps1` (Windows) and `scripts/check.sh` (macOS, Linux) run every check locally; `ci.yml` runs the same on GitHub-hosted Ubuntu, Windows and macOS for every PR and push to `main`:

- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` (`unwrap`, `expect`, `panic` and `todo` are denied outside tests in every crate) and `cargo test --workspace`. The engine is tested with fakes and proptests (`crates/engine/tests`); `crates/github` against wiremock (`tests/`, including the idle budget); `crates/fs` against temp homes (`tests/`, including `no_console.rs`, the list of child processes that must not open a console window on Windows, #178); `crates/actions/tests/actions.rs` drives the actor with a fake GitHub; `crates/store/tests/import.rs` imports a fixture v1 state folder that holds every v1 file; `crates/cli/tests` run the command tree and byte-compare the service files and `run-job` argv against v1's.
- `relay`: `node --test` with `fetch` mocked, and `tsc`.
- `app/ui`: `vitest` (`pnpm -C app/ui test`), the build with its JavaScript size budget (`size`), and the Playwright scenarios (`pnpm -C app/ui e2e`): every screen of the blueprint against a fake backend (`?scenario=...`, `src/fake.ts`) and a pixel diff of the frames.
- `scripts/test/make-latest-json.test.mjs`: the `latest.json` builder.
- Proof in every PR: the commands run, their exit codes and test counts, and the OS that ran them.

## 16. Personal memory

**Two scopes.** A **group** shares how Claude works for everyone in it: settings, skills, mods, shared instructions (§4). **Personal memory** is one person's own: the text they wrote in `~/.claude/CLAUDE.md` and the memories Claude Code saved for them ("remember that I like apples"). Other members never want it, but the same person's other computers do. Personal memory goes only to that person's devices, the ones signed in to the same GitHub account. It never goes through a group repo, and nothing from a group goes into it.

**Repo.** A private repo `<login>/claude-sync-personal`, created by the app the first time the user turns personal memory on. It has no `claude-sync-group` topic, so it is never listed as a group. The app refuses to use it if it is public or if its `claude-sync-personal.json` has a format the app does not know.

```
claude-sync-personal.json   {"format": 1, "newDevicesJoin": true}
devices.json                {"devices": [{"id", "name", "os", "lastSeenUnix"}]}
README.md                   what this is (generated)
CLAUDE.md                   the user's own text of ~/.claude/CLAUDE.md
memory/<key>/**             ~/.claude/projects/<folder>/memory/**
state/**                    ~/.claude/personal-state/**
settings.json                ~/.claude/settings.json (a view, see below)
```

**What syncs.** Three kinds, each on its own switch, all on by default:

- `CLAUDE.md`: everything outside the managed block (§7), without stray group import lines. On the way back in, this device's block and imports stay where they are; the synced text goes under them, in the file's line ends. A file with only the block in it syncs nothing.
- Memory: `~/.claude/projects/<folder>/memory/**`. Claude Code names `<folder>` after the project path, with every character that is not an ASCII letter or digit turned into `-`. That cannot be decoded, so the app never decodes it. A folder under the home folder gets the key `home<rest>` (`C--Users-ana-Projects-app` and `-Users-ana-Projects-app` are both `home-Projects-app`); another device maps the key to its own `<encoded home><rest>`, and reuses an existing folder whose name differs only in case (Windows has both `C--Users-…` and `c--Users-…`). Folders outside the home folder (`/tmp`, `/private/var/folders`), temp and app-data folders, background job folders and worktree folders are per machine and never sync.
- Mod settings: `~/.claude/personal-state/**` as `state/**`. Small state that mods save for the user, such as model-router's opus-only mode. Dotfiles and unsafe names are skipped; a linked `personal-state` folder is not synced.
- When `~/.claude/personal-state` is a git clone (has `.git`), only `state/claude-sync/**` syncs; the rest is that clone's own business, never written, pushed or listed as a conflict, and copies already in the repo stay.
- Settings: `~/.claude/settings.json` as `settings.json`, on by default. What is shared is a view of the file: every value a group sets or owns (`settings-owned.json`, `settings.shared.json`) is left out, because the group wins on those, then values that look like secrets, then what cannot work on another machine (`settings::machine_specific`): `apiKeyHelper`, `permissions.additionalDirectories`, canary overrides, and any string that is or contains an absolute path (hooks, `statusLine`, path lists in `env`, app-managed marketplace entries). The view is written with sorted keys. It merges per JSON pointer: one side changed, that side; both changed the same value, the version already pushed wins and this device's real file is backed up first; a deleted value is removed here only if it still holds what was last synced. Applying changes only the pointers that differ and leaves the rest of the real file, then the group merge runs as usual.

Never synced, and reported in Settings as "stays on this device": a file over 256 KiB (or past 16 MiB in all), a symlink, a file the secret scan (§9) flags, and a path that is not a safe file name. These are never written over on this device either.

**Merge.** The same three-way merge as a group (§6), with its own base in `<state>/personal/base/`. `MEMORY.md` is an index of one line per memory, and two devices adding memories at the same time is normal, so it merges as a set of lines: a line one side removed is dropped, every other line of either side is kept, and the remote's new lines go after the line that precedes them on the remote. It never conflicts. An index that differs from the remote only by line endings (`\r\n` vs `\n`) is not pushed and not rewritten locally; pushed index bytes use `\n`. Under `state/`, a `*.json` file that both sides have, each a JSON object with a top-level ISO-8601 `ts`, is not merged: the newer `ts` wins (equal keeps the remote), with no conflict; without a valid `ts` on both sides it takes the normal merge. A `*.jsonl` log is the union of both sides' lines: empty lines and exact duplicates dropped, stably sorted by each line's `ts` (lines without one first), ending in a newline. It never conflicts. Any other file both devices changed is a conflict: the other device's version wins and this device's is saved to `<state>/conflicts/personal/<UTC time>/`. Before a sync changes or deletes `CLAUDE.md`, a memory file or a `state/` file on this device, the app saves the old copy to `<state>/backups/personal/<UTC time>/` (under its repo path, such as `memory/<key>/<file>`).

**When it runs.** After each group sync: a conditional `GET` of the head (`304` costs nothing), and a pass only when the remote moved, the local files changed, this device was renamed, or a day passed (to refresh `lastSeenUnix`). Sync now always runs a pass.

**Turning it on and off.** Settings → Personal memory has the master switch "Sync my personal memory between my devices", the four kinds, the device list (this device marked; rename it with the Device name field; remove an old one), "New devices of mine join automatically", the last sync and "Open repo". Turning it off stops it on all the person's devices (see **Choices** below); it leaves the files and the repo's memories alone. A kind turned off is remembered as it was; turned back on, it merges against that, so nothing is lost or re-added by mistake.

**Choices.** What a person chooses in the app is personal, not per device: turning a group off, the CLAUDE.md review choice for a group, "Run this group's jobs without asking", the jobs accepted (and turned off), the lines left out with "Keep one", which groups are joined and their order, pause, the check interval, start at login, and turning personal sync off. They live in the store's `settings` row `choices` (`crates/actions/src/choices.rs`) and follow the person through the personal repo as `state/claude-sync/choices.json` (the bytes v1 kept in `~/.claude/personal-state/claude-sync/choices.json`, which the import reads once, §26). The merge is `crates/engine/src/merge.rs`:

```
{"groups": {"owner/repo": {"off": true, "reviewClaudeMd": false, "autoRunJobs": true,
                           "acceptedJobs": {"job-id": "<hash>"}, "offJobs": ["job-id"],
                           "droppedLines": ["a line"], "member": true, "ts": 1760000000000}},
 "personal": {"off": true, "ts": 1760000000000},
 "order": {"list": ["owner/repo"], "ts": 1760000000000},
 "app": {"paused": false, "intervalSecs": 60, "startAtLogin": true, "ts": 1760000000000}}
```

`ts` is Unix milliseconds. The file merges per key (`merge_choices_bytes`), the newer `ts` wins, so two devices changing different groups lose nothing. After each personal sync a device carries out what it has not yet: a group `off` that is not in effect here is applied (the group row's `off` is the effect on this device, the choice is in `choices`), and `personal.off` newer than the moment personal sync was turned on here turns it off. Turning personal sync off writes that choice, runs a last sync to send it, and sets `newDevicesJoin` to false; there is no "only here". Without personal sync the `choices` row is the local store. The review choice and auto-run are read by the engine and the jobs reconcile straight from the file. `order` and `app` merge as a whole: the newer `ts` wins. After a personal sync a device also joins a group marked `member: true` that it lacks, leaves one marked `member: false` (the entry stays as the record of the leaving), puts its groups in the `order` list, leaves out each `droppedLines` line of a group's CLAUDE.md copy, and applies `app` (pause and interval in the `app` row, start at login in the OS; a `startAtLogin` never chosen is left to the OS). A job is accepted when `acceptedJobs` holds its current hash; a changed job has another hash and asks again. A failed join is logged and tried again at the next sync. Leaving a group clears its other choices. v1 kept these per device; the import (§26) copies them into `choices` once with `ts` 0 and they are never read again.

**A new device.** After sign-in, a device that never decided checks for `<login>/claude-sync-personal`. If the repo says `newDevicesJoin: true`, personal memory turns on by itself, the repo and the local files are merged (never overwriting either side's memories), and the app says: "Personal memory from your other devices is on. Turn off in Settings." If it says `false`, the app shows a notice offering to turn it on; "Not now" stops asking on that device. Another account on the same computer starts over with its own repo.

**Local state.**

```
store `settings` row `personal/state`: on/off, decided, kinds, device id, last sync, the devices seen
store `personal_base_files`           base snapshot (§25)
<state>/personal/off-claude-md/    a kind as it was when it was turned off
<state>/personal/off-memory/
<state>/personal/off-state/
<state>/personal/off-settings/
```

A v0 clone of the personal repo is not touched or imported; the user turns this feature on instead.

## 11a. Command line

The app binary is also the command line: `app/src-tauri/src/main.rs` hands the words to `claude-sync-cli`'s `run` (`crates/cli/src/run.rs`) before Tauri starts, so `claude-sync <command>` and `claude-sync-cli <command>` are one implementation over `claude-sync-actions`. Exit code: 0 done, 1 failed, 2 wrong usage; `--json` for machine output. After a change that the running app shows, the CLI sends `--refresh` to it (`crates/cli/src/refresh.rs`, a `ps` or `tasklist` check, never starting the app).

Commands: `login` (device flow), `logout`, `groups`, `join <owner/repo>`, `leave`, `sync`, `status`, `personal status|enable|disable`, `ssh status|enable|disable|alias <name>`, `activity status|enable|disable`, `packs [list|setup|check|on|off <id>] [--group owner/repo]`, `review list|accept|undo <id>`, `daemon`, `service install|remove|status`, `update [--check]`, `version`. `run-job` is hidden: the OS tasks of group jobs call it.

On macOS and Linux the release app links `~/.local/bin/claude-sync` to its binary at start (only if that folder exists and the name is free). On Windows the release exe is a GUI program, so PowerShell shows no output when called directly; pipe it (`& "$env:LOCALAPPDATA\claude-sync\claude-sync.exe" status --json | Out-String`, #149).

## 17. Scheduled jobs

A group can ask Claude Code to run a prompt on a schedule on each member's device, for example a twice-weekly refresh of everyone's setup. Code: `crates/engine/src/formats` (the `schedules.json` format), `crates/actions/src/jobs.rs` (consent, reconcile, the run itself), `crates/fs/src/tasks.rs` (the OS tasks), `crates/store/src/jobs.rs` (the `jobs` table), `crates/cli/src/run.rs` (`run-job`).

**Format.** Optional `schedules.json` at the repo root, versioned like the other repo files (repo only: it is read from the base snapshot and never placed in `~/.claude`).

```json
{"jobs":[{"id":"setup-scout","title":"Claude setup refresh","cron":"17 10 * * 1,4",
  "prompt":"/setup-scout:setup-scout unattended","cwd":"~","permissionMode":"bypassPermissions",
  "timeoutMinutes":60,"report":"~/.claude/setup-scout/reports/latest.md","args":["--chrome"]}]}
```

Strict validation; a bad job is skipped and listed on the group screen, never half-registered:

- `id` matches `^[a-z0-9][a-z0-9-]{0,40}$`. Ids are global on a device (they name the OS task): if two groups use one id, the first joined group keeps it.
- `cron` has 5 fields. Minute and hour are numbers or comma lists; day of month and month are `*`; day of week is `*` or 0-6 lists and ranges. Times are local. The screen shows it in words ("Mon and Thu at 10:17").
- `permissionMode` is one of `default`, `acceptEdits`, `plan`, `bypassPermissions`.
- `cwd` and `report` are `~` or `~/...` with no `..`.
- `timeoutMinutes` 1 to 240, default 60. `prompt` non-empty, at most 4 KiB. At most 20 jobs.
- `args` (optional) are extra `claude` options, passed after the prompt and before `--permission-mode`. Allowlist only: `--chrome` (Claude in Chrome tools, which `claude -p` lacks otherwise), `--model <id>` and `--effort <low|medium|high|xhigh|max>`, each at most once. Any other option is rejected, because flags such as `--dangerously-skip-permissions`, `--settings` or `--mcp-config` would get around the permission mode the member accepted. No shell is involved: arguments go to `claude` as they are.

**Consent.** Nothing is registered until the member accepts the job on the group screen, which shows the full prompt, the permission mode (`bypassPermissions` in warning style), the folder and the schedule. Everyone accepts, the host included. The acceptance, stored as a choice (§16), is a SHA-256 of `[cron, prompt, cwd, permissionMode, timeoutMinutes, args]`; any change revokes it and unregisters the task until the member accepts the new version. The runner checks the hash again before every run. A per-job toggle turns an accepted job off (unregisters it) and on.

**Without asking.** A member can turn on "Run this group's jobs without asking" for one group on one device: from a pending job ("Always allow jobs from <group>") or from the top of the group's Scheduled jobs section, which says plainly that new or changed jobs then run without review. It is off by default, stored as a choice (§16, `autoAccept`), per account, and forgotten when the member leaves the group. While it is on, every reconcile (after each sync, and when it is turned on) accepts the current version of each of that group's jobs itself, storing the same hash an Accept stores, and the app shows a notification per job ("New job from <group>" or "Job changed in <group>"). A job the member turned off stays off. Turning it off keeps what was accepted; the next change asks again. The runner is unchanged: it still checks the stored hash.

**Reconcile.** After every sync, at app start (checking that the tasks still exist) and after accept, toggle or leave: register accepted and enabled jobs, unregister the rest, drop the records of left groups. A group whose `schedules.json` can't be read keeps its tasks.

**Runner.** The task runs the app binary headless: `claude-sync run-job <owner/repo> <id>` (no tray, no single-instance check; "Run now" adds `--now`). It takes `jobs/<id>/lock` (a lock older than the timeout plus 5 minutes is stale), skips with `skipped: offline` when `HEAD https://api.github.com` fails, and runs `claude -p <prompt> [args] --permission-mode <mode>` in `cwd` with `CREATE_NO_WINDOW`, killing the process tree at the timeout. `claude` is resolved at registration (`where.exe` on Windows, an `.exe` before a `.cmd`; known install folders and the login shell elsewhere) and again if that path is gone. Output goes to `logs/jobs/<id>/<UTC stamp>.log` (20 kept), the outcome to `jobs/<id>/last-run.json`. If the run wrote the report file, a desktop notification says so; clicking it opens the report on Windows. The task does not get `CLAUDE_SYNC_HOME`, so it always uses the default `~/.claude-sync`.

**OS layer** (behind the `Scheduler` trait; tests use a fake and never register real tasks):

- Windows: `schtasks /Create /F /TN "\claude-sync\<id>" /XML <file>`. Weekly calendar triggers, start when available, run on battery, `IgnoreNew` for overlapping runs, execution limit = timeout + 10 minutes, interactive token of the current user, hidden. `schtasks /Delete /F` to remove.
- macOS: `~/Library/LaunchAgents/dev.claude-sync.job.<id>.plist` with `StartCalendarInterval`, then `launchctl bootout` / `bootstrap gui/<uid>`.
- Linux: a systemd user timer. `~/.config/systemd/user/claude-sync-job-<id>.service` (oneshot, `ExecStart` = the app's `run-job`) and `.timer` (one `OnCalendar=[Mon,Thu ]*-*-* HH:MM:00` per time, `Persistent=true` so a missed run happens at the next start), enabled with `systemctl --user enable --now`. Without `systemctl --user` the screen says so and Accept is disabled. Debug builds use the prefix `claude-sync-dev-job-`. Beta: only the unit text is unit-tested; registering needs a runtime check on a Linux desktop.
- Debug builds use `\claude-sync-dev\` and `dev.claude-sync.dev-job.`, so a dev run never touches the installed app's tasks. The ignored integration test uses `\claude-sync-test-<random hex>\`.

**Uninstall.** The NSIS pre-uninstall hook (`app/src-tauri/windows/hooks.nsh`) runs `claude-sync.exe remove-jobs`, which deletes the tasks and keeps the acceptances, so a reinstall registers them again. Updates run no uninstaller, and the hook skips `/UPDATE` anyway. On macOS, deleting the app leaves the LaunchAgents; their runs fail until removed.

## 18. Group plugin install

Placing a group's marketplace (§7) and its `enabledPlugins` (§8) is not enough: Claude Code loads only plugins installed in its plugin cache. So after a sync whose apply changed a group's bundled marketplaces (fingerprint) or added a plugin the group turns on (`true` in the group's `settings.shared.json` and in the local `settings.json`), the app runs in the background, through the same `claude` resolver as §17:

1. `claude plugin marketplace update <mp>` once per changed marketplace;
2. `claude plugin install <name>@<mp> --scope user` for each group-enabled plugin that `claude plugin list --json` does not show;
3. `claude plugin update <name>@<mp>` when the plugin's marketplace entry or declared version changed.

Each command has a 2-minute timeout and no console window; its output goes to the app log, redacted (§9). Nothing is uninstalled, and `enabledPlugins` handling stays as in §8. `-y` is never passed: a plugin that asks for confirmation is reported as needing it. Failures become warnings on the group screen ("couldn't install X: reason") with a Retry, and never fail the sync. Without `claude` the screen says "Claude Code not found" and the next sync tries again. State is in `plugins.json`; the CLI sits behind the `PluginCli` trait, and tests use a fake.

## 19. Claude helper (optional)

Explains a collision (§7), a pending CLAUDE.md change (§7a) and incoming group activity with the user's own Claude Code login: the app runs the local `claude` command. There is no API key and nothing is stored.

- **Availability:** the buttons show only when `claude` is found (same lookup as §17/§18; on Windows `claude.exe` or `claude.cmd`). Settings → "Claude helper" says "Uses your Claude Code login on this computer." and whether Claude Code was found. Without it, a click gives: "Claude Code isn't installed or signed in. Install it and run `claude` once to sign in."
- **On demand only:** "Explain" (collisions, CLAUDE.md review) and "Summarise" (activity feed) run `claude -p --model sonnet --output-format json --max-turns 1` with all tools disallowed, from the temp folder (so no project files are read), the prompt on stdin and the `result` field as the answer, with a 2-minute timeout. Nothing runs automatically. It counts against the user's own Claude plan.
- **Before sending:** the §9 secret scan runs on the text; a hit refuses with a message that names the pattern, not the text. Input over 40 KB is cut with a note.
- **Old key:** earlier versions kept a pasted API key in the keychain (account `@anthropic-key`). On every start the app deletes that entry if present.
- **Code:** `crates/actions/src/extras.rs` (the helper switch and the explain task; tests never start `claude`), shown in `app/ui/src/screens/` where a change is explained.

## 20. Creating the GitHub App (owner checklist, issue #39)

Create it at GitHub, Settings, Developer settings, GitHub Apps, New GitHub App:

- Name: `claude-sync-app-github` (App ID 5412185; slug `claude-sync-app-github`, built in as the default of `CLAUDE_SYNC_GITHUB_APP_SLUG`). Homepage URL: the repo URL.
- Callback URL: leave empty is not allowed for some forms; use `https://github.com/MehdiMamas/claude-sync`. Request user authorization (OAuth) during installation: off. Webhook: **Active off**.
- **Enable Device Flow: on.** Expire user authorization tokens: **on** (the default; do not opt out).
- Repository permissions: Administration **write**, Contents **write**, Pull requests **write**, Metadata read (automatic). Workflows: none.
- Account permissions: **Repository creation: write** (needed for `POST /user/repos`). No email or other account permissions.
- Where can this App be installed: **Any account** (friends install it on their own accounts).
- The Client ID (public, not a secret) is built in as the default of `CLAUDE_SYNC_GITHUB_APP_CLIENT_ID`; set the variable only to point a build at another App. No client secret or private key exists (the app uses neither).
- Each user installs it with "All repositories" on their account, and the host installs it on any org that owns a group repo.

**Settings UI.** Settings has a "GitHub App (beta)" card: a toggle for the `githubApp` setting (off by default, takes effect at next sign-in; disabled with "This build has no GitHub App" when `app_info.appClientIdMissing`). When on, an "Install the App" button calls `open_app_install` and `app_coverage` fills a list of group repos the App does not reach ("Not covered: owner/repo"). Code: `app/ui/src/screens/settings.ts`, `coverageLines` in `app/ui/src/copy.ts`.

## 21. Reaching your own machines over SSH (issue #166)

Opt-in and personal: `sshPeers` `{on, ts}` in the choices file (§16), merged by its own `ts`, so it is on or off for every device of the account. `claude-sync-cli ssh enable|disable|status|alias <name>` (and the same in the app binary, §11a) only set the choice and this machine's alias; the sync pass does the work.

- **Records.** Each device with the choice on writes `<state_dir>/machines/<machine-id>.json` (`id, name, alias, os, user, host, pubkey, ts`) and personal sync carries it as `machines/<id>.json` in the personal repo (`personal::is_personal_path` and `Kinds::syncs` accept it whatever kinds are chosen). It is rewritten only when a field other than `ts` changes. Without personal sync the records do not travel. `pubkey` is `~/.ssh/id_ed25519.pub`; `ssh-keygen` runs only when no key exists. `host` is `tailscale ip -4` or the host name. `alias` is `mac` on macOS, else `pc`, or the chosen one. On every pass a name counts as taken only by a record with a lower `id` (string order; `ts` is not used because every rewrite changes it), so the higher id moves to `-2`, `-3` and the lower id keeps its name without coordination.
- **Apply** (`ssh_peers::apply`, after personal sync in the app loop and in the daemon, under the `ssh-peers` caller). For each other record whose alias, user, host and key pass the checks (only `ssh-ed25519` and `ecdsa-sha2-*` keys on one line, no options): a line in the managed block `# >>> claude-sync peers >>>` of `~/.ssh/authorized_keys` with `from="100.64.0.0/10,fd7a:115c:a1e0::/48",no-agent-forwarding,no-X11-forwarding` (skipped on Windows), and a `Host` entry in `~/.ssh/claude-sync.conf`. `~/.ssh/config` gets `Include claude-sync.conf` as its first line (top-level scope); `ssh_peers::host_names` gives each peer its `Host` name: the alias, or `<alias>-cs` when the user's own config already defines it, and while that name is a user Host, an earlier peer's name or an earlier `<name>-lan`, `-<first 4 chars of id>` and then `-2`, `-3` are appended; `status` shows these names. Lines outside the block are never touched; files are 0600, `~/.ssh` 0700.
- **LAN fallback.** A record may carry `lan`, the device's private IPv4 address (read from a UDP socket that sends nothing; none on Windows or inside WSL, and only 10/8, 172.16/12 or 192.168/16, checked on publish and again on read). A writer with a LAN address also adds its private block to `from=` (`key_line`'s `lan_block`). On a non-Windows writer with `nc` on PATH, a peer with `lan` gets `HostKeyAlias <tailscale ip>` and a `ProxyCommand` that tries the Tailscale address (2 s) and then the LAN address, plus a `Host <name>-lan` entry (skipped when a user or other peer Host already has that name). Without `lan`, `nc` or on Windows the entry is unchanged.
- **Off** removes the block, the Include line, the conf file and this device's record.
- **Trust.** Records come only from the owner's private personal repo, which can already deliver `settings.json` hooks to every device. No sudo or admin rights.
- **Alternatives.** Tailscale SSH (server side only on Linux and open-source macOS builds); group scheduled jobs (§17: wrong audience, per-device acceptance); a claude-mods hook (runs only inside a Claude session).

## Headless CLI and the shared loop

`crates/cli` runs the same actor as the app without Tauri and without WebKit. It depends on `claude-sync-github` with `default-features = false` (no Linux keyring, no libdbus); `cargo test -p claude-sync-cli` builds exactly that.

- **Lock.** Every sync takes `<state>/sync.lock` (§6). The daemon also holds `<state>/daemon.lock` and writes `daemon.owner` (§26), so a cron check or a second start exits at once.
- **Token.** `{login, tokens}` in `<state>/token`, mode 0600 (written through a temp file and renamed), the same JSON as v1. A refresh saves the new tokens; an expired refresh token deletes the file. The daemon reloads the file each pass, so a `login` or `logout` in another terminal takes effect without a restart.
- **Loop.** `daemon` runs the actor (§24) with the same schedule as the app.
- **Service.** `service install` writes `~/.config/systemd/user/claude-sync.service` (`Restart=on-failure`) when systemd runs, otherwise crontab lines `@reboot` and `*/5 * * * *`. The text is byte-for-byte v1's (`crates/cli/tests/service.rs`).
- **Adopting an existing setup.** `join` backs up `CLAUDE.md` to `~/.claude-sync/backup/<time>/` and strips group text pasted outside the managed block before the first sync delivers it (`crates/cli/src/adopt.rs`).
- **Install.** `scripts/install-cli.sh` installs the release archive after checking its `.sha256`.

## 22. Sharing whether you are at the computer (issue #195, step 1)

Opt-in and personal: `activity` `{share, ts}` in the choices file (§16), off by default, merged by its own `ts`. Step 1 is local only: nothing is sent between machines (that is step 2).

- **Reader** (the shell reads it and sends `Command::ActivityTick { idle_secs }`; `crates/actions/src/extras.rs`): seconds since the last keyboard or mouse input, by direct OS calls. Windows: `GetLastInputInfo` and `GetTickCount` (wrapping subtraction). macOS: IOKit `IOHIDSystem` property `HIDIdleTime` (nanoseconds). Other systems: `None`, written as `unknown`, never as idle.
- **State machine** (`activity::Tracker`, pure, clock and reading passed in): `active` below 120 s idle, `idle` from 120 s, `unknown` with no reading. It asks for a write on a change of state or after 300 s without one.
- **File** `<state>/activity.json`: `{schema: 1, state, idleSeconds, updatedAt}`. Removed when sharing is off.
- **Loop** (`activity::run_loop`): one pass every 30 s in the app (`background::start`) and in the daemon. A pass reads the choice first; off means the file is removed and no OS call is made. Idle cost: one OS call per 30 s, 0 to 12 small local writes per hour, no network.
- **Settings:** the row "Share activity and presence" (`app/ui/src/screens/settings.ts`); the consent copy and the decision logic are in `app/ui/src/activity-consent.ts`. Commands `get_activity` and `set_activity`.
- **Command line:** `claude-sync activity status|enable|disable` (app binary) and `claude-sync-cli activity status|enable|disable`.

## 23. Opt-in packs

A group can offer **packs**: extras that need something on one computer (an app, a sign-in, a permission) and so turn on there only after the member set them up and a check passed. Members get the group's core; each pack is one "Set up with Claude" away. A group without a `packs/` folder behaves exactly as before: every part below does nothing.

- **A pack** is a folder `packs/<id>/` in the group repo: `pack.json`, `setup.md` (what Claude is asked to do, at most 8 KiB), an optional `CLAUDE.md` (the rule lines, imported only where the pack is on) and the check's script. Like `schedules.json` it is repo only (`group::is_repo_only`): read from the base snapshot, copied to `<state>/groups/<owner>__<repo>/packs/<id>/`, never into `~/.claude`. The catalog of the last sync is `<state>/groups/<owner>__<repo>/packs.index.json`.
- **`pack.json`** (`crates/engine/src/formats`): `schema` (1), `id` (equals the folder; `[a-z0-9][a-z0-9-]{0,30}`), `title` (at most 40), `summary` (at most 200), `needs` (`[{name, url?}]`, urls `https://`), `os` (`windows`, `macos`, `linux`; WSL is `linux`; default all), `plugins` (`name@marketplace`), `jobs` (ids in `schedules.json`), `files` (`skills/<name>` or `agents/<name>.md`), `claudeMd` (`"CLAUDE.md"`), `setup` (`"setup.md"`), `check` (`{run: [argv] (1 to 8), timeoutSeconds: 1..120, default 60}`), `hostCarryOver` (default true), `order` (display order, then id). At most 20 packs. A bad pack is skipped and listed on the group screen with the reason, never half-applied; a job, plugin or file may belong to one pack only. A plugin of a pack should be `false` in `settings.shared.json`; one the group still turns on for everyone is listed (`groupEnabled`) and stays on, so the move can be made in two steps.
- **Per-machine state** the store `settings` row `packs` (`crates/actions/src/packs.rs`, never synced; `choices` is per account): per group and pack a record `{state, by, since, packHash, lastCheck, jobHashes}`. No record is "not set up". States: `tried` (set up started or the check failed), `ready` (the check passed, waiting for Turn on), `on`, `failing` (on, but the check fails after a change), `off`. `on` and `failing` are applied. `by` is `setup`, `carry-over` or `host-auto`.
- **Check** (`packs::check`): `check.run` runs with no shell in the pack folder with the timeout; exit 0 passes; only the last stderr line is kept (at most 200 characters, through `secrets::redact`). A pack that is on changes only to `failing`: nothing that worked stops (D9). Checks run: after the setup terminal closes, on "Check again", before Turn on again, once per app start for packs that were tried, and once after an upstream change for packs that are on (`ops::recheck_due`). Never for packs that are not set up or are off.
- **Turn on** (`ops::turn_on`) needs a passed check against the pack as it is now (its hash), and the consent hash of every job of the pack as the member saw it. It sets the plugins' value in this machine's `settings.json` to `true` (backup first), records `on`, and the next apply (`Engine::reapply_group`) places the pack's `files` and adds one import line per pack to the managed `CLAUDE.md` block; the job reconcile then accepts and registers the jobs. **Turn off** reverses it: the plugin gets the group's value back, the line and unchanged files go, jobs unregister.
- **The plugin override** is a local-only value, handled like a mods-canary (§8): `packs::pinned_pointers` joins `canary_pointers` in `settings::keep_overrides` (apply), `edited_pointers_with` (the watcher: no local edit, no Share), `reconstruct_with` (never pushed) and `personal::settings_file::skip_set` (never carried to other devices).
- **Jobs** of a pack register only where the pack is on (`packs::job_gate` in `Jobs::reconcile`); the group screen lists them under the pack until then. Jobs in no pack are unchanged.
- **Host carry-over** (D6): when the signed-in login owns the repo, the sync reads the catalog and `settings.json` *before* the apply merges the group's values (`packs::prepare`, called by `Engine::pull_push`), and gives every pack without a record a state: `on` when a plugin is `true` here, a job is accepted and on, or the pack has neither (its rule lines and files are in use); otherwise `off`, which stays off (reflect). No check runs. A machine that is offline across the switch does the same on its first sync, because the read comes first in the same pass.
- **Setup** (`packs::launch`): "Set up with Claude" writes a small script into `<state>/packs-setup/` and opens a terminal on it: Windows Terminal or `cmd` on Windows, Terminal on macOS, the first terminal found on Linux. The script runs interactive `claude` in the pack folder with a short first message (`Read setup.md in this folder and help me set up ...`), normal permission mode, then writes a `.done` marker; the group screen sees it and runs the check. If Claude Code or a terminal is missing, the command to run by hand is shown.
- **Commands.** The window sends the pack `Command`s of `crates/actions/src/model.rs` (`crates/actions/src/packs.rs`, launch in `launch.rs`); command line (app binary and `claude-sync-cli`): `packs [list | setup <id> | check <id> | on <id> | off <id>] [--group owner/repo] [--json]`; on a machine with no window `setup` runs `claude` in the same terminal, then the check, then asks `[y/N]`. Exit 0 done, 1 failed, 2 wrong usage. Both binaries use `crates/cli/src/run.rs`.
- **Never nags** (D8): claude-sync says nothing about packs by itself: no notification, no badge.

## 24. Actor model

`crates/actions/src/actor.rs`. One tokio task owns `Core` (the engine inputs, the store, the GitHub client and the filesystem handles). Nothing else mutates state.

- **Commands in, events out.** The window, the tray and the CLI call `Handle::send(Command)` and get one `Event` back (`model.rs`). Commands run one at a time on a blocking thread, because the engine, the store and the GitHub client are synchronous.
- **Snapshots.** After every change the actor publishes a read-only `Snapshot` on a `watch` channel. The window and the tray render snapshots only (`Handle::snapshot`, `Handle::watch`); they never touch state.
- **The loop.** The actor wakes every second and asks `schedule::due` (pure, tested with a fake clock) what is due: the sync pass at the person's interval (default 60 s), pending invitations every 15 minutes (and when the window opens), the rename check once a day per group, a wait while rate limited, nothing while paused, signed out or expired. After each pass it reconciles jobs (§17) and group plugins (§18) and retries the feedback outbox.
- **Types for the window.** `Command`, `Event` and `Snapshot` derive TypeScript types with ts-rs (`--features ts`); `scripts/gen-types.sh` writes `app/ui/src/generated/`.
- **Panics.** The release profile keeps `panic = "unwind"`: the shell catches an engine panic instead of aborting the app.

## 25. Store schema and migrations

`crates/store/src/lib.rs`: SQLite (`rusqlite`, bundled) in `~/.claude-sync/claude-sync.db`, mode 0600 on Unix, WAL, foreign keys on. The engine sees it through the `Store` trait (key and bytes, the `settings` table); the actions use the typed tables.

| Table | What |
|---|---|
| `groups` | repo, role (`host` or `member`), state, `order_idx`, `off`, `renamed_from` |
| `bases` | per group: head sha, head ETag, last sync time |
| `base_files` | per group and path: blob sha, hash of the placed bytes, the base bytes (migration 2) |
| `personal_base_files` | the personal repo's base bytes (migration 2) |
| `review_items` | CLAUDE.md review (§7a): line, author, status `pending`, `accepted` or `declined` |
| `jobs` | per group and job id: spec hash, accepted hash, enabled, last result and run (§17) |
| `notifications` | history (§11) |
| `conflicts` | saved local versions (§6) |
| `requests` | request counts by caller, endpoint and result (§6) |
| `feedback_queue` | reports waiting to be sent (§12) |
| `settings` | key to JSON value (§10) |

**Migrations.** `MIGRATIONS` is a list of SQL scripts; `PRAGMA user_version` says how many ran. `open` runs the missing ones, each in a transaction that also bumps `user_version`. A database whose version is higher than the build knows is refused with "store.db is schema N, this build knows up to M; update claude-sync", never opened read-write. A new migration is a new list entry; an entry is never edited once released.

## 26. The v1 hand-over and import

2.0.0 keeps its data in `claude-sync.db`; the 0.1.x files are read once and never written, moved or deleted, so a downgrade starts from them as they were.

- **Import.** `store::import_v1` runs only into an empty store (no report, no import marker) and only while the caller holds `daemon.lock`. Every v1 file is classified (`classify`): read into the store, left in place (base trees, `packs-setup/`, logs, backups), or unknown (the fixture test fails on an unknown file). A file that cannot be read is skipped and named on the sign-in screen ("Could not read N of M old v1 files"); nothing is deleted. The choices come from `~/.claude/personal-state/claude-sync/choices.json`. Values not modelled as tables are kept whole as `settings` rows `v1/<path>`. The first v2 pass is never a first sync: the bases point at the v1 base trees.
- **Hand-over.** A 0.1.x daemon may hold `daemon.lock`. `crates/actions/src/handover.rs`: a v2 start that finds the lock held by a process that is not a live v2 daemon takes it for a v1 daemon. It waits for the v1 pass to end (`sync.lock`), stops the process and polls the lock, all within 30 s. A v1 daemon that does not stop leaves a note (its pid and the command that ends it), nothing is imported and nothing is written; the sign-in screen offers Try again. When the hand-over stopped a systemd service, the unit is restarted so the machine does not stay without a daemon. The process list and the kill go through the `Procs` trait, so tests never touch real processes.
- **`daemon.owner`.** Every v2 daemon writes `<state>/daemon.owner` (`pid`, OS start time, version, exe) after it takes `daemon.lock` and removes it on exit. A program that finds the lock held reads it to tell a live v2 daemon (pid and start time match a running process) from a stale file or a v1 daemon (no file).
- **Order.** The app opens `Core`, which hands over and imports under `daemon.lock`; a live v2 daemon that holds the lock does the import itself.
