# AGENTS.md

## What it is
claude-sync: a small Tauri desktop app (Windows, macOS, beta Linux) that shares a Claude Code setup (settings, `CLAUDE.md`, skills, agents, commands, plugin marketplaces) with a group through a private GitHub repo. See `README.md`; design in `docs/architecture.md` (keep it in step with your change).

## Scopes
Every setting lives in exactly one scope: group, personal or project. There are no per-device choices; choices made in the app (group off, CLAUDE.md review, jobs accepted, lines kept, joined groups and their order, pause, interval, start at login, personal sync off) are personal and live in `state/claude-sync/choices.json` (`crates/core/src/choices.rs`, architecture §16); values that can't work on another machine (absolute paths, `apiKeyHelper`, secrets) are dropped automatically.

## Setup
- Needs Rust (stable), Node 22 or later and pnpm.
- Run the app: `pnpm -C app/ui install`, `pnpm -C app install`, `pnpm -C app tauri dev`.
- UI in a browser with a fake backend: `pnpm -C app/ui dev` (`?signedOut`, `?empty`, `?noHistory` show other states).
- Env vars at build time: `CLAUDE_SYNC_GITHUB_CLIENT_ID` (sign-in), `CLAUDE_SYNC_RELAY_URL` (updates and feedback). A release build (`pnpm -C app tauri build`) also needs `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

## Checks
- All checks before a PR: `scripts/check.sh` (macOS, Linux, Git Bash) or `scripts/check.ps1` (Windows).
- Single parts: `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `pnpm -C relay test`, `pnpm -C relay typecheck`, `pnpm -C app/ui test`, `pnpm -C app/ui build`.
- CI (`.github/workflows/ci.yml`) runs the same; `release.yml` builds releases (steps in `docs/releasing.md`; `scripts/bump-version.mjs` bumps versions).

## Where things live
- `crates/core`: the sync engine (Rust library `claude-sync-core`).
- `app/src-tauri`: the desktop app (tray, background sync, window commands). `app/ui`: the window (TypeScript, Vite, no framework).
- `relay/`: Vercel web service for updates and feedback (`api/`, `lib/`, `test/`; see `relay/README.md`).
- `templates/user-repo`: template for a group repo. `docs/`: architecture and releasing.

## Command line
The app binary is also the CLI (`app/src-tauri/src/cli.rs`, handled before Tauri starts, like `run-job`). Add `--json` for machine output. Exit code: 0 done, 1 failed, 2 wrong usage.
- `claude-sync status`: login, each group with its state and last sync, personal memory.
- `claude-sync personal status|enable|disable`: the same as Settings -> Personal memory. After a change the CLI sends `--refresh` to the running app (if any), which reloads its screens.
- On macOS and Linux the release app links `~/.local/bin/claude-sync` to its binary at startup (only if that folder exists and the name is free). Windows has no shim: run the full path of `claude-sync.exe`. Debug builds add no link; run `target/debug/claude-sync`.
- On Windows the release exe is a GUI program, so PowerShell shows none of its output when you call it directly: pipe it, e.g. `& "$env:LOCALAPPDATAclaude-syncclaude-sync.exe" status --json | Out-String` (#149).

## Conventions
- One change per PR; paste the commands you ran with exit codes and counts, and name the OS.
- Never commit tokens or keys; the secret-scan patterns are in `crates/core/src/secrets.rs`.

## Gotchas
- `GitHub` keeps an in-memory GET cache (URL to ETag and body, `github.rs`): a repeat GET sends `If-None-Match` and a `304` returns the cached body. Never cache non-GET calls or answers without an ETag. The head ETag is also in `head.json`; it is not part of `Head` equality.
- The background tick asks GitHub for every group repo and the personal repo in one GraphQL call (`github/graphql.rs`, `RemoteFactory::snapshots`): name and head. GraphQL has its own rate limit, kept apart from REST (`limited_until` is per resource). If the call fails, the engine falls back to the REST checks.
- Each delivered marketplace folder gets a `COMMIT` file (group main sha, then committer date; `apply/marketplaces.rs`): never a local edit, not part of a mod fingerprint, removed with the marketplace.
- Release profile keeps `panic = "unwind"` on purpose: the app catches engine panics.
- `.gitignore` excludes `target/`, `node_modules/`, `dist/`, `.vercel/`, `*.key`.
