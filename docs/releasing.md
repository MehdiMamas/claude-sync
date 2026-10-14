# Releasing claude-sync

The design is in [architecture.md §13](architecture.md#13-updates-and-releases). This is the checklist.

A release is a GitHub release `v<version>` on `MehdiMamas/claude-sync` with these assets (names from `relay/lib/assets.ts`):

| Asset | Built by |
|---|---|
| `claude-sync_<version>_windows_x86_64-setup.exe` | `scripts/release.ps1` (Windows PC) |
| `claude-sync_<version>_windows_x86_64-setup.exe.sig` | `scripts/release.ps1` |
| `claude-sync_<version>_darwin_universal.app.tar.gz` | `release-macos.yml` (self-hosted Mac) |
| `claude-sync_<version>_darwin_universal.app.tar.gz.sig` | `release-macos.yml` |
| `claude-sync_<version>_darwin_universal.dmg` | `release-macos.yml` |

The relay only sees releases that are published (`/releases/latest` skips drafts), so nothing reaches users until the last step.

## Once

- Updater key pair: `C:\Users\Mehdi\.claude-sync-keys\updater.key` (private, empty password, never in the repo) and its public key in `app/src-tauri/tauri.conf.json` (key id `54B66BE5E3412649`).
- Repository settings → Secrets and variables → Actions:
  - secret `TAURI_SIGNING_PRIVATE_KEY`: the contents of `updater.key`
  - variable `CLAUDE_SYNC_RELAY_URL`: the relay, e.g. `https://claude-sync-relay.vercel.app`
  - variable `CLAUDE_SYNC_GITHUB_CLIENT_ID`: the OAuth app's client ID (public)
- A self-hosted runner on the Mac with the labels `self-hosted` and `macOS`, and on it: Xcode command line tools, rustup, Node 24, pnpm, `gh`, `jq`.
- On the PC, in the shell you release from (or in your user environment):

  ```powershell
  $env:CLAUDE_SYNC_RELAY_URL = 'https://claude-sync-relay.vercel.app'
  $env:CLAUDE_SYNC_GITHUB_CLIENT_ID = '<client id>'
  ```

## 1. Windows build and the draft release

From the main checkout on `main`, clean and up to date:

```powershell
# Bump, run scripts/check.ps1, build and stage. Leaves the bump in the working tree.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 -Version 0.2.0

git commit -am "release: v0.2.0"
git push origin main

# Rebuild from the pushed commit and create the DRAFT release v0.2.0 with the Windows assets.
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 -Version 0.2.0 -Publish -Notes docs\notes-0.2.0.md
```

`-Notes` takes a file or the text itself. `-Publish` refuses when the tree is dirty, HEAD is not pushed, `CLAUDE_SYNC_RELAY_URL` or `CLAUDE_SYNC_GITHUB_CLIENT_ID` is unset (or the relay URL is the `.invalid` default), or `v0.2.0` is already published. Run it again to replace the assets of the draft. It always creates a draft; publishing is step 3.

The script prints each asset with its size and SHA-256 and has checked the `.sig` against the public key in `tauri.conf.json` (`node scripts/verify-updater-sig.mjs <file>`). The assets stay in `dist/release/<version>/` (gitignored).

Other modes:

| Command | Does |
|---|---|
| `-Version 0.2.0 -DryRun` | Bump, build, stage, verify, then put the version files back. Publishes nothing. Works without the relay URL and client ID (with a warning). |
| `-Version 0.2.0 -NoBump` | Build and stage the version already in the files. |
| `-SkipChecks` | Skip `scripts/check.ps1` (not for a real release). |

The version lives in `Cargo.toml` (`[workspace.package]`, used by the app and the core), `Cargo.lock`, `app/package.json` and `app/ui/package.json`. `tauri.conf.json` has no version of its own. The script refuses a version that is not greater than the current one or than any `v*` tag.

## 2. macOS build

```powershell
gh workflow run release-macos.yml --repo MehdiMamas/claude-sync -f version=0.2.0
gh run watch --repo MehdiMamas/claude-sync
```

The workflow runs only on the self-hosted Mac. It checks out the draft's target commit, checks the version in `Cargo.toml`, builds `--target universal-apple-darwin`, verifies the updater signature, and uploads the three macOS assets to the draft with `--clobber` (so a rerun replaces them).

## 3. Check and publish

1. Open the draft on GitHub: five assets, the notes as you want them.
2. Smoke test the installer from the draft on Windows (and the `.dmg` on the Mac): it starts, signs in, syncs. Write it down in the release PR.
3. Publish the draft (GitHub → Releases → Edit → Publish, or `gh release edit v0.2.0 --repo MehdiMamas/claude-sync --draft=false`). Publishing creates the tag `v0.2.0` at the target commit.

Within 6 hours every running app finds the update through `/api/update`, downloads it in the background and installs it on quit or on **Restart to update**.

If the Windows assets are published before the Mac build is uploaded, Macs get `204` until it is: the relay answers a platform only when its bundle and `.sig` are both there.

## Checking the update path locally

Without publishing anything, the relay can be run against a stub GitHub that lists a staged release:

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 -Version 0.2.0 -DryRun
cd relay
node dev/local.ts ..\dist\release\0.2.0 --current 0.1.0
```

It does what the app does (`GET /api/update`, then the download through `/api/download/<id>`) and checks that the signature and the downloaded bytes match the staged files. `relay/test/e2e.test.ts` runs the same path in `pnpm test`.
