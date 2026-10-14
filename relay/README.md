# claude-sync relay

A free Vercel project (Hobby plan) that does the few things that need a secret (`docs/architecture.md` §12–13):

- serves app updates from the `MehdiMamas/claude-sync` releases (the update JSON with its signature),
- hands out short-lived download links for release assets (the bytes never pass through Vercel),
- files in-app feedback as an issue in the nearest sprint, opened under the user's own GitHub account.

Plain TypeScript functions in `api/`, shared code in `lib/`, no framework and no runtime dependencies.

## Endpoints

Every endpoint except `/api/health` needs the caller's own GitHub token:
`Authorization: Bearer <token>`. The relay calls `GET /user` with it to learn the login. Any GitHub account is allowed, except logins listed in `BLOCKED_GITHUB_USERS` (case-insensitive, for abuse). The login is cached in memory for 10 minutes, keyed by a SHA-256 of the token. `/api/feedback` also uses the token once to open the issue as that user. Tokens are never logged or stored.

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
2. The issue goes to `FEEDBACK_REPO`, opened with the caller's token, so GitHub shows the user as its author. It has the title and a body with the description, kind, app version and OS. Nothing from the user's disk goes into it, and no labels yet.
3. The screenshot (`screenshot.png` or `.jpg`), the logs (`logs.txt`) and `report.json` (issue repo, number and URL, login, kind, app version, OS, time) are committed with the relay token to the private `FEEDBACK_ATTACHMENTS_REPO` at `issues/<number>/`, as one commit on its default branch (Git Data API: blobs, tree, commit, then the ref, retried when another post moved the ref first). Skipped when there is neither a screenshot nor logs. Best effort: on a failure the issue body says the attachments could not be stored. Because this runs after the create, a refused issue leaves no files behind.
4. GitHub drops labels and milestone sent by someone without push access, so the relay then sets labels `feedback` and `type:bug` / `type:idea` and the milestone with its own token (`PATCH /issues/{n}`, which also rewrites the body when storing failed). This comes last because the `feedback` label starts the triage workflow (below), which reads the attachments. Best effort: a failure is logged and the post still answers 201.
5. Milestone: the open milestone titled `Sprint N` with the earliest due date that is today (UTC) or later; else the open `Sprint N` with the latest due date; else none.
6. Response `201 {"number": 42, "url": "https://github.com/MehdiMamas/claude-sync/issues/42"}`.

**No other author.** The issue is only ever created with the caller's token. The relay token never creates an issue, so a report always sits under the account that wrote it and never under the owner's. When GitHub refuses the caller's create, the relay logs the status and files nothing:

| GitHub answers | Relay answers | App |
|---|---|---|
| 401, 403, 404, 410 (no repo scope, blocked from the repo, SSO, issues off) | `403 {"error": "GitHub did not let your account open an issue on <repo>: <reason> (GitHub said: <message>)."}` | Shows the message |
| 422 | `422` with the same kind of message | Shows the message |
| 429, or 403 with `Retry-After` or `x-ratelimit-remaining: 0` (a secondary rate limit) | `429` with `Retry-After` | Keeps the report in its outbox, sends it later |
| 5xx | `502` | Same |
| Network error | `500` | Same |

GitHub's `message` is redacted, put on one line and cut to 200 characters.

Rate limit: 5 posts per login per hour (sliding window, in memory per function instance). Invalid posts do not count against it.

The issue is public when `FEEDBACK_REPO` is a public repo, and it shows the user's GitHub account as its author. The screenshot and the logs never go into it: only the maintainer can read the attachments repo. The app says so on the feedback form, and logs are only sent when the user ticks the box.

### Triage (`triage/`, not deployed)

`triage/run.ts` is not part of the relay; the workflow `.github/workflows/feedback-triage.yml` runs it on GitHub Actions when the repo owner (the relay's token) adds the `feedback` label, or by hand with `gh workflow run feedback-triage.yml -f issue=<n|all> [-f force=true]`. Claude reads the issue, its private attachments and the code, and comments a write-up another agent can fix it from; spam and abuse are closed, locked and labelled `needs-moderation` instead. The steps and the checks on the comment are in `docs/architecture.md` §12. What keeps it safe:

- Only issues whose `feedback` label was added by the owner (or a login in the `TRIAGE_TRUSTED_LOGINS` variable) are read, checked from the issue's events, so a hand-made issue cannot get Claude to read another report's attachments.
- The attachments repo is cloned with a read-only deploy key (`FEEDBACK_DEPLOY_KEY`), only `issues/<number>/` of the queued issues is checked out, and the key and the clone are deleted before Claude starts.
- Claude gets Read, Grep and Glob and nothing else, and an environment without any GitHub token. Its answer is scrubbed of secrets, user names in paths and emails, its mentions, closing keywords, outside links and images are removed, and only then does the Actions token post it.

Secrets on this repo: `FEEDBACK_DEPLOY_KEY` and `CLAUDE_CODE_OAUTH_TOKEN` (`claude setup-token`). Labels `feedback`, `triaged` and `needs-moderation` must exist.

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
| `FEEDBACK_ATTACHMENTS_REPO` | no | Private repo for feedback screenshots and logs. Defaults to `<FEEDBACK_REPO>-feedback` |
| `BLOCKED_GITHUB_USERS` | no | Comma-separated logins refused with 403. Everyone else with a valid GitHub token may use the relay |
| `RELAY_PUBLIC_URL` | no | Public base URL used in `/api/update` download links, e.g. `https://claude-sync-relay.vercel.app`. Defaults to the request's origin |

## The relay token (fine-grained PAT)

GitHub → Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token:

- **Resource owner:** `MehdiMamas`. **Expiration:** up to a year; note the date, the relay returns 502 once it expires.
- **Repository access:** Only select repositories → `MehdiMamas/claude-sync`, the attachments repo (`MehdiMamas/claude-sync-feedback`, private), and `FEEDBACK_REPO` if it is another repo. The relay does not need access to any group repo.
- **Repository permissions:**
  - **Contents: Read and write.** Read: latest release, release assets. Write, on the attachments repo: commit the screenshot and logs (`POST /git/blobs`, `/git/trees`, `/git/commits`, `PATCH /git/refs/heads/<default>`).
  - **Issues: Read and write.** List milestones, and set labels, milestone and (when the attachments could not be stored) the body on the issue the user opened. The relay token never creates an issue.
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
