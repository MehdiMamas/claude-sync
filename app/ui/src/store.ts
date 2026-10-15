// Global UI state fed by commands and backend events.
import type { AppInfo, AuthStatus, GroupStatus, Invitation, LocalEdit, PersonalStatus, Status, UpdateInfo } from "./api";

interface State {
  auth: AuthStatus | null;
  info: AppInfo | null;
  status: Status;
  edits: LocalEdit[];
  invitations: Invitation[];
  update: UpdateInfo | null;
  /** A sync, join or leave changed plugins; cleared when the user dismisses the notice. */
  pluginsChanged: boolean;
  /** Personal memory on this device; drives the offer notice. */
  personal: PersonalStatus | null;
  /** Unread notifications (the bell's badge). */
  unread: number;
  /** CLAUDE.md changes from others waiting for review, over all groups (the nav badge). */
  review: number;
  /** Groups ("owner/repo") the user does not host: their edits there stay on this device. */
  guestOf: string[];
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
  pluginsChanged: false,
  personal: null,
  unread: 0,
  review: 0,
  guestOf: [],
  set(patch) {
    Object.assign(this, patch);
    for (const l of listeners) l();
  },
  /** A sync finished: let the current screen reload its data (not its forms). */
  bump() {
    refreshHook?.();
  },
};

/** Keep the nav badge and the groups the user does not host in step with a fresh group list. */
export function countReview(groups: GroupStatus[]): void {
  const review = groups.reduce((n, g) => n + (g.reviewPending ?? 0), 0);
  const guestOf = groups.filter((g) => !g.isHost).map((g) => `${g.id.owner}/${g.id.repo}`);
  if (review !== store.review || guestOf.join() !== store.guestOf.join()) store.set({ review, guestOf });
}

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
