# Upstream edits

Files that already existed and that this phase changed, and why.

- `bin/claude-sync` — registers `init` and `join`, and passes their flags through.
- `lib/sync.sh` — `pull` calls `link` after success. `clone`, `ls-remote`, and the first `push -u` live here so `lib/init.sh` does not run network git.
- `lib/link.sh` — comment only. Pull now calls link.
- `tests/lib.sh` — `prepare_empty_bare` and `make_other_home` for init/join tests.
- `tests/dispatcher.test.sh` — help is expected to list `init` and `join`.
