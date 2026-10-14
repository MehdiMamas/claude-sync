<#
.SYNOPSIS
  Builds a claude-sync Windows release locally and, with -Publish, uploads it to a DRAFT GitHub release.

.DESCRIPTION
  Releases normally come from .github/workflows/release.yml (push the tag v<version> after the
  version bump PR, made with scripts/bump-version.mjs, is merged). This script is the local
  Windows build: a test build before tagging, or a fallback when the hosted build is not an option.

  Steps (docs/releasing.md has the whole flow, docs/architecture.md section 13 the design):
    1. Checks the version and the build environment.
    2. Runs scripts/check.ps1 (unless -SkipChecks).
    3. Bumps the version everywhere it lives (unless -NoBump or -Publish).
    4. Builds the NSIS installer with its updater signature (pnpm -C app tauri build).
    5. Stages the assets under dist/release/<version>/ with the names the relay expects
       (relay/lib/assets.ts) and verifies the signature against tauri.conf.json.
    6. With -Publish: creates the draft release v<version> on MehdiMamas/claude-sync
       (or uploads to it with --clobber when the draft exists). Never a published release.

  Modes:
    (none)    bump, build, stage. The bump stays in the working tree to be committed.
    -DryRun   bump, build, stage, then put the version files back. Nothing is published.
    -NoBump   build and stage the version already in the files (-Version must equal it).
    -Publish  build and stage the committed, pushed version, then upload to a draft release.
              Implies -NoBump: run the script without -Publish first, commit and push the bump.

  Signing key: env TAURI_SIGNING_PRIVATE_KEY (path or contents), else the file
  $HOME\.claude-sync-keys\updater.key. The key has an empty password; the build
  runs with --ci, which makes the Tauri CLI use an empty password when
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD is unset. The key is never printed.

.EXAMPLE
  powershell -NoProfile -ExecutionPolicy Bypass -File scripts\release.ps1 -Version 0.1.1 -DryRun
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string]$Version,
  # Release notes: a path to a file, or the text itself.
  [string]$Notes,
  [switch]$DryRun,
  [switch]$NoBump,
  [switch]$Publish,
  [switch]$SkipChecks
)

$ErrorActionPreference = 'Stop'
$Repo = 'MehdiMamas/claude-sync'
$KeyFile = Join-Path $HOME '.claude-sync-keys\updater.key'
$Root = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
Set-Location $Root

function Die([string]$Msg) {
  Write-Host "release: $Msg" -ForegroundColor Red
  exit 1
}
function Warn([string]$Msg) { Write-Host "release: WARNING: $Msg" -ForegroundColor Yellow }
function Info([string]$Msg) { Write-Host "==> $Msg" }
# Runs a native command whose stderr is noise. In Windows PowerShell 5.1 a redirected
# native stderr line is an error record, which 'Stop' would turn into a throw.
function Quiet([scriptblock]$Run) {
  $ErrorActionPreference = 'Continue'
  & $Run 2>$null
}
function Native([string]$What) {
  if ($LASTEXITCODE -ne 0) { throw "$What failed (exit $LASTEXITCODE)" }
}

# --- semver -----------------------------------------------------------------------

$SemverRe = '^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?$'

function Parse-Semver([string]$V) {
  $m = [regex]::Match($V, $SemverRe)
  if (-not $m.Success) { return $null }
  $pre = $null
  if ($m.Groups[4].Success) { $pre = $m.Groups[4].Value }
  return [pscustomobject]@{ Major = [long]$m.Groups[1].Value; Minor = [long]$m.Groups[2].Value; Patch = [long]$m.Groups[3].Value; Pre = $pre }
}

# Negative, zero or positive, like relay/lib/semver.ts (a prerelease sorts before its release).
function Compare-Semver($A, $B) {
  foreach ($f in 'Major', 'Minor', 'Patch') {
    if ($A.$f -ne $B.$f) { return [math]::Sign($A.$f - $B.$f) }
  }
  if ($A.Pre -eq $B.Pre) { return 0 }
  if ($null -eq $A.Pre) { return 1 }
  if ($null -eq $B.Pre) { return -1 }
  $pa = $A.Pre.Split('.'); $pb = $B.Pre.Split('.')
  for ($i = 0; $i -lt [math]::Min($pa.Length, $pb.Length); $i++) {
    $na = $pa[$i] -match '^\d+$'; $nb = $pb[$i] -match '^\d+$'
    if ($na -and $nb) { $c = [math]::Sign([long]$pa[$i] - [long]$pb[$i]) }
    elseif ($na) { $c = -1 }
    elseif ($nb) { $c = 1 }
    else { $c = [math]::Sign([string]::CompareOrdinal($pa[$i], $pb[$i])) }
    if ($c -ne 0) { return $c }
  }
  return [math]::Sign($pa.Length - $pb.Length)
}

# --- version files ----------------------------------------------------------------
# Every place the app version lives (found with an uncapped grep); scripts/bump-version.mjs
# has the same list, keep them in step. The app and the core
# take theirs from [workspace.package] (version.workspace = true), and tauri.conf.json has
# no "version", so Tauri uses the Cargo one. bin/claude-sync is the v0 shell CLI with its
# own version and is not part of the app release.

