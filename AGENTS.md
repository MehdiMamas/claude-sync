# AGENTS.md

## What it is
claude-sync: a small Tauri desktop app (Windows, macOS, beta Linux) that shares a Claude Code setup (settings, `CLAUDE.md`, skills, agents, commands, plugin marketplaces) with a group through a private GitHub repo. See `README.md`; design in `docs/architecture.md` (keep it in step with your change).

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

## Conventions
- One change per PR; paste the commands you ran with exit codes and counts, and name the OS.
- Never commit tokens or keys; the secret-scan patterns are in `crates/core/src/secrets.rs`.

## Gotchas
- Release profile keeps `panic = "unwind"` on purpose: the app catches engine panics.
- `.gitignore` excludes `target/`, `node_modules/`, `dist/`, `.vercel/`, `*.key`.
