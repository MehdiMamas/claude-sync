# claude-sync app: architecture

This document is the design the sprint issues point to. The bash CLI in `bin/` and `lib/` (described in `SPEC.md`) is v0. The app described here replaces it.

## 1. What it is

A small desktop app (Windows and macOS first, Linux later) that lives in the tray, starts at login and keeps Claude Code setups in sync:

- **Groups.** A group is a set of friends who share Claude Code settings: `settings.json` keys, skills, subagents, slash commands, output styles and `CLAUDE.md` instructions. One person **hosts** the group: the group repo lives on their GitHub account. Everyone else **joins**. Every member has the same say: anyone can change the shared settings and anyone can add or remove members.
- **Devices.** Each person can run the app on as many PCs as they like. Every device signs in with the person's own GitHub account and has full access.
- **Personal memory.** A person's own `CLAUDE.md` text and Claude's memories follow them to their other devices through a private repo of their own, never through a group (§16).
- **Live.** The app watches the remote for changes and applies them within about a minute. It watches the local files it manages, and when you edit one it asks whether to share the change with the group.
- **Updates.** The app checks for new versions and updates itself.
- **Feedback.** A feedback form sends a description, and optionally the app's recent logs and a screenshot, to this repo as a public issue, put into the nearest sprint.

It costs nothing to run. GitHub (free private repos, OAuth device flow) holds all the data. A free Vercel project (the **relay**) does the few things that need a secret: serving updates from this repo's releases and filing feedback issues.

## 2. Components

```
crates/core/        Rust library: GitHub client, group format, sync engine, apply layer, settings merge, secret scan
app/                Tauri 2 app: tray, autostart, updater, UI (app/ui, plain TypeScript + Vite), feedback capture
relay/              Vercel project (TypeScript functions): /api/update, /api/download, /api/feedback, /api/health
scripts/            Local checks, the version bump and the local Windows release script
bin/ lib/ tests/    v0 bash CLI (SPEC.md), kept for now; the app replaces it
```

GitHub Actions run on GitHub-hosted runners only (free for a public repo): `ci.yml` runs the checks of `scripts/check.ps1` on Ubuntu, Windows and macOS for every PR and push to `main`, and `release.yml` builds releases (§13). No self-hosted runners, no AI jobs.

## 3. Identity and auth

