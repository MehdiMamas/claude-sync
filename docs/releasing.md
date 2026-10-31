# Releasing claude-sync

The design is in [architecture.md §13](architecture.md#13-updates-and-releases). This is the checklist.

A release is a GitHub release `v<version>` on `MehdiMamas/claude-sync` with these assets (names from `relay/lib/assets.ts`). `.github/workflows/release.yml` builds all of them on GitHub-hosted runners:

| Asset | Runner |
|---|---|
| `claude-sync_<version>_windows_x86_64-setup.exe` | `windows-latest` |
| `claude-sync_<version>_windows_x86_64-setup.exe.sig` | `windows-latest` |
| `claude-sync_<version>_darwin_universal.app.tar.gz` | `macos-latest` |
| `claude-sync_<version>_darwin_universal.app.tar.gz.sig` | `macos-latest` |
| `claude-sync_<version>_darwin_universal.dmg` | `macos-latest` |
| `claude-sync_<version>_linux_x86_64.AppImage` (+ `.sig`) and `.deb` | `ubuntu-22.04` (beta) |

The workflow only ever creates or updates a **draft**. The relay only sees releases that are published (`/releases/latest` skips drafts), so nothing reaches users until the last step.

## Once

All of this is in place on `MehdiMamas/claude-sync`. The list is here for a new key or a fork.

- Updater key pair: `~/.claude-sync-keys/updater.key` on the maintainer's machine (private, empty password, never in the repo) and its public key in `app/src-tauri/tauri.conf.json` (key id `54B66BE5E3412649`).
- Environment `release` (Settings → Environments) with the maintainer as required reviewer. Every build job of `release.yml` waits for that approval.
- Secrets:

  | Secret | Where | What |
  |---|---|---|
  | `TAURI_SIGNING_PRIVATE_KEY` | repository (could move into `release`) | the contents of `updater.key` |
  | `APPLE_CERTIFICATE` | environment `release` | base64 of the Developer ID Application certificate exported as `.p12` (`base64 -i cert.p12 \| pbcopy`) |
  | `APPLE_CERTIFICATE_PASSWORD` | environment `release` | the `.p12` password |
  | `APPLE_SIGNING_IDENTITY` | environment `release` | e.g. `Developer ID Application: <name> (TEAMID)` (`security find-identity -v -p codesigning`) |
  | `APPLE_API_ISSUER` | environment `release` | App Store Connect → Users and Access → Integrations → Keys: the issuer id |
  | `APPLE_API_KEY` | environment `release` | the key id of that API key |
  | `APPLE_API_KEY_P8` | environment `release` | the contents of `AuthKey_<key id>.p8` |

  A job in the environment sees both levels. Without any `APPLE_*` secret the macOS build is unsigned and the run's summary says so; with only some of them the macOS job fails.
- Repository variables (Settings → Secrets and variables → Actions → Variables):
  - `CLAUDE_SYNC_RELAY_URL`: the relay, e.g. `https://claude-sync-relay.vercel.app`
  - `CLAUDE_SYNC_GITHUB_CLIENT_ID`: the OAuth app's client ID (public)
- No self-hosted runners. The repo is public, so every workflow runs on GitHub-hosted runners.

## 1. Bump the version in a PR

```powershell
git switch -c release-0.2.0 origin/main
node scripts/bump-version.mjs 0.2.0
git commit -am "release: v0.2.0"
git push -u origin release-0.2.0
gh pr create --repo MehdiMamas/claude-sync --title "release: v0.2.0" --body "<notes>"
```

`bump-version.mjs` sets the version in `Cargo.toml` (`[workspace.package]`, used by the app and the core), `Cargo.lock`, `app/package.json` and `app/ui/package.json` (`tauri.conf.json` has no version of its own). It refuses a version that is not greater than the current one. `node scripts/bump-version.mjs --check 0.2.0` checks all five places, and `--print` prints the version.

`ci.yml` runs the checks on Ubuntu, Windows and macOS. Merge when it is green.

## 2. Tag, approve, build

```powershell
git switch main
git pull --ff-only
git tag v0.2.0
git push origin v0.2.0
gh run watch --repo MehdiMamas/claude-sync
```

The tag starts `release.yml`:

1. `prepare` checks that the tag is semver, that `v0.2.0` is not already a published release, and that the version files at the tagged commit say `0.2.0` (`bump-version.mjs --check`). A mismatch fails the run before anything is built.
2. `build (windows)` and `build (macos)` wait for your approval (environment `release`): approve them in the run page or with `gh run view --web`. They build without caches, check each updater `.sig` against the public key in `tauri.conf.json` (`scripts/verify-updater-sig.mjs`), and on macOS check `codesign`, the stapled notarization ticket and Gatekeeper (`spctl`) when signed.
3. `draft` creates the draft release `v0.2.0` with all eight assets (release notes generated from the merged PRs), or replaces the assets of an existing draft (`--clobber`). It refuses to touch a published release. The run summary lists the SHA-256 of every asset.

To rebuild (a flaky runner, a new secret): rerun the failed jobs, or `gh workflow run release.yml --repo MehdiMamas/claude-sync -f version=0.2.0`. A manual run builds the commit of the tag `v0.2.0` if it exists, else the branch you run it on.

## 3. Check and publish

1. Open the draft on GitHub: eight assets (the table above), edit the notes (they are what the app shows with the update).
2. Smoke test the installer from the draft on Windows and the `.dmg` on the Mac: it starts, signs in, syncs. Write it down in the release PR.
3. Publish the draft (GitHub → Releases → Edit → Publish, or `gh release edit v0.2.0 --repo MehdiMamas/claude-sync --draft=false`).

Within 6 hours every running app finds the update through `/api/update`, downloads it in the background and installs it on quit or on **Restart to update**.

The relay answers a platform only when its bundle and `.sig` are both in the release; a platform without them gets `204`.

## Linux

Releases build Windows, macOS and Linux (Linux is beta). `ci.yml` compiles, lints and tests on Ubuntu, but the app cannot keep a sign-in there: the `keyring` crate in `crates/core` has no Linux backend enabled, so it falls back to its in-memory mock store (see architecture.md §13). Before a Linux release: enable a Linux keyring backend, test the app on a Linux desktop, add `linux` to `relay/lib/assets.ts` (`claude-sync_<version>_linux_x86_64.AppImage` and its `.sig`, plus a `.deb`), and add an `ubuntu-22.04` or `ubuntu-24.04` leg (`--bundles appimage,deb`) to `release.yml`.

## Local Windows build

`scripts/release.ps1` builds the Windows installer on the PC, for testing a release before tagging it or when the hosted build is not an option:

```powershell
# Bump, run scripts/check.ps1, build, stage and verify, then put the version files back.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 -Version 0.2.0 -DryRun
```

| Command | Does |
|---|---|
| `-Version 0.2.0 -DryRun` | Bump, build, stage, verify, then put the version files back. Publishes nothing. Works without the relay URL and client ID (with a warning). |
| `-Version 0.2.0` | Bump, build, stage. The bump stays in the working tree (use `bump-version.mjs` in a PR instead). |
| `-Version 0.2.0 -NoBump` | Build and stage the version already in the files. |
| `-Version 0.2.0 -Publish` | Build the committed, pushed version and upload the Windows assets to the draft `v0.2.0` (creates it if missing). The tag workflow would replace them with its own build. |
| `-SkipChecks` | Skip `scripts/check.ps1` (not for a real release). |

Locally the signing key comes from `TAURI_SIGNING_PRIVATE_KEY` (path or contents) or `~/.claude-sync-keys/updater.key` (`$HOME\.claude-sync-keys\updater.key` on Windows), and the build needs `CLAUDE_SYNC_RELAY_URL` and `CLAUDE_SYNC_GITHUB_CLIENT_ID` in the environment. The assets stay in `dist/release/<version>/` (gitignored), printed with their size and SHA-256.

## Checking the update path locally

Without publishing anything, the relay can be run against a stub GitHub that lists a staged release:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 -Version 0.2.0 -DryRun
cd relay
node dev/local.ts ..\dist\release\0.2.0 --current 0.1.0
```

It does what the app does (`GET /api/update`, then the download through `/api/download/<id>`) and checks that the signature and the downloaded bytes match the staged files. `relay/test/e2e.test.ts` runs the same path in `pnpm test`.
