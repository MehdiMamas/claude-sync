# Contributing

Issues and pull requests are welcome. For a big change, open an issue first so we can agree on the approach.

## Set up

You need Rust (stable), Node 22 or later, and pnpm. The [README](README.md#building-from-source) shows how to run the app. The design is in [docs/architecture.md](docs/architecture.md); keep it in step with your change.

## Checks

Run every check before you open a pull request:

```
powershell -NoProfile -ExecutionPolicy Bypass -File scripts/check.ps1   # Windows
scripts/check.sh                                                        # macOS, Linux, Git Bash
```

They run `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test`, the relay's tests and typecheck, and the UI tests and build. They print a summary and exit non-zero if anything failed.

To run one part on its own:

```
cargo test --workspace
pnpm -C relay test
pnpm -C relay typecheck
pnpm -C app/ui test     # the UI's unit tests (activity feed parsing)
pnpm -C app/ui build
```

## Pull requests

- Keep one change per pull request.
- In the description, paste the commands you ran with their exit codes and test counts, and name the operating system.
- Never commit tokens or keys. The app's secret scan patterns are in `crates/core/src/secrets.rs`.

By contributing you agree that your work is licensed under the [MIT License](LICENSE).