- The app signs in with the **GitHub OAuth device flow** (no client secret in the app). It needs a GitHub OAuth App with device flow enabled. Its client ID is public and is compiled in (`CLAUDE_SYNC_GITHUB_CLIENT_ID`).
- Scope: `repo` (needed to read and write private repos and to manage collaborators and invitations) and `read:user`. This is broad. A GitHub App with per-repo access is in the backlog.
- The token is stored in the OS keychain (Windows Credential Manager, macOS Keychain) through the `keyring` crate, under service `claude-sync`, account = GitHub login. It is never written to a file or a log.
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
```

`claude-sync.json`:

```json
{
  "schema": 1,
  "name": "Mehdi & friends",
  "members": ["MehdiMamas", "friend-login"],
  "sync": { "settings": true, "claudeMd": true, "skills": true, "agents": true, "commands": true, "outputStyles": true, "plugins": true }
}
```

- The host is whoever owns the repo. It is not stored in the file, so a repo transfer moves the host role without edits.
- `members` is the list of GitHub logins that should have access. Logins compare case-insensitively.
- Unknown keys are kept on rewrite. A `schema` newer than the app knows means read-only until the app updates.

### Plugins

A group can bundle Claude Code plugin marketplaces (issue #30). `plugins/<marketplace>/` is a marketplace folder exactly as Claude Code expects it: `.claude-plugin/marketplace.json` plus the plugins it lists.

- **Valid marketplace.** The folder name matches `[a-z0-9][a-z0-9-]*`, and `plugins/<marketplace>/.claude-plugin/marketplace.json` exists with `"name"` equal to the folder name. Otherwise the marketplace is not installed and the sync report says why.
- **Allowlist.** Any path below `plugins/<marketplace>/` syncs, dot-files included (`.claude-plugin/`, `.gitignore`), except `.git`, `.DS_Store`, `.env` / `.env.*` and anything under `node_modules`. The usual path checks still apply (no `..`, no Windows-reserved names).
- **Size cap.** A plugin file over 1 MiB is neither placed nor shared. It is reported, and Share refuses it.
- **Trust.** Plugins run code (hooks, MCP servers, scripts) on every member's machine. The join screens and the group README say so: "Plugins in this group run code on every member's machine; only join groups whose members you trust."
- Where the files go and how Claude Code finds them is in §7 and §8. Which plugins are on stays in `settings.shared.json` (`enabledPlugins`).

## 5. Membership as code

Every member has the same say over who is in the group, even though only the repo owner can call GitHub's collaborator API:

1. Any member adds or removes a login in the app. The app commits the change to `members` in `claude-sync.json`.
2. The **host's** app reconciles on every sync: it invites (`PUT /repos/{o}/{r}/collaborators/{login}`, permission `push`) every listed login that is not a collaborator and has no pending invitation, and removes collaborators and cancels pending invitations for logins no longer listed. The host is never removed. If the host's app is offline, reconciliation happens the next time it runs.
3. GitHub sends the invitation by email and notification. The invitee's app (if installed) lists pending invitations to repos with the `claude-sync-group` topic (`GET /user/repository_invitations`) and offers **Accept**. When access appears, the app sets the group up on that device without further questions: snapshot, apply, start watching.
4. A member whose access disappears gets the group's managed content removed from their devices (with a backup) and is told why.

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

## 7. Apply layer

Moves the merged group state into `~/.claude` without touching anything it does not own.

- **Managed files are copies, not links.** Copies work the same on every OS, need no admin rights, and let the app see local edits. `~/.claude-sync/manifest.json` records every file the app wrote: path, group, SHA-256.
- Targets: `skills/<name>/**` → `~/.claude/skills/<name>/**`; `agents/*.md`, `commands/*.md`, `output-styles/*.md` → the same folder under `~/.claude`; `plugins/<marketplace>/**` → `~/.claude-sync/marketplaces/<marketplace>/**` (outside `~/.claude`, so Claude Code's own plugin cache is never touched).
- **Ownership rules.** The app writes or deletes a path only if the manifest says it owns it. A path that exists but is not in the manifest belongs to the user: the group file is skipped and reported as a collision. Two groups that ship the same path: the group joined first wins and the other is reported.
- **Marketplaces.** The folder under `~/.claude-sync/marketplaces/` is the app's, so a stray file in the way is backed up and replaced, not reported. A marketplace belongs to the first group that placed it: if a second joined group ships the same name, that group gets a collision on `plugins/<marketplace>` and none of its files for it are placed. When the owner leaves, the next sync of the other group takes it over. The manifest records the owner of each marketplace and the `settings.json` entry the app set for it (§8).
- **Reload.** Claude Code reads plugins when a session starts. When an apply, join or leave changes a plugin file or a marketplace entry, the sync report carries `pluginsChanged` and the app says: "Plugins changed. Run /reload-plugins in open Claude Code sessions, or restart them."
- **CLAUDE.md.** The group's `CLAUDE.md` is copied to `~/.claude-sync/groups/<owner>__<repo>/CLAUDE.md`. `~/.claude/CLAUDE.md` gets one managed block at the top, with one import line per group:
  ```
  <!-- claude-sync:begin (managed, do not edit inside) -->
  @~/.claude-sync/groups/MehdiMamas__claude-sync-data/CLAUDE.md
  <!-- claude-sync:end -->
  ```
  Everything outside the block is the user's.
- **Local edits.** A file watcher (debounced, 2 s) on managed paths compares hashes against the manifest. A changed managed file shows a notice: **Share with group** (commit it), **Keep local** (stop managing that path on this device), or **Undo** (restore the group's version). A new local skill can be shared from the app. The watcher also covers `~/.claude-sync/marketplaces/`: an edited plugin file, or a new file inside a marketplace folder a group placed, gets the same notice, and Share commits it under `plugins/<marketplace>/`. Undo of a new file removes it (a copy goes to the backups).
- Every write goes through write-to-temp-then-rename. Before the first change to an existing user file (`CLAUDE.md`, `settings.json`) the app copies it to `~/.claude-sync/backups/<UTC time>/`.

## 8. Settings merge

`settings.shared.json` is merged into `~/.claude/settings.json`, keeping the v0 CLI's rules (`SPEC.md`, `lib/`) and adding ownership so removals work:

- Objects merge key by key. Arrays get the shared items they lack, appended after local items. For plain values, the shared value wins.
- `~/.claude-sync/settings-owned.json` records, per group, every value the app set (JSON pointer and value) and every array item it added.
- A key or array item the group no longer has is removed locally **only if** the local value still equals what the app set. If the user changed it, it stays, and the app offers to share the user's value with the group.
- If either file is not one JSON object, nothing is merged and the app says so. If the result equals the current file, nothing is written.
- Never shared, even if present in `settings.shared.json`: `apiKeyHelper`, `env` values that the secret scan flags, and `hooks` whose commands contain absolute paths (reported, not applied).

Plugins and marketplaces:

- A marketplace bundled under `plugins/<marketplace>/` (§4) is registered by the app, not by `settings.shared.json`: apply sets `extraKnownMarketplaces.<marketplace>` in `~/.claude/settings.json` to `{"source": {"source": "directory", "path": "<absolute path of ~/.claude-sync/marketplaces/<marketplace>>"}}`. The path is this machine's, so the value never goes through `settings.shared.json` and is never offered for sharing.
- That value is the app's. It replaces a different local value for the same name (for example a `directory` source pointing at a local checkout); `settings.json` is backed up first and the report says so. It also wins over the same key in any group's `settings.shared.json`, which is reported and not applied. It is removed when the marketplace leaves the group or the group is removed, but only while it still holds the value the app set.
- `enabledPlugins` (and any marketplace not bundled in the repo) stay in `settings.shared.json` and merge as above. A marketplace that is not bundled needs a source that works on every machine, such as `github`; a `directory` source there is a path on one machine.

## 9. Secret scan

Ported from v0 (`lib/secrets.sh`, `SPEC.md` §7): `github_pat_…`, `sk-ant-…`, `ghp_…`, `gh[osu]_…`, `AKIA…`, `xox[abpr]-…`, private key headers, `"access_token"`/`"refresh_token"` fields. A line containing `claude-sync: allow-secret` is skipped. The same redaction patterns clean logs before they are written and before feedback is sent.

## 10. Local state

```
~/.claude-sync/
  state.json             signed-in login, device name, groups joined (order matters), poll interval, paused
  manifest.json          managed files and marketplace owners (§7)
  settings-owned.json    settings ownership (§8)
  groups/<owner>__<repo>/base/   base snapshot (§6)
  groups/<owner>__<repo>/CLAUDE.md
  marketplaces/<name>/   plugin marketplaces bundled in a group repo (§4, §7)
  conflicts/  backups/  logs/
```

v0 used `~/.claude-sync/` as a git clone of the personal repo. The app checks for a `.git` folder there first and, if found, leaves it alone and stores its own state in `~/.claude-sync/app/` until personal sync is migrated.

## 11. App

- **Tauri 2**, Rust backend plus a plain TypeScript UI. Tray icon with: status, Sync now, Open, Pause, Send feedback, Quit. Single instance. Starts at login (autostart plugin, on by default, can be turned off).
- Closing the window destroys the webview, so the idle app is just the Rust process (target: under 30 MB RAM idle, under 15 MB installer).
- Screens: Sign in (device code), Groups (list, status, last sync), Group (members, invite by GitHub login, pending invitations, recent changes, conflicts, leave), Settings (device name, poll interval, start at login, pause, personal memory §16), Feedback.
- Logs: `tracing` to `~/.claude-sync/logs/claude-sync.log`, rotated daily, 7 kept, redacted (§9).

## 12. Relay (Vercel, free tier)

Plain TypeScript functions, no framework. Env:

| Name | What |
|---|---|
| `RELAY_GITHUB_TOKEN` | Fine-grained PAT on `RELEASES_REPO` (and `FEEDBACK_REPO` if set): Contents read/write, Issues read/write, Metadata read |
| `RELEASES_REPO` | `MehdiMamas/claude-sync` |
| `FEEDBACK_REPO` | Where feedback issues go; defaults to `RELEASES_REPO` |
| `BLOCKED_GITHUB_USERS` | Comma-separated logins refused with 403 (abuse) |

**Auth.** Every call (except `/api/health`) carries the user's GitHub token: `Authorization: Bearer <token>`. The relay calls `GET /user` to get the login. Any GitHub account may use the relay unless its login is in `BLOCKED_GITHUB_USERS`. The login is cached in memory for 10 minutes. The user token is never logged or stored. Each login gets 120 calls per hour across all endpoints (`429` with `Retry-After` past that).

**`GET /api/update?target=<windows|darwin>&arch=<x86_64|aarch64>&current_version=<semver>`.** Reads the latest non-draft release of `RELEASES_REPO`. Returns `204` if not newer, else the Tauri updater JSON (`version`, `notes`, `pub_date`, `url`, `signature`). `signature` is the contents of the matching `.sig` asset. `url` is `/api/download/<asset-id>` on the relay.

**`GET /api/download/<asset-id>`.** Asks GitHub for the asset with `Accept: application/octet-stream` and `redirect: manual`, then returns that `302` to the short-lived signed URL. Bytes never pass through Vercel.

**`POST /api/feedback`.** JSON `{ title, description, kind: "bug"|"idea", appVersion, os, logs, screenshot? (base64 PNG or JPEG, ≤ 3 MB) }`.
1. Redact `logs` again (§9) and cut them to the last 200 KB.
2. If there is a screenshot, commit it to the `feedback-assets` branch at `feedback/<yyyy-mm>/<uuid>.png` (created from the default branch if missing).
3. Create an issue with labels `feedback` and `type:bug` or `type:idea`, the description, reporter login, app version, OS, the screenshot link, and the logs in a collapsed block.
4. Put it in the **nearest sprint**: the open milestone titled `Sprint N` with the earliest `due_on` that is today or later. If none, the open milestone with the latest due date. If none, no milestone.
5. Return `{ number, url }`.

Rate limit per login: 5 feedback posts per hour (in memory; good enough for a hobby).

**Feedback is public.** `FEEDBACK_REPO` defaults to the app's own repo, which is public, so the issue (title, description, login, app version, OS), the screenshot and the logs are public. The form says so before sending. Logs are off unless the user ticks the box; the app and the relay both redact tokens from them (§9), but they can still show file paths and repo names.

## 13. Updates and releases

- Tauri updater with its own signing key pair (minisign). The public key is in `app/src-tauri/tauri.conf.json`. The private key stays with the maintainer, outside the repo, plus the Actions secret `TAURI_SIGNING_PRIVATE_KEY` that the release workflow signs with.
- The app checks `/api/update` at start and every 6 hours, passing the user's GitHub token as a header. An update downloads in the background and installs on the next quit or when the user clicks **Restart to update**.
- Release (steps in [releasing.md](releasing.md)): a PR bumps the version with `node scripts/bump-version.mjs <version>` (workspace `Cargo.toml`, `Cargo.lock`, `app/package.json`, `app/ui/package.json`; `tauri.conf.json` takes the Cargo version) and `ci.yml` checks it. After the merge, the maintainer pushes the tag `v<version>` on the merge commit. `.github/workflows/release.yml` checks that the tag equals the version in those files, then builds on GitHub-hosted runners in the environment `release` (required reviewers; it holds the Apple signing secrets): on `windows-latest` the NSIS installer with `createUpdaterArtifacts` (the installer is also the Windows update bundle), on `macos-latest` `--target universal-apple-darwin` (the `.app.tar.gz` update bundle and the `.dmg`). Each `.sig` is checked against the public key (`scripts/verify-updater-sig.mjs`). A last job, the only one with `contents: write`, uploads the assets under the names in `relay/lib/assets.ts` to a **draft** release `v<version>`; it never publishes and never touches a published release. The maintainer publishes the draft by hand; until then `/api/update` does not see it. Release builds restore no caches.
- `scripts/release.ps1` still builds, stages and verifies the Windows installer locally (`-DryRun` for a test build, `-Publish` to upload it to the draft when the hosted build is not an option).
- A release build needs `CLAUDE_SYNC_RELAY_URL` and `CLAUDE_SYNC_GITHUB_CLIENT_ID` at compile time (Actions variables for the workflow); the workflow and `release.ps1 -Publish` refuse to build without them. The updater key has an empty password (`tauri build --ci`); locally it comes from `TAURI_SIGNING_PRIVATE_KEY` or `~/.claude-sync-keys/updater.key`.
- The update path is tested without GitHub or Vercel: `relay/dev/harness.ts` runs the relay's handlers on a local HTTP server against a stub GitHub API and asset host (`relay/test/e2e.test.ts`), and `node relay/dev/local.ts dist/release/<version>` runs it against real staged assets.
- Code signing: `release.yml` signs the macOS app with Developer ID and notarizes it when the `APPLE_*` secrets are set (Tauri imports the certificate into a temporary keychain itself); without them the build is unsigned, macOS needs right-click → Open the first time, and the job summary says so. Windows is not Authenticode-signed: SmartScreen shows once.
- Linux: `ci.yml` builds, lints and tests on Ubuntu, but no Linux release ships yet. `keyring` in `crates/core` has no Linux backend enabled, so it falls back to its in-memory mock store and a sign-in is lost right away; it needs the `sync-secret-service` (or `linux-native-sync-persistent`) and `crypto-rust` features, a Linux target in `relay/lib/assets.ts` and a runtime test of the tray, webview and screenshot first.

## 14. First install for a friend

1. The host adds the friend's login in the app. The friend gets GitHub's invitation email.
2. The group repo's README (generated) links to the relay's install page. Until that page exists, the host sends the installer directly.
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

**Merge.** The same three-way merge as a group (§6), with its own base in `<state>/personal/base/`. `MEMORY.md` is an index of one line per memory, and two devices adding memories at the same time is normal, so it merges as a set of lines: a line one side removed is dropped, every other line of either side is kept, and the remote's new lines go after the line that precedes them on the remote. It never conflicts. Any other file both devices changed is a conflict: the other device's version wins and this device's is saved to `<state>/conflicts/personal/<UTC time>/`. Before the first change to `CLAUDE.md` the app saves a copy to `<state>/backups/personal/<UTC time>/`.

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

The v0 bash CLI's personal repo (§10) is not migrated.