$Utf8 = New-Object System.Text.UTF8Encoding $false

$VersionFiles = @(
  @{ Path = 'Cargo.toml'; Pattern = '(?m)(^\[workspace\.package\][^\[]*?^version = ")([^"]+)(")' },
  @{ Path = 'Cargo.lock'; Pattern = '(?m)(^name = "claude-sync-app"\r?\nversion = ")([^"]+)(")' },
  @{ Path = 'Cargo.lock'; Pattern = '(?m)(^name = "claude-sync-core"\r?\nversion = ")([^"]+)(")' },
  @{ Path = 'app/package.json'; Pattern = '(?m)(^  "version": ")([^"]+)(")' },
  @{ Path = 'app/ui/package.json'; Pattern = '(?m)(^  "version": ")([^"]+)(")' }
)

function Read-Text([string]$Rel) { [System.IO.File]::ReadAllText((Join-Path $Root $Rel)) }

function Current-Versions {
  foreach ($f in $VersionFiles) {
    $m = [regex]::Match((Read-Text $f.Path), $f.Pattern)
    if (-not $m.Success) { Die "no version found in $($f.Path) (pattern $($f.Pattern))" }
    [pscustomobject]@{ Path = $f.Path; Version = $m.Groups[2].Value }
  }
}

$Backups = @{}
function Set-Versions([string]$New) {
  foreach ($f in $VersionFiles) {
    $abs = Join-Path $Root $f.Path
    if (-not $Backups.ContainsKey($abs)) { $Backups[$abs] = [System.IO.File]::ReadAllBytes($abs) }
    $text = [System.IO.File]::ReadAllText($abs)
    $re = New-Object System.Text.RegularExpressions.Regex $f.Pattern
    $eval = [System.Text.RegularExpressions.MatchEvaluator] { param($m) $m.Groups[1].Value + $New + $m.Groups[3].Value }
    $out = $re.Replace($text, $eval, 1)
    [System.IO.File]::WriteAllText($abs, $out, $Utf8)
  }
}
function Restore-Versions {
  foreach ($abs in $Backups.Keys) { [System.IO.File]::WriteAllBytes($abs, $Backups[$abs]) }
  if ($Backups.Count -gt 0) { Info 'version files restored' }
  $Backups.Clear()
}

# --- checks before anything changes ------------------------------------------------

$target = Parse-Semver $Version
if (-not $target) { Die "-Version must be semver like 0.2.0 or 0.2.0-rc.1 (no leading v), got '$Version'" }
if ($DryRun -and $Publish) { Die '-DryRun and -Publish cannot be combined' }
if ($Publish) { $NoBump = $true }

$versions = @(Current-Versions)
$distinct = @($versions | Select-Object -ExpandProperty Version -Unique)
if ($distinct.Count -ne 1) {
  Die ("the version files disagree: " + (($versions | ForEach-Object { "$($_.Path)=$($_.Version)" }) -join ', '))
}
$current = $distinct[0]
$cmp = Compare-Semver $target (Parse-Semver $current)
if ($NoBump) {
  if ($cmp -ne 0) {
    if ($Publish) { Die "-Publish uploads the committed version, which is $current. Run without -Publish to bump to $Version, commit and push, then publish." }
    Die "-NoBump needs -Version $current (the version in the files)"
  }
} elseif ($cmp -le 0) {
  Die "$Version is not greater than the current version $current"
}

# Never reuse a released version.
$tags = @(git tag -l 'v*') | ForEach-Object { Parse-Semver ($_ -replace '^v', '') } | Where-Object { $_ }
foreach ($t in $tags) {
  if ((Compare-Semver $target $t) -le 0) { Die "tag v$($t.Major).$($t.Minor).$($t.Patch)$(if ($t.Pre) { '-' + $t.Pre }) exists; $Version must be greater" }
}

# Build-time configuration (app/src-tauri/src/config.rs).
$relayUrl = $env:CLAUDE_SYNC_RELAY_URL
$clientId = $env:CLAUDE_SYNC_GITHUB_CLIENT_ID
$missing = @()
if ([string]::IsNullOrWhiteSpace($relayUrl) -or $relayUrl.TrimEnd('/').EndsWith('.invalid')) { $missing += 'CLAUDE_SYNC_RELAY_URL' }
if ([string]::IsNullOrWhiteSpace($clientId)) { $missing += 'CLAUDE_SYNC_GITHUB_CLIENT_ID' }
if ($missing.Count -gt 0) {
  if ($Publish) { Die "$($missing -join ' and ') must be set for a real release (the relay URL must not be the .invalid default)" }
  Warn "$($missing -join ' and ') not set: this build cannot sign in, update or send feedback. Fine for a dry run, never for a release."
}

# Signing key: path or contents in the env, else the key file. Never printed.
if ([string]::IsNullOrEmpty($env:TAURI_SIGNING_PRIVATE_KEY)) {
  if (-not (Test-Path -LiteralPath $KeyFile)) { Die "no updater key: set TAURI_SIGNING_PRIVATE_KEY or create $KeyFile" }
  $env:TAURI_SIGNING_PRIVATE_KEY = $KeyFile
  Info "signing with the key file $KeyFile"
} else {
  Info 'signing with TAURI_SIGNING_PRIVATE_KEY from the environment'
}

