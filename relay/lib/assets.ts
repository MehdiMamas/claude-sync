// Release asset naming convention (relay/README.md, "Release assets").
// `version` is the release version without a leading "v".
//
//   claude-sync_<version>_windows_<arch>-setup.exe        NSIS installer, also the Tauri update bundle
//   claude-sync_<version>_windows_<arch>-setup.exe.sig    its updater signature
//   claude-sync_<version>_darwin_universal.app.tar.gz     macOS update bundle, serves aarch64 and x86_64
//   claude-sync_<version>_darwin_universal.app.tar.gz.sig
//   claude-sync_<version>_linux_<arch>.AppImage           Linux update bundle (beta)
//   claude-sync_<version>_linux_<arch>.AppImage.sig
//   claude-sync_<version>_linux_<arch>.deb                Linux package, for people only
//
// A darwin request first looks for `darwin_<arch>.app.tar.gz`, then for the universal bundle.

import type { ReleaseAsset } from "./github.ts";

export const TARGETS = ["windows", "darwin", "linux"] as const;
export const ARCHES = ["x86_64", "aarch64"] as const;
export type Target = (typeof TARGETS)[number];
export type Arch = (typeof ARCHES)[number];

export function bundleNames(version: string, target: Target, arch: Arch): string[] {
  if (target === "windows") return [`claude-sync_${version}_windows_${arch}-setup.exe`];
  if (target === "linux") return [`claude-sync_${version}_linux_${arch}.AppImage`];
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

/** The Windows installer, the macOS .dmg and the Linux AppImage and .deb of a release, for people (the install page). */
export function findInstallers(
  assets: readonly ReleaseAsset[],
  version: string,
): { windows: ReleaseAsset | null; macos: ReleaseAsset | null; linuxAppImage: ReleaseAsset | null; linuxDeb: ReleaseAsset | null } {
  return {
    windows: assets.find((a) => a.name === `claude-sync_${version}_windows_x86_64-setup.exe`) ?? null,
    macos: assets.find((a) => a.name === `claude-sync_${version}_darwin_universal.dmg`) ?? null,
    linuxAppImage: assets.find((a) => a.name === `claude-sync_${version}_linux_x86_64.AppImage`) ?? null,
    linuxDeb: assets.find((a) => a.name === `claude-sync_${version}_linux_x86_64.deb`) ?? null,
  };
}
