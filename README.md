# claude-sync

claude-sync is a small desktop app that shares a Claude Code setup with a group of friends. The group shares settings, `CLAUDE.md` instructions, skills, subagents, slash commands, output styles and plugin marketplaces. Everything lives in a private GitHub repo that one member of the group owns.

The app sits in the tray and starts when you log in. When someone in the group changes a shared file, the change reaches everyone's machines within about a minute. When you edit a shared file yourself, the app asks whether to share the change with the group or keep it on your machine.

It runs on Windows and macOS. Linux is not supported yet.

## Screenshots

No screenshots yet.

## Install

Download the latest version from the [Releases page](https://github.com/MehdiMamas/claude-sync/releases):

- **Windows:** `claude-sync_<version>_windows_x86_64-setup.exe`. The installer is not code-signed, so Windows SmartScreen may say it "protected your PC". Click **More info**, then **Run anyway**.
- **macOS:** `claude-sync_<version>_darwin_universal.dmg`. The app is not notarized by Apple, so macOS may refuse to open it the first time. Open it from the Applications folder with right-click, **Open**, or allow it in System Settings, Privacy & Security.

The app updates itself after that.

## Use it

1. Open claude-sync and click **Sign in with GitHub**. Your browser opens GitHub; enter the code the app shows.
2. To start a group, click **Create a group** and give it a name. The app creates a private repo on your GitHub account. You are the group's host.
3. To invite friends, open the group and add their GitHub logins. GitHub sends each of them an invitation. Any member can add or remove members, but the invitations go out from the host's app, so they are sent the next time the host's app runs.
4. To join, a friend installs claude-sync and signs in. The invitation shows under **Pending invitations**. They click **Accept** and the app sets up the group on their machine.

### Personal memory

What you tell Claude to remember about you is yours, not the group's. Turn on **Settings → Personal memory** and your own `CLAUDE.md` text and Claude's memories (the memory folder of each project) follow you to your other computers. The app keeps them in a private repo on your account, `<your login>/claude-sync-personal`. Your other computers join it automatically when they sign in with the same GitHub account; you can switch that off, choose what syncs, see and remove your devices, or turn it off on one computer. Turning it off leaves your files and the repo as they are.

Each person can sign in on as many computers as they like. Before the app changes one of your own files for the first time, it saves a copy in `~/.claude-sync/backups/`. When two people change the same file at once, the group's version wins and your version is saved next to it.

## What access it asks for

Sign-in uses GitHub's device flow and asks for two scopes:

- `repo`: to create the group's private repo on your account, invite members to it, accept invitations, and read and write the shared files. GitHub has no smaller scope that covers private repos.
- `read:user`: to read your GitHub login.

The app finds group repos by their `claude-sync-group` topic. It does not read or change your other repos.

## Only join groups you trust

A group can share Claude Code plugins. A plugin runs code on every member's machine. Settings, hooks and skills from the group also change what Claude Code does on your computer. Only join groups whose members you trust, and only add people you trust to your own group.

## Group scheduled jobs

A group repo can hold a `schedules.json` that asks Claude Code to run a prompt on a schedule, for example a weekly refresh of everyone's setup. The tray app registers a job with Task Scheduler (Windows) or launchd (macOS) only after you accept it on the group's screen, which shows the prompt, its options, the permission mode and the schedule. Any change to the job needs your OK again, and each job has an on/off toggle. If you trust a group, you can let its jobs run without asking (off by default, per device); you then get a notification for each new or changed job instead. Linux is not supported yet. The app also installs the plugins a group turns on (`claude plugin install`) and shows on the group screen any it could not install. Details are in [docs/architecture.md](docs/architecture.md) §17 and §18.

## Privacy

- Your GitHub token is kept in the operating system's keychain (Windows Credential Manager, macOS Keychain). It is never written to files or logs.
- Your group's files stay in the private repo on GitHub and on the members' computers. There is no claude-sync server that stores them.
- **Personal memory is only yours.** It goes to a private repo on your own GitHub account, and only your own computers read it. It never goes into a group repo, and group members are never added to it. The app refuses to use the repo if it is made public. The same secret check runs before anything is pushed, and memory files over 256 KB stay on their computer.
- The app's update checks and the feedback form go through a small web service (the relay, in `relay/`). It uses your GitHub token to check who you are and, for feedback, to open the issue under your account. It does not save the token.
- **Feedback is public.** The feedback form opens a public issue on this repo under your GitHub account. Anyone can read its title, the description and your GitHub username. Logs are off by default; if you include them, tokens are removed but file paths and repo names can still show. A screenshot is public too.
- Before it shares a file, the app checks it for tokens and keys and refuses to share a file that has one. Tokens are also removed from the app's logs.

## Building from source

You need Rust (stable), Node 22 or later, and pnpm.

```
pnpm -C app/ui install
pnpm -C app install
pnpm -C app tauri dev
```

The layout:

- `crates/core`: the sync engine (Rust library `claude-sync-core`).
- `app/src-tauri`: the desktop app (Tauri 2): tray, background sync, commands for the window.
- `app/ui`: the app's window (TypeScript and Vite, no framework).
- `relay/`: the web service for updates and feedback ([relay/README.md](relay/README.md)).
- `bin/`, `lib/`, `tests/`: the older bash CLI that the app replaces ([SPEC.md](SPEC.md)).

`pnpm -C app/ui dev` serves the window in a browser with a fake backend (`?signedOut` and `?empty` show other states).

Sign-in needs a GitHub OAuth app with device flow turned on: set `CLAUDE_SYNC_GITHUB_CLIENT_ID` when you build. Updates and feedback need `CLAUDE_SYNC_RELAY_URL`. A release build (`pnpm -C app tauri build`) signs its update bundles, so it also needs `TAURI_SIGNING_PRIVATE_KEY` and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`.

The design is in [docs/architecture.md](docs/architecture.md) and releases are described in [docs/releasing.md](docs/releasing.md). To contribute, see [CONTRIBUTING.md](CONTRIBUTING.md). To report a security problem, see [SECURITY.md](SECURITY.md).

## License

MIT. See [LICENSE](LICENSE).
