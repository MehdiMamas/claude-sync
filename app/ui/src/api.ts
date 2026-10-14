// The app's backend API: Tauri commands and events (app/src-tauri/src/commands.rs).
// Types mirror the Rust structs (serde camelCase).
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export interface GroupId {
  owner: string;
  repo: string;
}
export type Collision =
  | { kind: "userFile"; path: string }
  | { kind: "otherGroup"; path: string; owner: GroupId };
export interface ConflictInfo {
  path: string;
  savedTo: string;
  atUnix: number;
}
export interface GroupStatus {
  id: GroupId;
  name: string;
  host: string;
  members: string[];
  invited: string[];
  lastSyncUnix: number | null;
  lastError: string | null;
  conflicts: ConflictInfo[];
  collisions: Collision[];
  /** Only on the status a join returns: plugin files were placed or changed. */
  pluginsChanged: boolean;
}
export interface PluginInfo {
  name: string;
  description: string | null;
  version: string | null;
  /** `enabledPlugins["<plugin>@<marketplace>"]` is true in ~/.claude/settings.json. */
  enabled: boolean;
}
export interface MarketplaceInfo {
  name: string;
  description: string | null;
  /** Its folder on this device; null when this group's copy is not placed here. */
  path: string | null;
  /** Another group placed a marketplace with this name first. */
  heldBy: GroupId | null;
  plugins: PluginInfo[];
}
export interface ContentItem {
  name: string;
  description: string | null;
}
export interface GroupContents {
  marketplaces: MarketplaceInfo[];
  skills: ContentItem[];
  agents: ContentItem[];
  commands: ContentItem[];
  outputStyles: ContentItem[];
  settings: {
    /** Top-level keys of the group's settings.shared.json, in file order. */
    keys: string[];
    /** `[<plugin>@<marketplace>, on]` */
    enabledPlugins: [string, boolean][];
    /** This device stopped applying the group's settings. */
    keptLocal: boolean;
  };
  /** The group's CLAUDE.md copy, when it has one. */
  claudeMd: string | null;
}
export interface ApplyReport {
  written: string[];
  removed: string[];
  collisions: Collision[];
  settingsSkipped: string[];
  kept: string[];
  skipped: string[];
  pluginsChanged: boolean;
}
export interface GroupSyncReport {
  id: GroupId;
  pulled: boolean;
  pushed: boolean;
  apply: ApplyReport;
  conflicts: ConflictInfo[];
  error: string | null;
  lostAccess: boolean;
}
export interface SyncReport {
  groups: GroupSyncReport[];
}
export interface CommitInfo {
  sha: string;
  message: string;
  author: string;
  atUnix: number;
}
export interface Invitation {
  id: number;
  repo: GroupId;
  inviter: string;
}
export interface LocalEdit {
  group: GroupId;
  path: string;
  deleted: boolean;
}
export type EditAction = "share" | "keepLocal" | "undo";
export interface Status {
  state: "signedOut" | "paused" | "syncing" | "ok" | "error" | "idle";
  text: string;
  lastSyncUnix: number | null;
  error: string | null;
}
export interface Settings {
  deviceName: string;
  pollSeconds: number;
  autostart: boolean;
  paused: boolean;
}
export interface UpdateInfo {
  current: string;
  available: boolean;
  version: string | null;
  notes: string | null;
  downloaded: boolean;
}
export interface Screenshot {
  data: string;
  mime: string;
  width: number;
  height: number;
  bytes: number;
}
export interface FeedbackInput {
  kind: "bug" | "idea";
  title: string;
  description: string;
  includeLogs: boolean;
  screenshot: Screenshot | null;
}
export interface FeedbackResult {
  queued: boolean;
  number: number | null;
  url: string | null;
}
export interface DeviceCode {
  userCode: string;
  verificationUri: string;
  expiresIn: number;
}
export interface AuthStatus {
  signedIn: boolean;
  login: string | null;
  clientIdMissing: boolean;
}
export interface AppInfo {
  version: string;
  os: string;
  relayConfigured: boolean;
  clientIdMissing: boolean;
  stateDir: string;
}

