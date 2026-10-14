# claude-sync relay

A free Vercel project (Hobby plan) that does the few things that need a secret (`docs/architecture.md` §12–13):

- serves app updates from the private `MehdiMamas/claude-sync` releases,
- hands out short-lived download links for release assets (the bytes never pass through Vercel),
- files in-app feedback as an issue in the nearest sprint.

Plain TypeScript functions in `api/`, shared code in `lib/`, no framework and no runtime dependencies.

## Endpoints

Every endpoint except `/api/health` needs the caller's own GitHub token:
`Authorization: Bearer <token>`. The relay calls `GET /user` with it to learn the login. Any GitHub account is allowed, except logins listed in `BLOCKED_GITHUB_USERS` (case-insensitive, for abuse). The login is cached in memory for 10 minutes, keyed by a SHA-256 of the token. Tokens are never logged or stored.

Each login gets 120 calls per hour across all endpoints, and 5 feedback posts per hour on top of that (sliding windows, in memory per function instance). A rejected token does not count against anyone.

Errors are JSON `{ "error": "<message>" }`:

| Status | When |
|---|---|
| 400 | Bad parameters or body |
| 401 | Missing `Authorization: Bearer`, or GitHub rejects the token (`WWW-Authenticate: Bearer`) |
| 403 | Valid token, but the login is in `BLOCKED_GITHUB_USERS` |
| 404 | Unknown asset (download) |
| 413 | Screenshot over 3 MB, or body over 4.5 MB |
| 429 | More than 120 calls, or more than 5 feedback posts, from one login in the last hour (`Retry-After: <seconds>`) |
| 500 | Relay not configured (missing env) |
| 502 | GitHub failed |

### `GET /api/health`

No auth. `200 {"ok": true}`.

### `GET /api/update?target=<windows|darwin>&arch=<x86_64|aarch64>&current_version=<semver>`

Reads the latest release of `RELEASES_REPO` (`GET /repos/{repo}/releases/latest`, which skips drafts and prereleases).

- `204` when there is no release, the release is not newer than `current_version` (semver 2.0.0; `0.3.0-rc.1` is older than `0.3.0`), or the release has no update bundle and `.sig` for this platform yet.
- `200` with the Tauri updater JSON otherwise:

```json
{
  "version": "0.3.0",
  "notes": "<release body>",
  "pub_date": "2026-10-12T10:00:00Z",
  "url": "https://<relay>/api/download/123456",
  "signature": "<contents of the .sig asset>"
}
```

`url` uses `RELAY_PUBLIC_URL` when set, else the origin of the request. The app must send its GitHub token on the download too (Tauri updater `headers`).

### `GET /api/download/<asset-id>`

Asks GitHub for asset `<asset-id>` of `RELEASES_REPO` with `Accept: application/octet-stream` and `redirect: manual`, and returns `302` with GitHub's short-lived signed `Location`. Ids of other repos' assets get `404` (the lookup is scoped to `RELEASES_REPO`). A non-numeric id is `400`.

### `POST /api/feedback`

Body (JSON, at most 4.5 MB, the Vercel limit):

```json
{
  "title": "≤ 200 characters, required",
  "description": "≤ 20000 characters, required",
  "kind": "bug | idea",
  "appVersion": "0.3.0",
  "os": "Windows 11 Pro 10.0.26200",
  "logs": "optional string",
  "screenshot": "optional base64 PNG or JPEG (a data: URL prefix is accepted), ≤ 3 MB decoded"
}
```

The screenshot type is checked by its magic bytes. The app should send at most the last ~500 KB of logs so the whole body stays under 4.5 MB.

1. Title, description and logs are redacted again (the patterns of `lib/secrets.sh` plus `Bearer <token>`), then the logs are cut to their last 200 KB.
2. A screenshot is committed to branch `feedback-assets` at `feedback/<yyyy-mm>/<uuid>.png` (or `.jpg`). The branch is created from the default branch head when missing.
3. GitHub refuses issue bodies over 65,536 characters, so logs that do not fit are also committed next to the screenshot as `feedback/<yyyy-mm>/<uuid>.log`, and the issue shows their end plus a link to the file.
4. The issue goes to `FEEDBACK_REPO` with labels `feedback` and `type:bug` / `type:idea`. Its body has the description, the reporter's login, kind, app version, OS, the screenshot (`https://github.com/<repo>/blob/feedback-assets/<path>?raw=true`) and the logs in a collapsed `<details>` block.
5. Milestone: the open milestone titled `Sprint N` with the earliest due date that is today (UTC) or later; else the open `Sprint N` with the latest due date; else none.
6. Response `201 {"number": 42, "url": "https://github.com/MehdiMamas/claude-sync/issues/42"}`.

