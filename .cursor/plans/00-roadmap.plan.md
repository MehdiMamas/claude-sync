---
name: "claude-sync – Roadmap (index only, do not Build)"
overview: "Index for the claude-sync build. It explains the architecture and the order of the numbered phase plans. It contains no implementation work. Each todo is completed by finishing the matching phase-NN-*.plan.md and passing its gate. Todo `dependencies` list only hard (Needs) prerequisites; order alone is not a dependency. Do not press Build on this file; open the phase plan instead."
todos:
  - id: phase-01-sync-engine
    content: "1 – Sync engine → complete phase-01-sync-engine.plan.md"
    status: pending
  - id: phase-02-link-layer
    content: "2 – Link layer → complete phase-02-link-layer.plan.md"
    status: pending
    dependencies: [phase-01-sync-engine]
  - id: phase-03-init-join
    content: "3 – init and join → complete phase-03-init-join.plan.md"
    status: pending
    dependencies: [phase-01-sync-engine, phase-02-link-layer]
  - id: phase-04-plugin
    content: "4 – Claude Code plugin → complete phase-04-plugin.plan.md"
    status: pending
    dependencies: [phase-03-init-join]
  - id: phase-05-doctor-release
    content: "5 – doctor, uninstall, docs, v0.1.0 → complete phase-05-doctor-release.plan.md"
    status: pending
    dependencies: [phase-04-plugin]
  - id: phase-06-cursor-sync
    content: "6 – Cursor config sync (opt-in) → complete phase-06-cursor-sync.plan.md"
    status: pending
    dependencies: [phase-03-init-join]
  - id: phase-07-shared-settings
    content: "7 – Shared settings (opt-in) → complete phase-07-shared-settings.plan.md"
    status: pending
    dependencies: [phase-03-init-join]
---

# claude-sync roadmap

claude-sync keeps a person's Claude Code setup (`CLAUDE.md`, skills, agents, commands, output styles) in a private git repo they own and syncs it across macOS, Linux and Windows machines automatically. It ships as a Claude Code plugin. Spec: `SPEC.md`.

## Architecture

```
Claude Code session
  ├─ SessionStart hook ─┐                     (plugin: hooks/hooks.json, Phase 4)
  └─ SessionEnd hook ───┤
/claude-sync:* commands ┤
terminal shim ──────────┘
                        ▼
              bin/claude-sync  (bash dispatcher)
     ┌──────────┬───────────┬────────────┬─────────────┐
 lib/sync.sh  lib/secrets.sh lib/link.sh  lib/init.sh  lib/doctor.sh
 pull/push    staged-diff    ~/.claude ⇄  init/join    doctor/uninstall
 lock, hook   scan           repo links
 mode
     └──────────────────────────┬──────────────────────┘
                                ▼
            ~/.claude-sync/  (user's private repo clone)  ⇄  GitHub
```

Dependency direction: commands and hooks → `bin/claude-sync` → `lib/*.sh`. `lib/init.sh` may call link and sync; link and sync never call init. Only `lib/sync.sh` talks to the network.

## Phases
| # | Phase | Needs | Gate (one line) |
|---|---|---|---|
| 1 | Sync engine | – | `bash tests/run.sh` passes pull/push/lock/secret/hook-mode tests locally, shellcheck clean |
| 2 | Link layer | 1 | link/unlink/status tests pass locally, including "unlink never deletes the target" |
| 3 | init and join | 1, 2 | two-machine test (init on A, join --merge on B, edit on B, pull on A) passes locally |
| 4 | Claude Code plugin | 3 | plugin validates, installs from the local marketplace, and SessionStart/SessionEnd run pull/push in a real session on one named OS |
| 5 | doctor, uninstall, docs, v0.1.0 | 4 | real round trip Windows ⇄ macOS from a clean install following only the README; release candidate ready for the user to tag |
| 6 | Cursor config sync (opt-in) | 3 | with `cursor=1`, Cursor global files sync in the two-machine test; off by default |
| 7 | Shared settings (opt-in) | 3 | `settings.shared.json` keys merge into `settings.json` without losing local keys, tested locally |

Phases 6 and 7 come after 5 in order, but only need Phase 3, so either can start once Phase 3 is merged.
