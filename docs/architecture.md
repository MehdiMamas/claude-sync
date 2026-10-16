# claude-sync app: architecture

This document is the design the sprint issues point to. The app described here replaced the v0 bash CLI, which is gone; §10 and §16 say how a v0 setup is moved.

## 1. What it is

A small desktop app (Windows and macOS first; Linux ships as a beta, untested until a runtime check) that lives in the tray, starts at login and keeps Claude Code setups in sync:

- **Groups.** A group is a set of friends who share Claude Code settings: `settings.json` keys, skills, subagents, slash commands, output styles and `CLAUDE.md` instructions. One person **hosts** the group: the group repo lives on their GitHub account. Everyone else **joins**. Only the host changes the group: shared settings, skills, `CLAUDE.md` and the member list. A member's edits stay on their machine; they propose a change as a pull request to the group repo, and the host merges it (§5a).
- **Devices.** Each person can run the app on as many PCs as they like. Every device signs in with the person's own GitHub account and has full access.
- **Personal memory.** A person's own `CLAUDE.md` text and Claude's memories follow them to their other devices through a private repo of their own, never through a group (§16).
- **Live.** The app watches the remote for changes and applies them within about a minute. It watches the local files it manages, and when you edit one it asks whether to share the change with the group.
- **Updates.** The app checks for new versions and updates itself.
- **Feedback.** A feedback form sends a description, and optionally the app's recent logs and a screenshot, to this repo as a public issue opened under the user's GitHub account, put into the nearest sprint.

It costs nothing to run. GitHub (free private repos, OAuth device flow) holds all the data. A free Vercel project (the **relay**) does the few things that need a secret: serving updates from this repo's releases and filing feedback issues.

## 2. Components

```
crates/core/        Rust library: GitHub client, group format, sync engine, apply layer, settings merge, secret scan
app/                Tauri 2 app: tray, autostart, updater, UI (app/ui, plain TypeScript + Vite), feedback capture
relay/              Vercel project (TypeScript functions): /api/update, /api/download, /api/feedback, /api/health
scripts/            Local checks, the version bump and the local Windows release script
```

GitHub Actions run on GitHub-hosted runners only (free for a public repo): `ci.yml` runs the checks of `scripts/check.ps1` on Ubuntu, Windows and macOS for every PR and push to `main`, and `release.yml` builds releases (§13). No self-hosted runners, no AI jobs.

## 3. Identity and auth

- The app signs in with the **GitHub OAuth device flow** (no client secret in the app). It needs a GitHub OAuth App with device flow enabled. Its client ID is public and is compiled in (`CLAUDE_SYNC_GITHUB_CLIENT_ID`).
- Scope: `repo` (needed to read and write private repos and to manage collaborators and invitations) and `read:user`. This is broad. A GitHub App with per-repo access is in the backlog.
- The tokens are stored in the OS keychain (Windows Credential Manager, macOS Keychain, Linux Secret Service such as GNOME Keyring or KWallet; Linux is beta) through the `keyring` crate, under service `claude-sync`, account = GitHub login, as one entry: a JSON object with the access token and, when GitHub sends them, the refresh token (`ghr_…`) and both expiry times. An entry saved by 0.1.1 or earlier is the bare access token and still loads. Tokens are never written to a file or a log.
- **Token refresh.** When the OAuth App has expiring user tokens on, the access token lasts 8 hours and the refresh token 6 months. The app refreshes 5 minutes before the access token expires, and again after a `401`, with `POST https://github.com/login/oauth/access_token` (`grant_type=refresh_token`, `client_id`, `refresh_token`; no client secret, as in the device flow), then saves the new pair to the keychain and retries the call once. All GitHub calls of a sign-in share one session (`github::Session`), so concurrent calls wait for a single refresh instead of each spending the refresh token (GitHub rotates it on every use).
- **Expired sign-in.** A `401` that a refresh cannot cure (no refresh token, or GitHub refuses it, e.g. `bad_refresh_token`) and that `GET /user` confirms means GitHub no longer accepts the sign-in. The app then signs out as *expired*: background GitHub calls (sync loop, update check, feedback outbox) stop, the dead tokens are removed from the keychain, `state.json` and the groups stay, one notification says so, and the window shows "Your GitHub sign-in expired" with a **Sign in again** button instead of an error every minute. The tray says "Sign-in expired". Signing in again clears it and syncing resumes where it left off. A network error or a GitHub 5xx during a refresh is not an expiry: the call fails and the next pass tries again.
- Each device signs in on its own. Signing out on one device does not affect the others.

## 4. Group repo

A private repo on the host's account, created by the app. Default name `claude-sync-data` (Mehdi's group is `MehdiMamas/claude-sync-data`). It gets the repo topic `claude-sync-group`, which is how the app recognises group repos.

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
  "name": "Mehdi & friends",
  "members": ["MehdiMamas", "friend-login"],
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

**No double instructions.** Once the group has the text, the managed block imports it, so Claude would read it twice. "Remove it from my own CLAUDE.md (it now comes from the group)" is on by default. It removes the shared lines (blank lines aside, trailing spaces ignored, each shared line once) from the user's own text only; the managed block and import lines stay. It runs only when this device applies the group's `CLAUDE.md` (not kept local), and the file is backed up to `~/.claude-sync/backups/<UTC time>/` first. With personal memory on, the shorter text reaches the person's other devices too; they get the instructions back from the group once they join it.

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