If storing the screenshot fails, the issue is still filed and says so. Rate limit: 5 posts per login per hour (sliding window, in memory per function instance). Invalid posts do not count against it.

The issue, the screenshot and the logs are public when `FEEDBACK_REPO` is a public repo. The app says so on the feedback form, and logs are only sent when the user ticks the box.

## Release assets

`scripts/release.ps1` and the macOS release workflow must name the assets exactly like this. `<version>` is the release version without the leading `v` (tag `v0.3.0` → `0.3.0`).

| Asset | What |
|---|---|
| `claude-sync_<version>_windows_x86_64-setup.exe` | NSIS installer. With `bundle.createUpdaterArtifacts: true` it is also the Tauri v2 update bundle. |
| `claude-sync_<version>_windows_x86_64-setup.exe.sig` | Its updater signature |
| `claude-sync_<version>_darwin_universal.app.tar.gz` | macOS update bundle; served to both `aarch64` and `x86_64` |
| `claude-sync_<version>_darwin_universal.app.tar.gz.sig` | Its updater signature |
| `claude-sync_<version>_darwin_universal.dmg` | macOS installer for people (not used by the updater) |

A darwin request first looks for `claude-sync_<version>_darwin_<arch>.app.tar.gz`, then the universal one, so per-arch builds can be added later. `windows_aarch64` works the same way if it is ever built. A platform whose bundle or `.sig` is not uploaded yet gets `204`, so the Windows release can go out before the macOS build finishes. The rule lives in `lib/assets.ts`.

## Environment

| Name | Required | What |
|---|---|---|
| `RELAY_GITHUB_TOKEN` | yes | Fine-grained PAT, see below |
| `RELEASES_REPO` | yes | `MehdiMamas/claude-sync` |
| `FEEDBACK_REPO` | no | Where feedback issues go. Defaults to `RELEASES_REPO` |
| `BLOCKED_GITHUB_USERS` | no | Comma-separated logins refused with 403. Everyone else with a valid GitHub token may use the relay |
| `RELAY_PUBLIC_URL` | no | Public base URL used in `/api/update` download links, e.g. `https://claude-sync-relay.vercel.app`. Defaults to the request's origin |

## The relay token (fine-grained PAT)

GitHub → Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token:

- **Resource owner:** `MehdiMamas`. **Expiration:** up to a year; note the date, the relay returns 502 once it expires.
- **Repository access:** Only select repositories → `MehdiMamas/claude-sync` (and `FEEDBACK_REPO` if it is another repo). The relay does not need access to any group repo.
- **Repository permissions:**
  - **Contents: Read and write.** Read: latest release, release assets, git refs. Write: create the `feedback-assets` branch (`POST /git/refs`) and commit screenshots (`PUT /contents/...`).
  - **Issues: Read and write.** List milestones, create the issue with labels and milestone.
  - **Metadata: Read** (always on). Repo info.

No response includes this token. Callers get only release data, a download redirect, or the new issue's number and URL.

## Deploy (Vercel CLI, Hobby plan)

From this folder (`relay/`), with `npm i -g vercel` and `vercel login` done once:

```sh
vercel link                      # create the project, e.g. "claude-sync-relay", on the Hobby plan
vercel env add RELAY_GITHUB_TOKEN production   # paste the PAT when asked
vercel env add RELEASES_REPO production        # MehdiMamas/claude-sync
vercel env add RELAY_PUBLIC_URL production     # https://<project>.vercel.app
vercel deploy --prod
curl https://<project>.vercel.app/api/health   # {"ok":true}
```

If the project is linked from the repo root instead, set its Root Directory to `relay` in the Vercel dashboard. No build command is needed: Vercel compiles each `api/**/*.ts` file (its TypeScript build rewrites the `.ts` import extensions). `vercel.json` gives each function up to 30 s. Env changes take effect on the next deploy. A relay deployed before the relay was opened to everyone may still have `ALLOWED_GITHUB_USERS` and `GROUP_REPOS` set; they are ignored now and can be removed (`vercel env rm <name> production`).

## Develop

Node 24 runs the TypeScript directly (type stripping), so tests need no build:

```sh
pnpm install
pnpm test        # node --test test/*.test.ts; fetch is mocked, nothing reaches the network
pnpm typecheck   # tsc --noEmit, strict
```

Only erasable TypeScript syntax is allowed (no enums, namespaces or parameter properties), and relative imports end in `.ts`.