$existingDraft = $false
if ($Publish) {
  if (-not (Get-Command gh -ErrorAction SilentlyContinue)) { Die 'gh (GitHub CLI) is not installed' }
  Quiet { gh auth status } | Out-Null
  if ($LASTEXITCODE -ne 0) { Die 'gh is not signed in (gh auth login)' }
  if (git status --porcelain) { Die 'the working tree is not clean; commit the version bump first' }
  Quiet { git fetch --quiet origin }
  Native 'git fetch'
  $head = (git rev-parse HEAD).Trim()
  if (-not (git branch -r --contains $head)) { Die "HEAD $head is not pushed; push it so the release tag can point at it" }
  $existing = Quiet { gh release view "v$Version" --repo $Repo --json isDraft }
  if ($LASTEXITCODE -eq 0) {
    if (-not ($existing | ConvertFrom-Json).isDraft) { Die "release v$Version is already published; use a new version" }
    $existingDraft = $true
    Info "draft release v$Version exists; its assets will be replaced"
  }
}

# --- the release --------------------------------------------------------------------

$stage = Join-Path $Root "dist/release/$Version"
$assets = @()
try {
  if (-not $SkipChecks) {
    Info 'scripts/check.ps1'
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Root 'scripts/check.ps1')
    Native 'scripts/check.ps1'
  } else {
    Warn 'checks skipped (-SkipChecks)'
  }

  if (-not $NoBump) {
    Info "version $current -> $Version"
    Set-Versions $Version
    $after = @(Current-Versions | Where-Object { $_.Version -ne $Version })
    if ($after.Count -gt 0) { throw "version bump missed: $(($after | ForEach-Object { $_.Path }) -join ', ')" }
  }

  Info 'building the NSIS installer and its updater signature'
  pnpm -C app tauri build --bundles nsis --ci
  Native 'tauri build'

  $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $Root 'target' }
  $built = Join-Path $targetDir "release/bundle/nsis/claude-sync_${Version}_x64-setup.exe"
  foreach ($p in $built, "$built.sig") {
    if (-not (Test-Path -LiteralPath $p)) { throw "the build did not produce $p" }
  }

  # Names from relay/lib/assets.ts.
  if (Test-Path -LiteralPath $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
  New-Item -ItemType Directory -Force -Path $stage | Out-Null
  $exe = Join-Path $stage "claude-sync_${Version}_windows_x86_64-setup.exe"
  Copy-Item -LiteralPath $built -Destination $exe
  Copy-Item -LiteralPath "$built.sig" -Destination "$exe.sig"
  $assets = @($exe, "$exe.sig")

  Info 'verifying the updater signature against tauri.conf.json'
  node (Join-Path $Root 'scripts/verify-updater-sig.mjs') $exe "$exe.sig"
  Native 'signature check'

  if ($Publish) {
    if ($existingDraft) {
      gh release upload "v$Version" @assets --repo $Repo --clobber
      Native 'gh release upload'
    } else {
      $notesFile = Join-Path ([System.IO.Path]::GetTempPath()) "claude-sync-notes-$Version.md"
      if ($Notes -and (Test-Path -LiteralPath $Notes -PathType Leaf)) { Copy-Item -LiteralPath $Notes -Destination $notesFile -Force }
      elseif ($Notes) { [System.IO.File]::WriteAllText($notesFile, $Notes, $Utf8) }
      else { [System.IO.File]::WriteAllText($notesFile, "claude-sync $Version", $Utf8) }
      # Always --draft: publishing is a separate, manual step (docs/releasing.md).
      gh release create "v$Version" @assets --repo $Repo --draft --target $head --title "claude-sync $Version" --notes-file $notesFile
      Native 'gh release create'
      Remove-Item -LiteralPath $notesFile -Force
    }
  }
} catch {
  Restore-Versions
  Die $_.Exception.Message
}

if ($DryRun) { Restore-Versions }

# --- summary ------------------------------------------------------------------------

Write-Host ''
Info "release $Version staged in $stage"
foreach ($a in $assets) {
  $item = Get-Item -LiteralPath $a
  $hash = (Get-FileHash -LiteralPath $a -Algorithm SHA256).Hash.ToLower()
  Write-Host ("  {0}  {1,12:N0} bytes ({2:N2} MB)  sha256 {3}" -f $item.Name, $item.Length, ($item.Length / 1MB), $hash)
}
Write-Host ''
if ($Publish) {
  Write-Host "Draft release v$Version updated on $Repo with the Windows assets. The macOS assets come from release.yml (push the tag v$Version); then publish the draft (docs/releasing.md)."
} elseif ($DryRun) {
  Write-Host 'Dry run: nothing was published and the version files are back as they were.'
} elseif (-not $NoBump) {
  Write-Host "Next: commit and push the bump (git commit -am 'release: v$Version'), then run again with -Version $Version -Publish."
}
exit 0
