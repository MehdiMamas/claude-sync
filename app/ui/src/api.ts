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
  /** Changes others made to the group's CLAUDE.md that wait for this user. */
  reviewPending: number;
  /** "Review changes others make to CLAUDE.md" (on by default). */
  reviewClaudeMd: boolean;
}
/** One block of lines another member added to or removed from the group's CLAUDE.md. */
export interface ReviewItem {
  id: string;
  kind: "add" | "remove";
  lines: string[];
  /** GitHub login; empty when unknown. */
  author: string;
  commit: string;
  message: string;
  atUnix: number;
}
export interface ClaudeMdReview {
  review: boolean;
  /** Oldest first. */
  pending: ReviewItem[];
  /** How many changes the user declined (never asked again). */
  declined: number;
  /** The user's copy differs from the group's (declined or pending changes). */
  differs: boolean;
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
  /** CLAUDE.md changes from others that this sync found; they wait for review. */
  review: ReviewItem[];
}
export interface SyncReport {
  groups: GroupSyncReport[];
}
/** The user's own CLAUDE.md text to share with a group, as edited in the preview. */
export interface ClaudeMdShare {
  text: string;
  /** Take the shared lines out of the user's own CLAUDE.md (it now comes from the group). */
  removeOwn: boolean;
}
export interface ClaudeMdShared {
  /** False: the group's CLAUDE.md already had the text. */
  appended: boolean;
  removedOwn: boolean;
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
  /** Show desktop notifications. The history is kept either way. */
  desktopNotifications: boolean;
}
export type NotificationKind = "update" | "group" | "plugins" | "local-edit" | "job" | "personal" | "other";
/** One entry of the notification history (crates/core/src/notifications.rs). */
export interface Notification {
  id: number;
  /** UTC seconds since the Unix epoch. */
  time: number;
  title: string;
  body: string;
  kind: NotificationKind;
  group: GroupId | null;
  read: boolean;
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
/** What personal memory syncs. */
export interface Kinds {
  claudeMd: boolean;
  memory: boolean;
}
export interface DeviceStatus {
  id: string;
  name: string;
  os: string;
  lastSeenUnix: number;
  thisDevice: boolean;
}
/** Personal memory (docs/architecture.md): this account's devices share it; groups never see it. */
export interface PersonalStatus {
  enabled: boolean;
  /** The user turned it on or off, or said "Not now", on this device. */
  decided: boolean;
  /** The account's other devices share personal memory and this one was not asked yet. */
  offer: boolean;
  kinds: Kinds;
  /** `owner/repo` */
  repo: string;
  repoUrl: string;
  deviceId: string;
  devices: DeviceStatus[];
  /** null until the repo was read once. */
  newDevicesJoin: boolean | null;
  lastSyncUnix: number | null;
  lastError: string | null;
  conflicts: ConflictInfo[];
  /** Files left out of the last sync and why. */
  skipped: string[];
}
/** `jobs/<id>/last-run.json` (crates/core/src/jobs/run.rs). */
export interface LastRun {
  startUnix: number;
  endUnix: number;
  exitCode: number | null;
  /** `ok`, `failed`, `timed out`, `skipped: offline`, or `error: <why>`. */
  status: string;
  manual: boolean;
  log: string | null;
  /** The job's report, when the run wrote it. */
  report: string | null;
}
export type PermissionMode = "default" | "acceptEdits" | "plan" | "bypassPermissions";
/** One job of the group's schedules.json, as this device sees it. */
export interface JobView {
  id: string;
  title: string;
  cron: string;
  /** `Mon and Thu at 10:17` (local time). */
  schedule: string;
  prompt: string;
  cwd: string;
  permissionMode: PermissionMode;
  timeoutMinutes: number;
  report: string | null;
  /** Extra claude options, after the prompt: --chrome, --model <id>, --effort <level>. */
  args: string[];
  /** What "Accept" sends back: the consent hash of the version shown. */
  hash: string;
  /** Accepted in this version. */
  accepted: boolean;
  /** Accepted before, but the job changed since. */
  changed: boolean;
  enabled: boolean;
  registered: boolean;
  error: string | null;
  lastRun: LastRun | null;
  running: boolean;
  hasLog: boolean;
  hasReport: boolean;
}
/** A job of schedules.json that was skipped (bad id, cron, path, ...). */
export interface JobProblem {
  group: GroupId;
  id: string | null;
  message: string;
}
export interface JobsOverview {
  /** Null, or why this OS cannot run jobs. */
  unsupported: string | null;
  jobs: JobView[];
  problems: JobProblem[];
  /** "Run this group's jobs without asking" is on for this device. */
  autoAccept: boolean;
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
  "notifications-changed": { unread: number };
  navigate: string;
  personal: PersonalStatus;
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
  createGroup: (name: string, repo?: string, claudeMd?: ClaudeMdShare) => call<GroupStatus>("create_group", { name, repo, claudeMd }),
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
  claudeMdPreview: () => call<string>("claude_md_preview"),
  shareClaudeMd: (group: GroupId, share: ClaudeMdShare) => call<ClaudeMdShared>("share_claude_md", { group, share }),
  recentCommits: (group: GroupId, limit = 20) => call<CommitInfo[]>("recent_commits", { group, limit }),
  claudeMdReview: (group: GroupId) => call<ClaudeMdReview>("claude_md_review", { group }),
  /** Accept or decline `ids`, or every pending change when `ids` is null. */
  decideClaudeMd: (group: GroupId, ids: string[] | null, accept: boolean) =>
    call<ClaudeMdReview>("decide_claude_md", { group, ids, accept }),
  setClaudeMdReview: (group: GroupId, on: boolean) => call<ClaudeMdReview>("set_claude_md_review", { group, on }),
  useGroupClaudeMd: (group: GroupId) => call<ClaudeMdReview>("use_group_claude_md", { group }),
  syncNow: () => call<SyncReport>("sync_now"),
  getSettings: () => call<Settings>("get_settings"),
  setSettings: (settings: Partial<Settings>) => call<Settings>("set_settings", { settings }),
  notifications: () => call<Notification[]>("notifications"),
  markNotificationRead: (id: number) => call<void>("mark_notification_read", { id }),
  markAllNotificationsRead: () => call<void>("mark_all_notifications_read"),
  clearNotifications: () => call<void>("clear_notifications"),
  testNotification: () => call<void>("test_notification"),
  feedbackLogs: () => call<string>("feedback_logs"),
  feedbackCaptureScreen: () => call<Screenshot>("feedback_capture_screen"),
  feedbackAttachFile: (data: string) => call<Screenshot>("feedback_attach_file", { data }),
  feedbackPasteImage: () => call<Screenshot>("feedback_paste_image"),
  feedbackSubmit: (report: FeedbackInput) => call<FeedbackResult>("feedback_submit", { report }),
  checkUpdate: () => call<UpdateInfo>("check_update"),
  updateStatus: () => call<UpdateInfo>("update_status"),
  installUpdate: () => call<void>("install_update"),
  personalStatus: () => call<PersonalStatus>("personal_status"),
  personalEnable: () => call<PersonalStatus>("personal_enable"),
  personalDisable: () => call<PersonalStatus>("personal_disable"),
  personalSetKinds: (kinds: Kinds) => call<PersonalStatus>("personal_set_kinds", { kinds }),
  personalSetNewDevicesJoin: (join: boolean) => call<PersonalStatus>("personal_set_new_devices_join", { join }),
  personalRemoveDevice: (id: string) => call<PersonalStatus>("personal_remove_device", { id }),
  personalDismissOffer: () => call<void>("personal_dismiss_offer"),
  openUrl: (url: string) => call<void>("open_url", { url }),
  openPath: (path: string, reveal = false) => call<void>("open_path", { path, reveal }),
  jobs: (group: GroupId) => call<JobsOverview>("jobs", { group }),
  acceptJob: (group: GroupId, id: string, hash: string) => call<JobsOverview>("accept_job", { group, id, hash }),
  setJobEnabled: (group: GroupId, id: string, enabled: boolean) => call<JobsOverview>("set_job_enabled", { group, id, enabled }),
  setJobsAutoAccept: (group: GroupId, on: boolean) => call<JobsOverview>("set_jobs_auto_accept", { group, on }),
  runJobNow: (group: GroupId, id: string) => call<JobsOverview>("run_job_now", { group, id }),
  openJobFile: (group: GroupId, id: string, kind: "log" | "report") => call<void>("open_job_file", { group, id, kind }),
  pluginWarnings: (group: GroupId) => call<string[]>("plugin_warnings", { group }),
  retryPlugins: (group: GroupId) => call<string[]>("retry_plugins", { group }),
};

export const groupKey = (g: GroupId) => `${g.owner}/${g.repo}`;
