#!/usr/bin/env bash
set -euo pipefail

ROOT=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
cd "$ROOT" || exit 1

pass=0
fail=0

shopt -s nullglob
for t in "$ROOT"/tests/*.test.sh; do
  printf '== %s\n' "$(basename "$t")"
  if bash "$t"; then
    pass=$((pass + 1))
  else
    fail=$((fail + 1))
  fi
done
shopt -u nullglob

printf '%s passed, %s failed\n' "$pass" "$fail"
if [ "$fail" -ne 0 ]; then
  exit 1
fi