export interface Events {
  status: Status;
  "sync-report": SyncReport;
  "plugins-changed": null;
  "local-edits": LocalEdit[];
  invitations: Invitation[];
  "update-available": UpdateInfo;
  "signed-in": { login: string };
  "sign-in-failed": { message: string };
  navigate: string;
}

export interface Backend {
  invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T>;
  listen<K extends keyof Events>(event: K, cb: (payload: Events[K]) => void): Promise<() => void>;
}

const tauriBackend: Backend = {
  invoke: (cmd, args) => invoke(cmd, args),
  listen: (event, cb) => listen(event, (e) => cb(e.payload as never)),
};

let backend: Backend = tauriBackend;
export function setBackend(b: Backend): void {
  backend = b;
}

/** Errors from commands are plain strings; normalise anything else. */
export function errorText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return JSON.stringify(e);
}

const call = <T>(cmd: string, args?: Record<string, unknown>) => backend.invoke<T>(cmd, args);
export const on = <K extends keyof Events>(event: K, cb: (p: Events[K]) => void) => backend.listen(event, cb);

export const api = {
  appInfo: () => call<AppInfo>("app_info"),
  authStatus: () => call<AuthStatus>("auth_status"),
  startSignIn: () => call<DeviceCode>("start_sign_in"),
  cancelSignIn: () => call<void>("cancel_sign_in"),
  signOut: () => call<void>("sign_out"),
  status: () => call<Status>("status"),
  groups: () => call<GroupStatus[]>("groups"),
  createGroup: (name: string, repo?: string) => call<GroupStatus>("create_group", { name, repo }),
  invitations: () => call<Invitation[]>("invitations"),
  acceptInvitation: (id: number) => call<GroupStatus>("accept_invitation", { id }),
  discover: () => call<GroupId[]>("discover"),
  join: (group: GroupId) => call<GroupStatus>("join", { group }),
  addMember: (group: GroupId, login: string) => call<void>("add_member", { group, login }),
  removeMember: (group: GroupId, login: string) => call<void>("remove_member", { group, login }),
  leaveGroup: (group: GroupId) => call<void>("leave_group", { group }),
  localEdits: () => call<LocalEdit[]>("local_edits"),
  resolveEdit: (edit: LocalEdit, action: EditAction) => call<void>("resolve_edit", { edit, action }),
  groupContents: (group: GroupId) => call<GroupContents>("group_contents", { group }),
  localSkills: () => call<string[]>("local_skills"),
  shareSkill: (group: GroupId, skill: string) => call<void>("share_skill", { group, skill }),
  recentCommits: (group: GroupId, limit = 20) => call<CommitInfo[]>("recent_commits", { group, limit }),
  syncNow: () => call<SyncReport>("sync_now"),
  getSettings: () => call<Settings>("get_settings"),
  setSettings: (settings: Partial<Settings>) => call<Settings>("set_settings", { settings }),
  feedbackLogs: () => call<string>("feedback_logs"),
  feedbackCaptureScreen: () => call<Screenshot>("feedback_capture_screen"),
  feedbackAttachFile: (data: string) => call<Screenshot>("feedback_attach_file", { data }),
  feedbackPasteImage: () => call<Screenshot>("feedback_paste_image"),
  feedbackSubmit: (report: FeedbackInput) => call<FeedbackResult>("feedback_submit", { report }),
  checkUpdate: () => call<UpdateInfo>("check_update"),
  updateStatus: () => call<UpdateInfo>("update_status"),
  installUpdate: () => call<void>("install_update"),
  openUrl: (url: string) => call<void>("open_url", { url }),
  openPath: (path: string, reveal = false) => call<void>("open_path", { path, reveal }),
};

export const groupKey = (g: GroupId) => `${g.owner}/${g.repo}`;
