# AGENTS.md

## What it is
claude-sync: a small Tauri desktop app (Windows, macOS, beta Linux) that shares a Claude Code setup (settings, `CLAUDE.md`, skills, agents, commands, plugin marketplaces) with a group through a private GitHub repo. See `README.md`; design in `docs/architecture.md` (keep it in step with your change). 2.0.0 is a from-scratch rewrite of 0.1.x; the old code is not in the tree.

## Scopes
Every setting lives in exactly one scope: group, personal or project. There are no per-device choices; choices made in the app (group off, CLAUDE.md review, jobs accepted, lines kept, joined groups and their order, pause, interval, start at login, personal sync off) are personal: the store `settings` row `choices`, synced as `state/claude-sync/choices.json` (`crates/actions/src/choices.rs`, architecture §16). Values that can't work on another machine (absolute paths, `apiKeyHelper`, secrets) are dropped automatically.

## Setup
- Needs Rust (stable), Node 22 or later and pnpm.
- Run the app: `pnpm -C app/ui install`, `pnpm -C app install`, `pnpm -C app tauri dev`.
- UI in a browser with a fake backend: `pnpm -C app/ui dev` (`?scenario=<name>` picks a state, see `app/ui/src/fake.ts`).
- Env vars at build time: `CLAUDE_SYNC_GITHUB_CLIENT_ID` (sign-in), `CLAUDE_SYNC_RELAY_URL` (feedback). A release build (`pnpm -C app tauri build`) also needs `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

## Checks
- All checks before a PR: `scripts/check.sh` (macOS, Linux, Git Bash) or `scripts/check.ps1` (Windows).
- Single parts: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C relay test`, `pnpm -C relay typecheck`, `pnpm -C app/ui test`, `pnpm -C app/ui build`, `pnpm -C app/ui e2e` (Playwright screens).
- CI (`.github/workflows/ci.yml`) runs the same; `release.yml` builds releases (steps in `docs/releasing.md`; `scripts/bump-version.mjs` bumps versions).

## Where things live
- `crates/engine`: pure rules (formats, planner, merges, settings, review, secret scan; no I/O). `crates/github`: the GitHub client. `crates/store`: SQLite store, migrations, v1 import. `crates/fs`: disk, executor, sync pass, OS tasks, SSH peers. `crates/actions`: every user action and the actor (architecture §24).
- `crates/cli`: the command tree and the headless binary `claude-sync-cli` (servers, WSL, no GUI). `app/src-tauri`: the desktop app (tray, window, updater). `app/ui`: the window (React, TypeScript, Vite; Rust types in `src/generated/` from `scripts/gen-types.sh`).
- `relay/`: Vercel web service for updates and feedback (`api/`, `lib/`, `test/`; see `relay/README.md`).
- `templates/user-repo`: template for a group repo. `docs/`: architecture and releasing.

## Headless command line (`claude-sync-cli`)
`crates/cli` is a separate binary with no GUI and no WebKit; it depends on `claude-sync-github` with `default-features = false` (no Linux keyring), so `cargo test -p claude-sync-cli` is the headless build. Exit code: 0 done, 1 failed, 2 wrong usage; `--json` for machine output.
- Commands: `login` (device flow), `logout`, `groups`, `join <owner/repo>`, `leave`, `sync`, `status`, `personal status|enable|disable`, `ssh status|enable|disable|alias <name>`, `activity status|enable|disable`, `packs [list|setup|check|on|off <id>] [--group owner/repo]`, `review list|accept|undo <id>`, `daemon`, `service install|remove|status`, `update [--check]`, `version`. `run-job` is hidden: the OS tasks of group jobs call it.
- The token is the file `~/.claude-sync/token` (mode 0600), not the keyring. The daemon reloads it each pass.
- `service install` writes a systemd user unit when systemd runs, else an `@reboot` crontab line plus a 5-minute check. `daemon.lock` keeps one daemon alive.
- Syncs from the app and the CLI share `<state>/sync.lock`, so they never run at once.
- `join` first backs up `~/.claude/CLAUDE.md` to `~/.claude-sync/backup/<time>/` and removes group text pasted outside the managed block (`crates/cli/src/adopt.rs`).
- Install: `scripts/install-cli.sh`; releases carry `claude-sync-cli-<target>.tar.gz` (`.zip` on Windows) and a `.sha256`.

## Command line (the app binary)
The app binary is also the CLI (`app/src-tauri/src/main.rs` hands the words to `crates/cli` before Tauri starts). Same commands and exit codes as above.
- After `personal`, `ssh` or `activity` changes the CLI sends `--refresh` to the running app (if any).
- On macOS and Linux the release app links `~/.local/bin/claude-sync` to its binary at startup (only if that folder exists and the name is free). Windows has no shim: run the full path of `claude-sync.exe`; it is a GUI program, so pipe it: `& "$env:LOCALAPPDATA\claude-sync\claude-sync.exe" status --json | Out-String` (#149).
- UI preview states of packs and others are fake-backend scenarios (`app/ui/src/fake.ts`).
- SSH between your own machines (`crates/fs/src/ssh.rs`, architecture §21): the pass edits only the marked block of `~/.ssh/authorized_keys`, `~/.ssh/claude-sync.conf` and the `Include` first line of `~/.ssh/config`; tests inject their own home and never touch the real `~/.ssh`.

## Conventions
- One change per PR; paste the commands you ran with exit codes and counts, and name the OS.
- Never commit tokens or keys; the secret-scan patterns are in `crates/engine/src/secrets.rs`.

## Gotchas
- The GitHub client keeps an in-memory GET cache (URL to ETag and body, `crates/github/src/client.rs`): a repeat GET sends `If-None-Match` and a `304` returns the cached body. Never cache non-GET calls or answers without an ETag. The idle budget (424 requests an hour) is tested in `crates/github/tests/idle.rs`.
- GitHub request counts (per caller, endpoint and result) go to the store `requests` table every minute; read them with `claude-sync status --json` (`requests`). A new caller needs `with_caller("name", ...)`, else it counts as `other`.
- Rate limits are per resource (`core`, `graphql`); while one is set, no request for it goes out.
- When a head moves, files are reused from the base by git blob sha, so only changed paths are downloaded.
- Each delivered marketplace folder gets a `COMMIT` file (group main sha, then committer date): never a local edit, not part of a mod fingerprint, removed with the marketplace.
- Schema changes are a new entry at the end of `MIGRATIONS` in `crates/store/src/lib.rs`; never edit a released one.
- The v1 state files are read once by `store::import_v1` and never written or deleted (architecture §26).
- Release profile keeps `panic = "unwind"` on purpose: the app catches engine panics.
- `.gitignore` excludes `target/`, `node_modules/`, `dist/`, `.vercel/`, `*.key`.
