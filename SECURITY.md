# Security

Please do not report security problems in a public issue, and do not use the app's feedback form for them: feedback becomes a public issue under your GitHub account.

Report them privately instead:

- On GitHub: open this repo's **Security** tab and click **Report a vulnerability**.
- Or by email: mehdimamasxd@gmail.com.

Say what the problem is, how to reproduce it, and which version and operating system you used. You will get an answer as soon as possible. Fixes ship in a new release, and the app updates itself.

Things worth reporting include a way to read another person's GitHub token, to change a group you are not a member of, to run code on a member's machine without it coming from the group's repo, to make the relay leak its own token, to make the relay use a person's token for anything other than checking who they are and opening their own feedback issue, to make the relay open an issue with its own token (under the maintainer's account) instead of the reporter's, or to get a feedback report's private screenshot or logs into a public issue or comment (for example by steering the triage script). The triage script posts only as its own GitHub App bot; a report of it posting under the maintainer's account, or of its App private key being exposed, is in scope.

The app never asks for, stores or sends an Anthropic API key. The optional Explain and Summarise helper runs the local `claude` command with your own Claude Code login, and an API key saved by an older version is deleted at start.
