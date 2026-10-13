// Global UI state fed by commands and backend events.
import type { AppInfo, AuthStatus, Invitation, LocalEdit, Status, UpdateInfo } from "./api";

interface State {
  auth: AuthStatus | null;
  info: AppInfo | null;
  status: Status;
  edits: LocalEdit[];
  invitations: Invitation[];
  update: UpdateInfo | null;
}

type Listener = () => void;
const listeners: Listener[] = [];
let refreshHook: (() => void) | null = null;

export const store: State & {
  set(patch: Partial<State>): void;
  bump(): void;
} = {
  auth: null,
  info: null,
  status: { state: "idle", text: "Starting", lastSyncUnix: null, error: null },
  edits: [],
  invitations: [],
  update: null,
  set(patch) {
    Object.assign(this, patch);
    for (const l of listeners) l();
  },
  /** A sync finished: let the current screen reload its data (not its forms). */
  bump() {
    refreshHook?.();
  },
};

export function subscribe(l: Listener): void {
  listeners.push(l);
}

/** The screen on display registers how to reload its data after a sync. */
export function onRefresh(fn: (() => void) | null): void {
  refreshHook = fn;
}

export function go(path: string): void {
  location.hash = `#/${path}`;
}
