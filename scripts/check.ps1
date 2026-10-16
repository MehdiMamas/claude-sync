# Run every local check: Rust format, lint and tests, the relay's tests, the app UI build.
# Exits non-zero if any check fails, after running all of them and printing a summary.
$ErrorActionPreference = 'Continue'
Set-Location (Join-Path $PSScriptRoot '..')

$results = New-Object System.Collections.Generic.List[string]
$failed = $false

function Step([string]$Name, [scriptblock]$Run) {
  Write-Host ''
  Write-Host "==> $Name"
  & $Run
  $code = $LASTEXITCODE
  if ($code -eq 0) {
    $script:results.Add("PASS  $Name")
  } else {
    $script:results.Add("FAIL  $Name (exit $code)")
    $script:failed = $true
  }
}

# The app's Tauri crate embeds app/ui/dist at compile time, so build the UI before cargo.
if (-not (Test-Path 'app/ui/node_modules')) {
  Step 'pnpm -C app/ui install' { pnpm -C app/ui install --frozen-lockfile }
}
Step 'pnpm -C app/ui test' { pnpm -C app/ui test }
Step 'pnpm -C app/ui build' { pnpm -C app/ui build }
Step 'cargo fmt --check' { cargo fmt --all -- --check }
Step 'cargo clippy' { cargo clippy --workspace --all-targets -- -D warnings }
Step 'cargo test' { cargo test --workspace }
if (Test-Path 'relay/package.json') {
  if (-not (Test-Path 'relay/node_modules')) {
    Step 'pnpm -C relay install' { pnpm -C relay install --frozen-lockfile }
  }
  Step 'pnpm -C relay test' { pnpm -C relay test }
  Step 'pnpm -C relay typecheck' { pnpm -C relay typecheck }
} else {
  $results.Add('SKIP  relay (no relay/package.json)')
}

Write-Host ''
Write-Host '==> Summary'
foreach ($r in $results) { Write-Host "  $r" }
if ($failed) { exit 1 } else { exit 0 }
