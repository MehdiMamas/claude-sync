#!/usr/bin/env bash
# Run every local check: Rust format, lint and tests, the relay's tests, the app UI build.
# Exits non-zero if any check fails, after running all of them and printing a summary.
set -u
cd "$(dirname "$0")/.."

results=()
failed=0

step() {
  local name=$1
  shift
  echo
  echo "==> $name"
  if "$@"; then
    results+=("PASS  $name")
  else
    results+=("FAIL  $name (exit $?)")
    failed=1
  fi
}

# The app's Tauri crate embeds app/ui/dist at compile time, so build the UI before cargo.
if [ ! -d app/ui/node_modules ]; then
  step "pnpm -C app/ui install" pnpm -C app/ui install --frozen-lockfile
fi
step "pnpm -C app/ui test" pnpm -C app/ui test
step "pnpm -C app/ui build" pnpm -C app/ui build
step "cargo fmt --check" cargo fmt --all -- --check
step "cargo clippy" cargo clippy --workspace --all-targets -- -D warnings
step "cargo test" cargo test --workspace
if [ -f relay/package.json ]; then
  if [ ! -d relay/node_modules ]; then
    step "pnpm -C relay install" pnpm -C relay install --frozen-lockfile
  fi
  step "pnpm -C relay test" pnpm -C relay test
  step "pnpm -C relay typecheck" pnpm -C relay typecheck
else
  results+=("SKIP  relay (no relay/package.json)")
fi

echo
echo "==> Summary"
printf '  %s\n' "${results[@]}"
exit $failed
