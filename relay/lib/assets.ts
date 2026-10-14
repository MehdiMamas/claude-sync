// Release asset naming convention (relay/README.md, "Release assets").
// `version` is the release version without a leading "v".
//
//   claude-sync_<version>_windows_<arch>-setup.exe        NSIS installer, also the Tauri update bundle
//   claude-sync_<version>_windows_<arch>-setup.exe.sig    its updater signature
//   claude-sync_<version>_darwin_universal.app.tar.gz     macOS update bundle, serves aarch64 and x86_64
//   claude-sync_<version>_darwin_universal.app.tar.gz.sig
//
// A darwin request first looks for `darwin_<arch>.app.tar.gz`, then for the universal bundle.

import type { ReleaseAsset } from "./github.ts";

export const TARGETS = ["windows", "darwin"] as const;
export const ARCHES = ["x86_64", "aarch64"] as const;
export type Target = (typeof TARGETS)[number];
export type Arch = (typeof ARCHES)[number];

export function bundleNames(version: string, target: Target, arch: Arch): string[] {
  if (target === "windows") return [`claude-sync_${version}_windows_${arch}-setup.exe`];
  return [`claude-sync_${version}_darwin_${arch}.app.tar.gz`, `claude-sync_${version}_darwin_universal.app.tar.gz`];
}

/** The update bundle and its signature for a platform, or null when either is missing. */
export function findBundle(
  assets: readonly ReleaseAsset[],
  version: string,
  target: Target,
  arch: Arch,
): { bundle: ReleaseAsset; sig: ReleaseAsset } | null {
  for (const name of bundleNames(version, target, arch)) {
    const bundle = assets.find((a) => a.name === name);
    const sig = assets.find((a) => a.name === `${name}.sig`);
    if (bundle && sig) return { bundle, sig };
  }
  return null;
}
