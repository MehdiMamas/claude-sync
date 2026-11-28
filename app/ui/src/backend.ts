import type { Command } from "./generated/Command";
import type { Event } from "./generated/Event";
import type { Snapshot } from "./generated/Snapshot";

/** The window's whole contract with the shell (`send`, `snapshot`, `open_target`, `snapshot` event). */
export interface Backend {
  send(cmd: Command): Promise<Event>;
  snapshot(): Promise<Snapshot>;
  subscribe(cb: (s: Snapshot) => void): () => void;
  openTarget(target: string): Promise<void>;
  /** Install the verified download and restart the app. */
  installUpdate(): Promise<void>;
  /** `granted`, `denied`, `not_asked` or `unknown` (not macOS). */
  notifyPermission(): Promise<string>;
  /** The primary screen as base64 PNG or JPEG; the shell hides the window while it grabs it. */
  captureScreen(): Promise<string>;
}

export async function tauriBackend(): Promise<Backend> {
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");
  return {
    send: (cmd) => invoke<Event>("send", { cmd }),
    snapshot: () => invoke<Snapshot>("snapshot"),
    subscribe(cb) {
      const p = listen<Snapshot>("snapshot", (e) => cb(e.payload));
      return () => void p.then((off) => off());
    },
    openTarget: (target) => invoke("open_target", { target }),
    installUpdate: () => invoke("install_update"),
    notifyPermission: () => invoke<string>("notification_permission"),
    captureScreen: () => invoke<string>("capture_screen"),
  };
}

export const inTauri = () => "__TAURI_INTERNALS__" in window;