- **The app.** Members are invited with read (pull) access, and only the host's app pushes to a group repo. A member's local edit to a managed file is never pushed: the notice offers **Keep local** and **Undo** but no **Share with group**, the group screen shows "Only <host> can change this group. Your edits stay on this machine.", and the core refuses a member's change with `Error::HostOnly` ("Only <host> can change <group>. Your change stays on this machine; ask <host>, or propose it as a pull request."). A member who wants a change opens a pull request on the group repo; the host reviews and merges it.
- **GitHub.** The host's app creates a repository ruleset on the default branch that blocks update, deletion and non-fast-forward pushes for everyone except repository admins. If GitHub refuses (for example a private repo on a free plan), the app still only pushes from the host's machine, and the host sees "GitHub can't lock this repo on your plan; the app still only sends changes from your machine." (`GroupStatus.hostLock` is `on`, `unavailable`, or null for members.)
- **Proposals.** A group can opt into proposal mode, which lets members send changes through the host's review (§5b).

### 5b. Proposals

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

### 5b. Transfer and rename

- **Transfer (host only).** The group screen has "Hand over the group": pick a current member and confirm. The app calls `POST /repos/{o}/{r}/transfer {new_owner}` (`Engine::transfer_group`, refused for non-members, for yourself and for non-hosts). GitHub emails the new owner, who must accept; until then nothing changes. Afterwards the new owner is the host and you are a member. Before asking, the app makes sure the former host's login is in `members`, so the new host's reconcile (§5) keeps their read access. GitHub also keeps the former owner as a collaborator of a transferred repo; the member list is what the app relies on.
- **Following a move.** GitHub redirects a renamed or transferred repo. On every sync pass, before the groups are synced, the engine asks each joined repo for its current `full_name` (`GET /repos/{o}/{r}`, `RemoteRepo::current_id`). If it differs from the stored id (case-insensitive), `rename::migrate` moves every local trace to the new id: `groups/<old>/` and `conflicts/<old>/` (renamed, never deleted), `manifest.json` (groups, files, marketplaces, order, and the `~/.claude/CLAUDE.md` import line), `settings-owned.json`, `jobs.json` (re-registering scheduled tasks on the next job reconcile), `plugins.json`, `notifications.json`, and last `state.json`, where the id is replaced in place so the group order is kept. Writes of the repo (commits, transfer, reconcile) always use the resolved new name, never the stale one.
- **Resumable.** The steps are idempotent and `state.json` is saved last, so a run interrupted halfway (folder moved, state not yet saved) finishes on the next sync; an id that is already migrated is a no-op. The host role is recomputed on the next sync from the repo owner and the signed-in login, so it flips on its own. Other groups' stored collisions that name this group's owner are recomputed by their next sync.
- **Notice.** The app shows one notification: "<old> moved to <new>; claude-sync follows it." (`SyncReport.moved`).

## 6. Sync engine

The engine talks to GitHub's REST API only. It never needs `git` on the machine.

**Base snapshot.** For each group the engine keeps the last synced state in `~/.claude-sync/groups/<owner>__<repo>/base/`: the commit SHA and a copy of every synced file. This is the common ancestor for three-way merges.

**Remote changes.**
- Poll `GET /repos/{o}/{r}/commits/{default-branch}` every 60 s (configurable) with `If-None-Match`. A `304` is free against the rate limit.
- On a new commit, fetch the tree recursively and the blobs that changed, compute remote = files under the allowlist, and merge.

**Merge, per file** (base B, local L, remote R):
- L == B: take R. R == B: keep L (it is a local change to push). L == R: nothing to do.
- All three differ: if both are JSON objects (`settings.shared.json`, `claude-sync.json`), merge key by key three-way. A key both sides changed differently: remote wins. Otherwise it is a **conflict**: remote wins, the local version is saved to `~/.claude-sync/conflicts/<group>/<UTC time>/<path>`, and the app shows it.
- Deletes follow the same table, with "missing" as a value.

**Pushing local changes.** Create blobs, a tree with `base_tree` = the remote head's tree, a commit whose parent is the remote head, then `PATCH` the ref with `force: false`. A non-fast-forward (`422`) means someone else pushed: fetch, merge, try again (at most 3 times). Never force. Before any commit, the secret scan (§9) runs on every added or changed file. A hit blocks the commit, names the file, line and pattern, and never prints the value.

**Commit messages:** `<login>@<device>: <summary>` (for example `MehdiMamas@laptop-win: update skills/review`).

**Reverting a commit (host only, §5a).** `Engine::revert_commit(group, sha)` undoes one commit from the activity feed with a NEW commit on the default branch (history is never rewritten). Everything goes through the REST `RemoteRepo`: `commit_files` lists the files the commit touched (`GET /commits/{sha}`; merge commits and commits of 300+ files are refused), then each allowlisted file is put back to its content at the commit's parent (a file the commit added is deleted, one it deleted is restored). The message is `<login>@<device>: revert <sha7> (<original summary>)`, so it shows in the feed like any change. It runs the secret scan (§9) and the push path above (retry on a race, never force), then syncs so this device applies it. **Safer rather than smarter:** if any file changed again after the commit, the whole revert is refused and nothing is committed; the error names each file, the later commit and its author (`<path> changed again after this commit, in <sha7> by <login>; revert that first.`). There is no 3-way merge. `revert_preview` returns the same plan (files, action, blocked reason) without writing, for the confirm step. A member gets `Error::HostOnly`.

## 7. Apply layer

Moves the merged group state into `~/.claude` without touching anything it does not own.

- **Managed files are copies, not links.** Copies work the same on every OS, need no admin rights, and let the app see local edits. `~/.claude-sync/manifest.json` records every file the app wrote: path, group, SHA-256.
- Targets: `skills/<name>/**` → `~/.claude/skills/<name>/**`; `agents/*.md`, `commands/*.md`, `output-styles/*.md` → the same folder under `~/.claude`; `plugins/<marketplace>/**` → `~/.claude-sync/marketplaces/<marketplace>/**` (outside `~/.claude`, so Claude Code's own plugin cache is never touched).
- **Ownership rules.** The app writes or deletes a path only if the manifest says it owns it. A path that exists but is not in the manifest belongs to the user: the group file is skipped and reported as a collision. Two groups that ship the same path: the group higher in the list (§10) wins and the other is reported.
- **Marketplaces.** The folder under `~/.claude-sync/marketplaces/` is the app's, so a stray file in the way is backed up and replaced, not reported. A marketplace belongs to the group highest in the list that ships it: a lower group gets a collision on `plugins/<marketplace>` and none of its files for it are placed. When the owner leaves, or another group is moved above it (§10), the next sync of the other group takes it over. The manifest records the owner of each marketplace and the `settings.json` entry the app set for it (§8).
- **Reload.** Claude Code reads plugins when a session starts. When an apply, join or leave changes a plugin file or a marketplace entry, the sync report carries `pluginsChanged` and the app says: "Plugins changed. Run /reload-plugins in open Claude Code sessions, or restart them."
- **CLAUDE.md.** Each device keeps its own copy of the group's `CLAUDE.md` at `~/.claude-sync/groups/<owner>__<repo>/CLAUDE.md`. Changes other members make reach it only when the user accepts them (§7a). `~/.claude/CLAUDE.md` gets one managed block at the top, with one import line per group:
  ```
  <!-- claude-sync:begin (managed, do not edit inside) -->
  @~/.claude-sync/groups/MehdiMamas__claude-sync-data/CLAUDE.md
  <!-- claude-sync:end -->
  ```
  Everything outside the block is the user's. Lines that say the same thing as, or the opposite of, a line in another group or the user's own text are listed on the group page (§7b).
- **Local edits.** A file watcher (debounced, 2 s) on managed paths compares hashes against the manifest. A changed managed file shows a notice: **Share with group** (commit it; the host only, §5a), **Keep local** (stop managing that path on this device), or **Undo** (restore the group's version). A new local skill can be shared from the app (host only). The watcher also covers `~/.claude-sync/marketplaces/`: an edited plugin file, or a new file inside a marketplace folder a group placed, gets the same notice, and Share commits it under `plugins/<marketplace>/`. Undo of a new file removes it (a copy goes to the backups).
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

**The user's own edits.** An unshared edit of the copy stays; an accepted item is patched into it. Undo (§7) returns the copy to `accepted`, not to the group's text. **Use the group's version** (group page, shown when the copy differs from the group's text and nothing is pending) sets `accepted` and the copy to the group's text and forgets the declined items. **Keep local** stops managing the copy, so nothing is reviewed for it; the group's text is still recorded as seen.

**The switch.** "Auto-accept CLAUDE.md changes" on the group page is the group's setting, `autoAcceptClaudeMd` in `claude-sync.json`, on by default (an absent key counts as on, so existing groups get it as members update). Only the host can change it (`Error::HostOnly` for a member); it is committed as `<login>@<device>: auto-accept CLAUDE.md on` or `off` (fetch and try again on a concurrent push, §6). On: every device applies other members' changes to its copy at once. Off: each member reviews them first, as above. Each device can override this on the group page ("On this device": follow the group, review first, or auto-accept); the choice is kept only on that device. A device reviews when its choice says so, else when the group's setting is off; if the group file cannot be read, auto-accept applies. When review turns off for a device, by its own choice or because the group turned on auto-accept, the items pending are accepted in that sync (the app asks first when the user changes their own choice). Past declines stay declined.

**Telling the user.** A sync that finds new items sends one notification per group ("ana changed Friends's CLAUDE.md: 1 addition, 1 removal to review"). The tray tooltip and the nav show the number waiting, the Groups list tags the group, and the group page lists each item with its author, time, commit message and lines, with Accept and Decline (and Accept all / Decline all).

**Other groups.** Review state is per group. Accepting, declining or deleting a line in one group never touches another group's copy, even when the same line is in both. Settling a clash between groups (§7b) uses the same decline: the line left out is declined in that group's copy on this device only.

**Scope.** Only `CLAUDE.md` is reviewed. Skills, agents, commands and output styles still follow §6 and §7 (the group's version applies; a local edit gets the Share / Keep local / Undo notice). Settings merge per key (§8).

## 7b. Groups that must not conflict

Claude reads every group's `CLAUDE.md` and the user's own text together, and every group's settings are merged into one `settings.json`. Two groups, or a group and the user, can say the same thing twice or the opposite. The app finds these clashes and lets the user settle each one on this device.

**What counts as a clash.** Deterministic checks, no model involved:

- Each `CLAUDE.md` Claude reads here (each group's copy as accepted, §7a, and the user's own text outside the managed block) is cut into rule lines. Headings, code blocks, import lines, HTML comments, table rows and lines of fewer than two words are left out.
- **Duplicate:** two lines from different sources that are equal once case, list markers, punctuation and spacing are ignored, or equal apart from words that only set strength or direction ("always", "do", "must", "please"), with the same sense ("Never push to main" and "Do not push to main").
- **Contradiction:** two such lines that are equal apart from those words, where one is negated and the other is not ("Always use tabs" and "Never use tabs").
- **Setting:** two groups' `settings.shared.json` set the same plain value (JSON pointer) to different values. The group higher in the list wins on apply (§8), so its side is marked "applies now". Arrays are merged by union and never clash. A value kept local for a group (below) is not applied, so it no longer clashes.

At most 200 clashes are listed. Each has an id that stays the same while both sides do.

**The Conflicts card.** On each group page: the clashes with a side in that group, both sides with the group name (or "Your CLAUDE.md") and the line or value. The card is hidden when there are none. Per item:

- **Keep A's / Keep B's** for a line: the other line is taken out of what Claude reads on this device only. In a group's copy it is declined like a change under review (§7a), so "Use the group's version" brings it back; it never changes the group for anyone else. In the user's own `CLAUDE.md` the line is removed after the file is backed up. For a duplicate, this is "keep one".
- **Edit** a line of the group on screen: the new text (redacted and scanned, §9) replaces the line in the group's `CLAUDE.md` and is committed as `edit CLAUDE.md`; the group's other members get it to review (§7a). Empty text removes the line.
- **Keep A's / Use B's** for a setting: the dropped group's value is kept local on this device (`settings.shared.json#<pointer>` in the keep-local list), so it is not applied; both groups are synced again.

**Warnings before sharing.** Sharing the user's `CLAUDE.md` with a group and copying lines to another group (§4) run the same check on the text in the preview against the target group and the user's other groups, and list the clashes under it. The source of the text (the user's own file, or the group copied from) is left out of the check.

**Not built: "Check with Claude".** A semantic check (rules that clash in meaning but not in words) could run `claude -p` the way scheduled jobs do (§17). It is not implemented; the checks above compare words only.

## 8. Settings merge

`settings.shared.json` is merged into `~/.claude/settings.json`, with ownership so removals work (code: `crates/core/src/settings.rs`):

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

Patterns in `crates/core/src/secrets.rs`: `github_pat_…`, `sk-ant-…`, `ghp_…`, `gh[orsu]_…` (OAuth, refresh, server and user tokens), `AKIA…`, `xox[abpr]-…`, private key headers, `"access_token"`/`"refresh_token"` fields. A line containing `claude-sync: allow-secret` is skipped. The same redaction patterns clean logs before they are written and before feedback is sent.

## 10. Local state

```
~/.claude-sync/
  state.json             signed-in login, device name, groups joined (the order is the priority, below), poll interval, paused
  manifest.json          managed files and marketplace owners (§7)
  settings-owned.json    settings ownership (§8)
  groups/<owner>__<repo>/base/   base snapshot (§6)
  groups/<owner>__<repo>/CLAUDE.md
                         (all keyed by group id; a rename or transfer moves them, §5b)
  marketplaces/<name>/   plugin marketplaces bundled in a group repo (§4, §7)
  jobs.json              per job: group, accepted hash, on/off, claude path, registered task;
                         groups whose jobs run without asking (§17)
  jobs/<id>/             run lock and last-run.json of a job (§17)
  plugins.json           group plugin installs: what was last done, warnings (§18)
  notifications.json     notification history (§11)
  app.json               app preferences (first-run autostart, desktop notifications on/off)
  conflicts/  backups/  logs/   (logs/jobs/<id>/<UTC stamp>.log: job output, §17)
```

**Group order.** `groups` in `state.json` is the priority list: new groups are added at the end, and when two groups set the same file, settings value or marketplace, the one higher in the list wins (§7, §8). The user changes the order with the up and down buttons on the Groups screen (the `reorder_groups` command, `Engine::reorder_groups`). It refuses a list that is not exactly the joined groups, saves `state.json` (write-then-rename), then syncs every group again in the new order, so the new winner takes over the files, the settings values it was losing (a group's claim on a value it no longer wins is dropped) and the marketplace, and the old winner is reported as a collision. If that sync fails (offline), the order is kept and the next sync finishes it. Personal memory (§16) is not a group and has no place in the list.

v0 used `~/.claude-sync/` as a git clone of the personal repo, with SessionStart/SessionEnd hooks in `~/.claude/settings.json` that ran its bash script. `crates/core/src/migrate.rs` moves that away once, at app start, when `~/.claude-sync/.git` exists:

1. v0's hook entries (commands that run `claude-sync … --hook`) are removed from `~/.claude/settings.json`, after a copy of the file goes into the backup folder. Other hooks and keys stay.
2. The clone's files go to `~/.claude-sync-v0-backup-<UTC time>/claude-sync/`. Nothing is deleted.
3. `~/.claude-sync/app/*` moves up to `~/.claude-sync/`, so the normal layout applies.
4. One notification says where the old setup went and that Personal memory (§16) can be turned on in Settings.

The clone's `.git` moves last and is what marks the migration as unfinished, so a stop half way is finished by the next start; the backup folder holds a `.migrating` file (with the names to move up) until then. `~/.claude` files other than `settings.json` and the v0 personal repo on GitHub are not touched. If the migration fails, the app keeps its state in `~/.claude-sync/app/` and tries again at the next start.

## 11. App

- **Tauri 2**, Rust backend plus a plain TypeScript UI. Tray icon with: status, Sync now, Open, Notifications (N) while N are unread, Pause, Send feedback, Quit. Single instance. Starts at login (autostart plugin, on by default, can be turned off).
- Closing the window destroys the webview, so the idle app is just the Rust process (target: under 30 MB RAM idle, under 15 MB installer).
- Screens: Sign in (device code), Groups (list in priority order with up/down buttons, status, last sync; the note "Groups higher in the list win when two groups set the same thing." shows when there are two or more), Group (what the group gives this device, plugins that failed to install (§18), scheduled jobs (§17), members, invite by GitHub login (host only; members see a line saying only the host can change the group), pending invitations, recent changes as an activity feed parsed from commit messages in `app/ui/src/activity.ts`, a **Revert** button on each feed entry (host only; opens a confirm step listing the files and why a revert is blocked, §6), conflicts, leave), Notifications, Settings (device name, poll interval, start at login, pause, desktop notifications, personal memory §16), Feedback.
- **Notifications.** Everything the app tells the user goes through one function, `notify::show(app, kind, group, title, body)` (`app/src-tauri/src/notify.rs`); new features call it rather than the notification plugin. It:
  1. records the notification in `~/.claude-sync/notifications.json` (`crates/core/src/notifications.rs`): id, UTC time, title, body, kind (`update`, `group`, `plugins`, `local-edit`, `job`, `personal`, `other`), the group it is about if any, and read/unread. Newest first, at most 200; read ones older than 30 days are dropped. A missing or corrupt file is an empty history (a corrupt one is kept as `notifications.json.bad`).
  2. shows the desktop notification, unless **Show desktop notifications** is off in Settings. The history is kept either way.
  3. emits `notifications-changed` (`{unread}`), which updates the bell's badge in the window and the tray's **Notifications (N)** item.

  The bell next to the app name opens the Notifications screen: newest first, relative time (full time on hover), kind, and the group's name, which links to the group. Opening the screen marks everything read; notifications that arrive while it is open are marked read once the window has focus. **Mark all as read** and **Clear** are there too. Clicking a desktop notification opens that screen (Windows: only while the toast is on screen, not from the Action Center; macOS: from the banner or Notification Center).

  Windows shows toasts under the app's AppUserModelID, which is the bundle identifier `com.mehdimamas.claudesync`; the NSIS installer's Start menu shortcut carries the same id, so the toasts are named and iconed as claude-sync. A dev build (`target/debug`) has no such shortcut, and the plugin falls back to PowerShell's id, so dev toasts show as "Windows PowerShell". On macOS the plugin posts through `NSUserNotificationCenter` as the signed bundle; dev builds post as Terminal.
- Logs: `tracing` to `~/.claude-sync/logs/claude-sync.log`, rotated daily, 7 kept, redacted (§9).

## 12. Relay (Vercel, free tier)

Plain TypeScript functions, no framework. Env:

| Name | What |
|---|---|
| `RELAY_GITHUB_TOKEN` | Fine-grained PAT on `RELEASES_REPO` (and `FEEDBACK_REPO` if set): Contents read/write, Issues read/write, Metadata read |
| `RELEASES_REPO` | `MehdiMamas/claude-sync` |
| `FEEDBACK_REPO` | Where feedback issues go; defaults to `RELEASES_REPO` |
| `BLOCKED_GITHUB_USERS` | Comma-separated logins refused with 403 (abuse) |
| `GITHUB_OAUTH_CLIENT_ID`, `GITHUB_OAUTH_CLIENT_SECRET`, `INSTALL_SESSION_SECRET` | The install page's GitHub sign-in (web flow of the same OAuth App the app uses; its callback URL is `<relay>/api/install/callback`) and the key that signs its session cookie. Unset: the page says sign-in is not configured |

**Auth.** Every call (except `/api/health`) carries the user's GitHub token: `Authorization: Bearer <token>`. The relay calls `GET /user` to get the login. Any GitHub account may use the relay unless its login is in `BLOCKED_GITHUB_USERS`. The login is cached in memory for 10 minutes. `/api/feedback` also uses the token to open the issue as the user. The user token is never logged or stored. Each login gets 120 calls per hour across all endpoints (`429` with `Retry-After` past that).

**`GET /api/update?target=<windows|darwin|linux>&arch=<x86_64|aarch64>&current_version=<semver>`.** Reads the latest non-draft release of `RELEASES_REPO`. Returns `204` if not newer, else the Tauri updater JSON (`version`, `notes`, `pub_date`, `url`, `signature`). `signature` is the contents of the matching `.sig` asset. `url` is `/api/download/<asset-id>` on the relay.

**`GET /api/download/<asset-id>`.** Asks GitHub for the asset with `Accept: application/octet-stream` and `redirect: manual`, then returns that `302` to the short-lived signed URL. Bytes never pass through Vercel.

**`GET /install`.** A self-contained HTML page. Signed out: **Sign in with GitHub** (`/api/install/login` → GitHub OAuth web flow, scope `read:user`, random `state` in a 10-minute HttpOnly cookie; `/api/install/callback` checks it, exchanges the code, reads the login with `GET /user`, discards the token and sets a 1-day session cookie signed with HMAC-SHA256 and `INSTALL_SESSION_SECRET`). Signed in and not in `BLOCKED_GITHUB_USERS`: the version of the latest release, buttons for its Windows `-setup.exe` and macOS `.dmg` (served by `/api/download/<asset-id>`, which accepts the session cookie as well as a bearer token) and the macOS first-run steps (Done, then Privacy & Security, Open Anyway; fallback `xattr -dr com.apple.quarantine /Applications/claude-sync.app`). A blocked login is told it isn't on the list yet.

**`POST /api/feedback`.** JSON `{ title, description, kind: "bug"|"idea", appVersion, os, logs, screenshot? (base64 PNG or JPEG, ≤ 3 MB) }`.
1. Redact `logs` again (§9) and cut them to the last 200 KB.
2. If there is a screenshot, commit it to the `feedback-assets` branch at `feedback/<yyyy-mm>/<uuid>.png` (created from the default branch if missing).
3. Create the issue **with the user's token**, so GitHub shows the user as its author: the description, app version, OS, the screenshot link, and the logs in a collapsed block. Users have no push access, so the screenshot and log files are committed with the relay token (step 2).
4. GitHub drops labels and milestone from someone without push access, so the relay then sets labels `feedback` and `type:bug` or `type:idea` and the **nearest sprint** with its own token: the open milestone titled `Sprint N` with the earliest `due_on` that is today or later. If none, the open milestone with the latest due date. If none, no milestone. Best effort: a failure is logged, the post still succeeds.
5. Return `{ number, url }`.

The issue is only ever created with the user's token, never with the relay token, so a report always sits under the account that wrote it. If GitHub refuses the user's create (401, 403, 404, 410, 422: missing scope, blocked from the repo, SSO, issues off), the relay answers 403 (422 for a 422) with `GitHub did not let your account open an issue on <repo>: <reason>`, and the app shows that message. A rate limit (429, or a 403 with `Retry-After` or no requests left) answers 429, a 5xx 502 and a network error 500: the app keeps the report in its outbox and sends it later. A report the outbox later finds refused is set aside and the user gets a notification with the reason.

Rate limit per login: 5 feedback posts per hour (in memory; good enough for a hobby).

**Feedback is public.** `FEEDBACK_REPO` defaults to the app's own repo, which is public, so the issue (title, description, author's GitHub account, app version, OS), the screenshot and the logs are public. The form says so before sending. Logs are off unless the user ticks the box; the app and the relay both redact tokens from them (§9), but they can still show file paths and repo names.

## 13. Updates and releases

- Tauri updater with its own signing key pair (minisign). The public key is in `app/src-tauri/tauri.conf.json`. The private key stays with the maintainer, outside the repo, plus the Actions secret `TAURI_SIGNING_PRIVATE_KEY` that the release workflow signs with.
- The app checks `/api/update` at start and every 6 hours, passing the user's GitHub token as a header. An update downloads in the background and installs on the next quit or when the user clicks **Restart to update**.
- Release (steps in [releasing.md](releasing.md)): a PR bumps the version with `node scripts/bump-version.mjs <version>` (workspace `Cargo.toml`, `Cargo.lock`, `app/package.json`, `app/ui/package.json`; `tauri.conf.json` takes the Cargo version) and `ci.yml` checks it. After the merge, the maintainer pushes the tag `v<version>` on the merge commit. `.github/workflows/release.yml` checks that the tag equals the version in those files, then builds on GitHub-hosted runners in the environment `release` (required reviewers; it holds the Apple signing secrets): on `windows-latest` the NSIS installer with `createUpdaterArtifacts` (the installer is also the Windows update bundle), on `macos-latest` `--target universal-apple-darwin` (the `.app.tar.gz` update bundle and the `.dmg`). Each `.sig` is checked against the public key (`scripts/verify-updater-sig.mjs`). A last job, the only one with `contents: write`, uploads the assets under the names in `relay/lib/assets.ts` to a **draft** release `v<version>`; it never publishes and never touches a published release. The maintainer publishes the draft by hand; until then `/api/update` does not see it. Release builds restore no caches.
- `scripts/release.ps1` still builds, stages and verifies the Windows installer locally (`-DryRun` for a test build, `-Publish` to upload it to the draft when the hosted build is not an option).
- A release build needs `CLAUDE_SYNC_RELAY_URL` and `CLAUDE_SYNC_GITHUB_CLIENT_ID` at compile time (Actions variables for the workflow); the workflow and `release.ps1 -Publish` refuse to build without them. The updater key has an empty password (`tauri build --ci`); locally it comes from `TAURI_SIGNING_PRIVATE_KEY` or `~/.claude-sync-keys/updater.key`.
- The update path is tested without GitHub or Vercel: `relay/dev/harness.ts` runs the relay's handlers on a local HTTP server against a stub GitHub API and asset host (`relay/test/e2e.test.ts`), and `node relay/dev/local.ts dist/release/<version>` runs it against real staged assets.
- Code signing: `release.yml` signs the macOS app with Developer ID and notarizes it when the `APPLE_*` secrets are set (Tauri imports the certificate into a temporary keychain itself); without them the build is unsigned, macOS needs right-click → Open the first time, and the job summary says so. Windows is not Authenticode-signed: SmartScreen shows once.
- Linux (beta): `release.yml` builds `.deb` and `.AppImage` on `ubuntu-22.04` (the AppImage is the update bundle, signed like the others; served as target `linux`, arch `x86_64`), and `/install` offers both. `keyring` uses the Secret Service backend (`sync-secret-service` + `crypto-rust`, Linux only), so a session needs a running Secret Service (GNOME Keyring or KWallet). Nobody has run the Linux build yet: the tray, webview, autostart, keychain and screenshot still need a runtime test on a Linux desktop, so it stays beta until then (issue #38).

## 14. First install for a friend

1. The host adds the friend's login in the app. The friend gets GitHub's invitation email.
2. The group repo's README (generated) links to the relay's install page (`<relay>/install`, §12): the friend signs in with GitHub there and downloads the Windows or macOS build.
3. The friend installs, signs in with GitHub, sees the pending invitation, clicks **Accept**. The app does the rest.

## 15. Testing

`scripts/check.ps1` (Windows) and `scripts/check.sh` (macOS, Linux) run every check locally; `ci.yml` runs the same checks on GitHub-hosted Ubuntu, Windows and macOS for every PR and push to `main`:

- `cargo test` for `crates/core`. The engine talks to GitHub through a `RemoteRepo` trait with an in-memory fake, so merge, push-retry, membership reconciliation and apply are tested without the network. Apply tests use a temp `CLAUDE_HOME`.
- `relay`: `node --test` with `fetch` mocked.
- `app`: `cargo check` and `pnpm build` for the UI. Manual smoke test per release, written down in the PR.
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
```

**What syncs.** Two kinds, each on its own switch, both on by default:

- `CLAUDE.md`: everything outside the managed block (§7), without stray group import lines. On the way back in, this device's block and imports stay where they are; the synced text goes under them, in the file's line ends. A file with only the block in it syncs nothing.
- Memory: `~/.claude/projects/<folder>/memory/**`. Claude Code names `<folder>` after the project path, with every character that is not an ASCII letter or digit turned into `-`. That cannot be decoded, so the app never decodes it. A folder under the home folder gets the key `home<rest>` (`C--Users-ana-Projects-app` and `-Users-ana-Projects-app` are both `home-Projects-app`); another device maps the key to its own `<encoded home><rest>`, and reuses an existing folder whose name differs only in case (Windows has both `C--Users-…` and `c--Users-…`). Folders outside the home folder (`/tmp`, `/private/var/folders`), temp and app-data folders, background job folders and worktree folders are per machine and never sync.

Never synced, and reported in Settings as "stays on this device": a file over 256 KiB (or past 16 MiB in all), a symlink, a file the secret scan (§9) flags, and a path that is not a safe file name. These are never written over on this device either.

**Merge.** The same three-way merge as a group (§6), with its own base in `<state>/personal/base/`. `MEMORY.md` is an index of one line per memory, and two devices adding memories at the same time is normal, so it merges as a set of lines: a line one side removed is dropped, every other line of either side is kept, and the remote's new lines go after the line that precedes them on the remote. It never conflicts. Any other file both devices changed is a conflict: the other device's version wins and this device's is saved to `<state>/conflicts/personal/<UTC time>/`. Before a sync changes or deletes `CLAUDE.md` or a memory file on this device, the app saves the old copy to `<state>/backups/personal/<UTC time>/` (under its repo path, such as `memory/<key>/<file>`).

**When it runs.** After each group sync: a conditional `GET` of the head (`304` costs nothing), and a pass only when the remote moved, the local files changed, this device was renamed, or a day passed (to refresh `lastSeenUnix`). Sync now always runs a pass.

**Turning it on and off.** Settings → Personal memory has the master switch "Sync my personal memory between my devices", the two kinds, the device list (this device marked; rename it with the Device name field; remove an old one), "New devices of mine join automatically", the last sync and "Open repo". Turning it off on a device stops syncing there and leaves its files and the repo alone. A kind turned off is remembered as it was; turned back on, it merges against that, so nothing is lost or re-added by mistake.

**A new device.** After sign-in, a device that never decided checks for `<login>/claude-sync-personal`. If the repo says `newDevicesJoin: true`, personal memory turns on by itself, the repo and the local files are merged (never overwriting either side's memories), and the app says: "Personal memory from your other devices is on. Turn off in Settings." If it says `false`, the app shows a notice offering to turn it on; "Not now" stops asking on that device. Another account on the same computer starts over with its own repo.

**Local state.**

```
<state>/personal/state.json        on/off, decided, kinds, device id, last sync, the devices seen
<state>/personal/base/             base snapshot
<state>/personal/off-claude-md/    a kind as it was when it was turned off
<state>/personal/off-memory/
```

A v0 clone of the personal repo is moved aside by the migration in §10; its CLAUDE.md and memories are not imported, the user turns this feature on instead.

## 17. Scheduled jobs

A group can ask Claude Code to run a prompt on a schedule on each member's device, for example a twice-weekly refresh of everyone's setup. Code: `crates/core/src/jobs/` (format, consent, reconcile, OS layer, runner), `app/src-tauri/src/jobs.rs` (commands, headless subcommands, notification).

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

**Consent.** Nothing is registered until the member accepts the job on the group screen, which shows the full prompt, the permission mode (`bypassPermissions` in warning style), the folder and the schedule. Everyone accepts, the host included. The acceptance stored in `jobs.json` is a SHA-256 of `[cron, prompt, cwd, permissionMode, timeoutMinutes, args]`; any change revokes it and unregisters the task until the member accepts the new version. The runner checks the hash again before every run. A per-job toggle turns an accepted job off (unregisters it) and on.

**Without asking.** A member can turn on "Run this group's jobs without asking" for one group on one device: from a pending job ("Always allow jobs from <group>") or from the top of the group's Scheduled jobs section, which says plainly that new or changed jobs then run without review. It is off by default, stored in `jobs.json` (`auto_accept`), never synced, and forgotten when the member leaves the group. While it is on, every reconcile (after each sync, and when it is turned on) accepts the current version of each of that group's jobs itself, storing the same hash an Accept stores, and the app shows a notification per job ("New job from <group>" or "Job changed in <group>"). A job the member turned off stays off. Turning it off keeps what was accepted; the next change asks again. The runner is unchanged: it still checks the stored hash.

**Reconcile.** After every sync, at app start (checking that the tasks still exist) and after accept, toggle or leave: register accepted and enabled jobs, unregister the rest, drop the records of left groups. A group whose `schedules.json` can't be read keeps its tasks.

**Runner.** The task runs the app binary headless: `claude-sync run-job <owner/repo> <id>` (no tray, no single-instance check; "Run now" adds `--now`). It takes `jobs/<id>/lock` (a lock older than the timeout plus 5 minutes is stale), skips with `skipped: offline` when `HEAD https://api.github.com` fails, and runs `claude -p <prompt> [args] --permission-mode <mode>` in `cwd` with `CREATE_NO_WINDOW`, killing the process tree at the timeout. `claude` is resolved at registration (`where.exe` on Windows, an `.exe` before a `.cmd`; known install folders and the login shell elsewhere) and again if that path is gone. Output goes to `logs/jobs/<id>/<UTC stamp>.log` (20 kept), the outcome to `jobs/<id>/last-run.json`. If the run wrote the report file, a desktop notification says so; clicking it opens the report on Windows. The task does not get `CLAUDE_SYNC_HOME`, so it always uses the default `~/.claude-sync`.

**OS layer** (behind the `Scheduler` trait; tests use a fake and never register real tasks):

- Windows: `schtasks /Create /F /TN "\claude-sync\<id>" /XML <file>`. Weekly calendar triggers, start when available, run on battery, `IgnoreNew` for overlapping runs, execution limit = timeout + 10 minutes, interactive token of the current user, hidden. `schtasks /Delete /F` to remove.
- macOS: `~/Library/LaunchAgents/dev.claude-sync.job.<id>.plist` with `StartCalendarInterval`, then `launchctl bootout` / `bootstrap gui/<uid>`.
- Linux: not supported yet (the Linux build is beta); the screen says so and Accept is disabled.
- Debug builds use `\claude-sync-dev\` and `dev.claude-sync.dev-job.`, so a dev run never touches the installed app's tasks. The ignored integration test uses `\claude-sync-test-<random hex>\`.

**Uninstall.** The NSIS pre-uninstall hook (`app/src-tauri/windows/hooks.nsh`) runs `claude-sync.exe remove-jobs`, which deletes the tasks and keeps the acceptances, so a reinstall registers them again. Updates run no uninstaller, and the hook skips `/UPDATE` anyway. On macOS, deleting the app leaves the LaunchAgents; their runs fail until removed.

## 18. Group plugin install

Placing a group's marketplace (§7) and its `enabledPlugins` (§8) is not enough: Claude Code loads only plugins installed in its plugin cache. So after a sync whose apply changed a group's bundled marketplaces (fingerprint) or added a plugin the group turns on (`true` in the group's `settings.shared.json` and in the local `settings.json`), the app runs in the background, through the same `claude` resolver as §17:

1. `claude plugin marketplace update <mp>` once per changed marketplace;
2. `claude plugin install <name>@<mp> --scope user` for each group-enabled plugin that `claude plugin list --json` does not show;
3. `claude plugin update <name>@<mp>` when the plugin's marketplace entry or declared version changed.

Each command has a 2-minute timeout and no console window; its output goes to the app log, redacted (§9). Nothing is uninstalled, and `enabledPlugins` handling stays as in §8. `-y` is never passed: a plugin that asks for confirmation is reported as needing it. Failures become warnings on the group screen ("couldn't install X: reason") with a Retry, and never fail the sync. Without `claude` the screen says "Claude Code not found" and the next sync tries again. State is in `plugins.json`; the CLI sits behind the `PluginCli` trait, and tests use a fake.

## 19. Claude helper (optional)

Explains a collision (§7), a pending CLAUDE.md change (§7a) and incoming group activity, using the user's own Anthropic API key. Off until a key is saved in Settings → "Claude helper (optional)".

- **Key:** stored only in the OS keychain, service `claude-sync`, account `@anthropic-key` (the GitHub token uses `@current` and the login). Never in `state.json`, never logged; the UI shows its last four characters. "Test key" sends one 5-token request.
- **On demand only:** "Explain" (collisions, CLAUDE.md review) and "Summarise" (activity feed) call `POST https://api.anthropic.com/v1/messages` (`x-api-key`, `anthropic-version: 2023-06-01`, model `claude-haiku-5-5`, `max_tokens` 600) when clicked. The buttons are hidden without a key. Each click costs the user money, so nothing runs automatically.
- **Before sending:** the §9 secret scan runs on the text; a hit refuses with a message that names the pattern, not the text. Input over 40 KB is cut with a note.
- **Errors in plain words:** 401/403 key rejected, 429 too many requests or no credit, 529 overloaded, no answer means offline.
- **Code:** `crates/core/src/helper.rs` (an `Http` trait so tests use a fake; the keychain goes through `token_store::Store`), commands `helper_*` in `app/src-tauri/src/commands.rs`, `app/ui/src/screens/helper.ts`.
