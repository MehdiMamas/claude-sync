import { type ReactNode, useEffect, useState } from "react";
import pkg from "../../package.json";
import type { ClashView } from "../generated/ClashView";
import type { ConflictView } from "../generated/ConflictView";
import type { PackView } from "../generated/PackView";
import type { Snapshot } from "../generated/Snapshot";
import { useBackend } from "../store";
import { ago } from "../time";
import { Banner, Chip, go, Spin, useLoad } from "../ui";
import type { Call } from "./Group";
import { data } from "./Group";
import { Toggle } from "./GroupMembers";
import { initials } from "./Shell";

export type SettingsCtx = { snap: Snapshot; now: number; call: Call; toast: (t: string) => void };

const tabs = [
  ["", "General"],
  ["personal", "Personal"],
  ["notifications", "Notifications"],
  ["account", "Account"],
  ["updates", "Updates"],
  ["advanced", "Advanced"],
] as const;
const intervals = [
  [30, "Every 30 seconds"],
  [60, "Every minute"],
  [120, "Every 2 minutes"],
  [300, "Every 5 minutes"],
  [900, "Every 15 minutes"],
] as const;
const RELEASES = "https://github.com/MehdiMamas/claude-sync/releases/latest";
const settingsUrl = /Mac/.test(navigator.platform)
  ? "x-apple.systempreferences:com.apple.Notifications-Settings.extension"
  : "ms-settings:notifications";
const lower = (s: string) => s.toLowerCase();

const Frame = ({ tab, sub, children }: { tab: string; sub?: boolean; children?: ReactNode }) => (
  <>
    <h1>Settings</h1>
    {sub !== false && <p className="sub">These choices follow you to all your machines.</p>}
    <div className="tabs">
      {tabs.map(([id, label]) => (
        <a key={id} href={`#/settings${id && `/${id}`}`} className={id === tab ? "on" : ""}>
          {label}
        </a>
      ))}
    </div>
    {children}
  </>
);
const Row = ({ children, act }: { children: ReactNode; act?: ReactNode }) => (
  <div className="row">
    <div>{children}</div>
    <div className="acts">{act}</div>
  </div>
);
const Page = ({ title, sub, children }: { title: string; sub: string; children: ReactNode }) => (
  <>
    <h1>{title}</h1>
    <p className="sub">{sub}</p>
    {children}
  </>
);
const Back = () => (
  <a className="b g" href="#/settings/advanced">
    Back
  </a>
);

/** Runs a command and shows its refusal as a toast. */
const useRun = (c: SettingsCtx) => async (cmd: Parameters<Call>[0]) => {
  const r = await c.call(cmd);
  if ("err" in r) c.toast(r.err);
  return r;
};

/** v2-ScreenSettings and Paused: start at login, the interval, pause, this machine's name. */
function General(c: SettingsCtx) {
  const { snap } = c;
  const run = useRun(c);
  const me = snap.personal.devices.find((d) => d.this);
  return (
    <Frame tab="">
      {snap.paused && (
        <Banner
          kind="warn"
          actions={
            <button type="button" className="b" onClick={() => void run({ kind: "resume" })}>
              Resume
            </button>
          }
        >
          Syncing is paused on all your machines.
        </Banner>
      )}
      <div className="card">
        <Row
          act={
            <Toggle
              on={snap.startAtLogin === true}
              label={snap.startAtLogin ? "Turn off" : "Turn on"}
              set={() => void run({ kind: "startAtLogin", data: { on: !snap.startAtLogin } })}
            />
          }
        >
          Start claude-sync when I log in
        </Row>
        {!snap.paused && (
          <>
            <Row
              act={
                <select
                  className="in"
                  aria-label="Check for changes"
                  style={{ width: 150 }}
                  value={snap.intervalSecs}
                  onChange={(e) =>
                    void run({ kind: "setInterval", data: { secs: Number(e.target.value) } })
                  }
                >
                  {intervals.map(([s, l]) => (
                    <option key={s} value={s}>
                      {l}
                    </option>
                  ))}
                </select>
              }
            >
              Check for changes
            </Row>
            <div className="sm muted">Every 30 seconds, minute, 2, 5 or 15 minutes.</div>
          </>
        )}
        <Row
          act={
            <Toggle
              on={snap.paused}
              label={snap.paused ? "Turn off" : "Turn on"}
              set={() => void run({ kind: snap.paused ? "resume" : "pause" })}
            />
          }
        >
          Pause syncing
        </Row>
        {!snap.paused && me && (
          <Row
            act={
              <input
                className="in"
                aria-label="This machine's name"
                style={{ width: 160 }}
                key={me.name}
                defaultValue={me.name}
                onBlur={(e) => {
                  const name = e.target.value.trim();
                  if (name && name !== me.name)
                    void run({ kind: "deviceRename", data: { id: me.id, name } });
                }}
              />
            }
          >
            This machine's name
          </Row>
        )}
      </div>
    </Frame>
  );
}

/** v2-ScreenSettingsOff, Offer, Personal, PersonalPublic and DeviceRemove. */
function Personal(c: SettingsCtx & { sub: string[] }) {
  const { snap, now } = c;
  const p = snap.personal;
  const run = useRun(c);
  const [conflicts] = useLoad(
    () => data(c.call, { kind: "conflicts" }, "conflicts") as Promise<ConflictView[]>,
    String(p.on),
  );
  const [, , id] = c.sub;
  const doomed = p.devices.find((d) => d.id === id);
  const kinds = [
    ["claudeMd", "CLAUDE.md"],
    ["memory", "Memory"],
    ["state", "Mod state"],
    ["settings", "Settings"],
  ] as const;
  const mine = conflicts ?? [];
  return (
    <Frame tab="personal">
      {doomed ? (
        <div className="dlg" style={{ position: "fixed" }}>
          <div className="card" role="dialog" aria-label={`Remove ${doomed.name}?`}>
            <h2>Remove {doomed.name}?</h2>
            <p className="muted">
              {doomed.name} stops getting your personal memory and its SSH key leaves your other
              machines. Its files stay on it.
            </p>
            <div className="acts">
              <button
                type="button"
                className="b dp"
                onClick={async () => {
                  const r = await run({ kind: "deviceRemove", data: { id: doomed.id } });
                  if ("ev" in r) go("settings/personal");
                }}
              >
                Remove
              </button>
              <a className="b" href="#/settings/personal">
                Cancel
              </a>
            </div>
          </div>
        </div>
      ) : (
        <>
          {p.offer && !p.on && (
            <Banner
              kind="info"
              actions={
                <>
                  <button
                    type="button"
                    className="b p"
                    onClick={() => void run({ kind: "personalOn" })}
                  >
                    Use it
                  </button>
                  <button
                    type="button"
                    className="b"
                    onClick={() => void run({ kind: "personalDismiss" })}
                  >
                    Not now
                  </button>
                </>
              }
            >
              Found your private repo {p.repo} from another machine.
            </Banner>
          )}
          {p.error && (
            <Banner
              kind="err"
              actions={
                <button type="button" className="b" onClick={() => void run({ kind: "syncNow" })}>
                  Check again
                </button>
              }
            >
              {p.error}
            </Banner>
          )}
          <div className="card">
            <h2>Personal memory</h2>
            <Row
              act={
                <Toggle
                  on={p.on}
                  label={p.on ? "Turn off" : "Turn on"}
                  set={() => void run({ kind: p.on ? "personalOff" : "personalOn" })}
                />
              }
            >
              {p.on ? (
                <>
                  Your own CLAUDE.md, rules, memory, settings and skills, through your private repo{" "}
                  <span className="mono">{p.repo}</span>
                </>
              ) : p.offer || p.error ? (
                "Keep your own setup the same on all your machines"
              ) : (
                "Keep your own setup the same on all your machines. Makes a private repo the first time."
              )}
            </Row>
            {p.on && !p.error && (
              <Row
                act={
                  <button type="button" className="b" onClick={() => void run({ kind: "syncNow" })}>
                    Sync now
                  </button>
                }
              >
                {p.lastSync === null ? (
                  <Chip kind="n">Not synced yet</Chip>
                ) : (
                  <>
                    <Chip kind="ok">Synced</Chip> {ago(p.lastSync, now)}
                  </>
                )}
              </Row>
            )}
          </div>
          {p.on && (
            <>
              <div className="card">
                <h2>What syncs</h2>
                {kinds.map(([k, label]) => {
                  const on = !p.kindsOff.includes(k);
                  return (
                    <Row
                      key={k}
                      act={
                        <Toggle
                          on={on}
                          label={`${label}: ${on ? "turn off" : "turn on"}`}
                          set={() => void run({ kind: "personalKind", data: { kind: k, on: !on } })}
                        />
                      }
                    >
                      {label}
                    </Row>
                  );
                })}
                <Row
                  act={
                    <Toggle
                      on={p.newDevicesJoin}
                      label={`New devices join automatically: ${p.newDevicesJoin ? "turn off" : "turn on"}`}
                      set={() =>
                        void run({ kind: "personalJoinAuto", data: { on: !p.newDevicesJoin } })
                      }
                    />
                  }
                >
                  New devices join automatically
                </Row>
              </div>
              <div className="card">
                <h2>Your machines</h2>
                {p.devices.map((d) => (
                  <Row
                    key={d.id}
                    act={
                      d.this ? (
                        <Rename id={d.id} name={d.name} run={run} />
                      ) : (
                        <a className="b d" href={`#/settings/personal/remove/${d.id}`}>
                          Remove
                        </a>
                      )
                    }
                  >
                    <b>{d.name}</b> {d.this ? "(this one) · " : "· "}
                    {d.os}
                    {d.this ? "" : ` · seen ${ago(d.lastSeen, now)}`}
                  </Row>
                ))}
              </div>
              {(p.stays.length > 0 || mine.length > 0) && (
                <div className="card">
                  <h2>Stays on this device</h2>
                  {p.stays.map((s) => {
                    const [path = "", ...why] = s.split(": ");
                    return (
                      <Row key={s}>
                        <span className="mono">{path}</span>
                        <div className="sm muted">{why.join(": ")}</div>
                      </Row>
                    );
                  })}
                  {mine.length > 0 && (
                    <Row
                      act={
                        <a className="b" href="#/settings/advanced/conflicts">
                          Open
                        </a>
                      }
                    >
                      Conflicts: {mine.length} file{mine.length === 1 ? "" : "s"} changed on two
                      devices
                    </Row>
                  )}
                </div>
              )}
            </>
          )}
        </>
      )}
    </Frame>
  );
}

function Rename(p: { id: string; name: string; run: ReturnType<typeof useRun> }) {
  const [editing, setEditing] = useState(false);
  if (!editing)
    return (
      <button type="button" className="b g" onClick={() => setEditing(true)}>
        Rename
      </button>
    );
  return (
    <input
      className="in"
      aria-label="New name"
      style={{ width: 140 }}
      defaultValue={p.name}
      // biome-ignore lint/a11y/noAutofocus: the person just pressed Rename
      autoFocus
      onBlur={(e) => {
        const name = e.target.value.trim();
        setEditing(false);
        if (name && name !== p.name) void p.run({ kind: "deviceRename", data: { id: p.id, name } });
      }}
      onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
    />
  );
}

/** v2-ScreenSettingsNotifications and NotifyBlocked. Test uses the system's own permission. */
function Notifications(c: SettingsCtx) {
  const { snap } = c;
  const run = useRun(c);
  const backend = useBackend();
  const [blocked, setBlocked] = useState(false);
  const os = /Mac/.test(navigator.platform)
    ? "macOS"
    : /Win/.test(navigator.platform)
      ? "Windows"
      : "Your system";
  const test = async () => {
    await run({ kind: "testNotification" });
    setBlocked((await backend.notifyPermission()) === "denied");
  };
  return (
    <Frame tab="notifications">
      {blocked && (
        <Banner
          kind="warn"
          actions={
            <button
              type="button"
              className="b"
              onClick={() => void backend.openTarget(settingsUrl)}
            >
              Open System Settings
            </button>
          }
        >
          {os} blocks notifications from claude-sync. Open System Settings, Notifications,
          claude-sync, and allow them.
        </Banner>
      )}
      <div className="card">
        <Row
          act={
            <Toggle
              on={snap.desktopNotifications}
              label={snap.desktopNotifications ? "Turn off" : "Turn on"}
              set={() =>
                void run({ kind: "desktopNotifications", data: { on: !snap.desktopNotifications } })
              }
            />
          }
        >
          Desktop notifications
        </Row>
        {!blocked && (
          <Row
            act={
              <button type="button" className="b" onClick={() => void test()}>
                Test
              </button>
            }
          >
            Send a test notification
          </Row>
        )}
      </div>
    </Frame>
  );
}

/** v2-ScreenSettingsAccount, AccountApp and SignOut. */
function Account(c: SettingsCtx & { sub: string[] }) {
  const { snap } = c;
  const run = useRun(c);
  const backend = useBackend();
  const login = snap.login ?? "";
  const [, mode] = c.sub;
  const app = snap.appMode;
  const open = (u: string) => void backend.openTarget(u);
  // which groups the App cannot reach: read when the screen opens in App mode
  useEffect(() => {
    if (app) void c.call({ kind: "appCoverage" });
  }, [app, c.call]);
  if (mode === "signout")
    return (
      <Frame tab="account" sub={false}>
        <div className="dlg" style={{ position: "fixed" }}>
          <div className="card" role="dialog" aria-label="Sign out of claude-sync?">
            <h2>Sign out of claude-sync?</h2>
            <p className="muted">Syncing stops on this machine. Your files stay.</p>
            <div className="acts">
              <button type="button" className="b dp" onClick={() => void run({ kind: "signOut" })}>
                Sign out
              </button>
              <a className="b" href="#/settings/account">
                Cancel
              </a>
            </div>
          </div>
        </div>
      </Frame>
    );
  const who = (
    <>
      <span className="av">{initials(login)}</span>
      {app ? " Signed in through the app as " : " Signed in as "}
      <b>{login}</b>
    </>
  );
  return (
    <Frame tab="account">
      <div className="card">
        <h2>{app ? "GitHub App (beta)" : "GitHub"}</h2>
        <Row
          act={
            app ? (
              <Toggle
                on
                label="Turn off"
                set={() => void run({ kind: "signInStart", data: { app: false } })}
              />
            ) : (
              <a className="b d" href="#/settings/account/signout">
                Sign out
              </a>
            )
          }
        >
          {who}
        </Row>
        {!app && (
          <Row
            act={
              <Toggle
                on={false}
                label="Turn on"
                set={() => void run({ kind: "signInStart", data: { app: true } })}
              />
            }
          >
            Sign in with the GitHub App (beta): access only to repos you pick
          </Row>
        )}
        {app &&
          snap.groups.map((g) => {
            const bad = snap.appUncovered.includes(g.repo);
            return (
              <Row
                key={g.repo}
                act={
                  bad && (
                    <button
                      type="button"
                      className="b p"
                      onClick={() => open("https://github.com/apps/claude-sync/installations/new")}
                    >
                      Install the App
                    </button>
                  )
                }
              >
                <b>{g.repo}</b>{" "}
                <Chip kind={bad ? "warn" : "ok"}>{bad ? "Not covered" : "Covered"}</Chip>
                {bad && (
                  <div className="sm muted">
                    This group does not sync until the app is installed on it.
                  </div>
                )}
              </Row>
            );
          })}
        {app && (
          <Row
            act={
              <button
                type="button"
                className="b"
                onClick={() => open("https://github.com/apps/claude-sync/installations/new")}
              >
                Open
              </button>
            }
          >
            Install on more repos
            <div className="sm muted">Opens the app install page on github.com.</div>
          </Row>
        )}
        {!app && (
          <Row
            act={
              <button
                type="button"
                className="b"
                onClick={() => open("https://github.com/settings/connections/applications")}
              >
                Open
              </button>
            }
          >
            Manage app access
            <div className="sm muted">
              Opens github.com/settings/connections/applications in the browser, where you can
              revoke claude-sync.
            </div>
          </Row>
        )}
      </div>
    </Frame>
  );
}

/** v2-ScreenSettingsUpdates, UpdateReady, UpdateError and Updated, from the snapshot `update`. */
function Updates(c: SettingsCtx) {
  const { snap, now } = c;
  const run = useRun(c);
  const backend = useBackend();
  const [kind = "", v = ""] = snap.update.split(":");
  const here = pkg.version;
  const check = (
    <button type="button" className="b" onClick={() => void run({ kind: "checkUpdate" })}>
      {kind === "failed" ? "Check again" : "Check now"}
    </button>
  );
  const status =
    kind === "ready"
      ? `Version ${here} · ${v} downloaded and verified`
      : kind === "failed"
        ? `Version ${here}`
        : kind === "available"
          ? `Version ${here} · ${v} is available`
          : `Version ${kind === "updated" ? v : here} · up to date${
              snap.updateChecked ? ` · checked ${ago(snap.updateChecked, now)}` : ""
            }`;
  return (
    <Frame tab="updates">
      {kind === "ready" && (
        <Banner
          kind="info"
          actions={
            // Installing is the shell's job (plan step 9): no Command exists for it yet.
            <button
              type="button"
              className="b p"
              onClick={() => c.toast("Installing from the window comes with the update step.")}
            >
              Restart to update
            </button>
          }
        >
          Version {v} is ready. It installs when the app is idle, or now.
        </Banner>
      )}
      {kind === "available" && (
        <Banner kind="info">Version {v} is available. It downloads in the background.</Banner>
      )}
      {kind === "failed" && (
        <Banner
          kind="err"
          actions={
            <>
              {check}
              <a className="b" href="#/feedback">
                Report
              </a>
            </>
          }
        >
          The download for {v} failed its signature check and was thrown away.
        </Banner>
      )}
      {kind === "updated" && (
        <Banner
          kind="ok"
          actions={
            <button type="button" className="b" onClick={() => void backend.openTarget(RELEASES)}>
              What changed
            </button>
          }
        >
          Updated to {v}. Groups, jobs and sign-in carried over.
        </Banner>
      )}
      <div className="card">
        <Row act={kind === "failed" || kind === "ready" ? undefined : check}>{status}</Row>
      </div>
    </Frame>
  );
}

const OS_NAME: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };

/** v2-ScreenSettingsAdvanced. */
function Advanced(c: SettingsCtx) {
  const { snap } = c;
  const run = useRun(c);
  const [conflicts] = useLoad(
    () => data(c.call, { kind: "conflicts" }, "conflicts") as Promise<ConflictView[]>,
    "advanced",
  );
  const n = conflicts?.length ?? 0;
  return (
    <Frame tab="advanced">
      <div className="card">
        <Row
          act={
            <Toggle
              on={snap.helper}
              label={snap.helper ? "Turn off" : "Turn on"}
              set={() => void run({ kind: "helper", data: { on: !snap.helper } })}
            />
          }
        >
          Claude helper: explain group changes with your Claude login
        </Row>
        <Row
          act={
            <a className="b" href="#/settings/advanced/packs">
              Open
            </a>
          }
        >
          Opt-in packs
        </Row>
        <Row
          act={
            <a className="b" href="#/settings/advanced/ssh">
              Open
            </a>
          }
        >
          SSH between your machines
        </Row>
        <Row
          act={
            <Toggle
              on={snap.activityOn}
              label={snap.activityOn ? "Turn off" : "Turn on"}
              set={() => void run({ kind: "activitySet", data: { on: !snap.activityOn } })}
            />
          }
        >
          Share when I am at the computer
        </Row>
        <Row
          act={
            <a className="b" href="#/settings/advanced/conflicts">
              Open{n > 0 ? ` (${n})` : ""}
            </a>
          }
        >
          Conflicts saved by claude-sync
        </Row>
      </div>
    </Frame>
  );
}

/** v2-ScreenSettingsPacks: every group's packs with the switch and Try again. */
function Packs(c: SettingsCtx) {
  const run = useRun(c);
  const [packs, reload] = useLoad(
    () => data(c.call, { kind: "packs", data: { repo: null } }, "packs") as Promise<PackView[]>,
    "packs",
  );
  const act = async (cmd: Parameters<Call>[0]) => {
    await run(cmd);
    reload();
  };
  const repos = [...new Set((packs ?? []).map((p) => p.repo))];
  return (
    <Page
      title="Opt-in packs"
      sub="Extra setups a group offers. Each runs only after you turn it on."
    >
      {!packs && (
        <p className="muted">
          <Spin />
          Loading…
        </p>
      )}
      {packs && packs.length === 0 && <p className="muted">No group offers a pack.</p>}
      {repos.map((repo) => (
        <div className="card" key={repo}>
          <h2>{repo}</h2>
          {packs
            ?.filter((p) => p.repo === repo)
            .map((p) => {
              const on = p.state === "on";
              const failed = p.state === "failed";
              const chip = p.unsupported ? (
                <Chip kind="n">Needs {p.os.map((o) => OS_NAME[o] ?? o).join(" or ")}</Chip>
              ) : failed ? (
                <Chip kind="err">Setup failed</Chip>
              ) : on ? (
                <Chip kind="ok">On</Chip>
              ) : p.state === "ready" ? (
                <Chip>Ready</Chip>
              ) : null;
              return (
                <Row
                  key={p.id}
                  act={
                    p.unsupported ? undefined : failed ? (
                      <button
                        type="button"
                        className="b"
                        onClick={() => void act({ kind: "packSetup", data: { repo, id: p.id } })}
                      >
                        Try again
                      </button>
                    ) : (
                      <Toggle
                        on={on}
                        label={`${p.title}: ${on ? "turn off" : "turn on"}`}
                        set={() =>
                          void act({
                            kind: on ? "packOff" : "packOn",
                            data: { repo, id: p.id },
                          })
                        }
                      />
                    )
                  }
                >
                  <b>{p.title}</b> {chip}
                  <div className="sm muted">
                    {p.unsupported ? "not for this machine" : p.tail || p.summary}
                    {p.checkedAt ? ` · checked ${new Date(p.checkedAt * 1000).toLocaleString()}` : ""}
                  </div>
                </Row>
              );
            })}
        </div>
      ))}
      <Back />
    </Page>
  );
}

/** v2-ScreenSettingsSsh. The other machines come from the personal device list. */
function Ssh(c: SettingsCtx) {
  const { snap } = c;
  const run = useRun(c);
  const me = snap.personal.devices.find((d) => d.this);
  const others = snap.personal.devices.filter((d) => !d.this);
  return (
    <Page
      title="SSH between your machines"
      sub="Adds your other machines to ~/.ssh so ssh desktop just works. Only the marked block is changed."
    >
      <div className="card">
        <Row
          act={
            <Toggle
              on={snap.sshOn}
              label={snap.sshOn ? "Turn off" : "Turn on"}
              set={() => void run({ kind: "sshSet", data: { on: !snap.sshOn } })}
            />
          }
        >
          Turn on for this machine
        </Row>
        {snap.sshOn &&
          others.map((d) => (
            <Row key={d.id}>
              <b>{lower(d.name)}</b> · Tailscale, LAN fallback
            </Row>
          ))}
        {snap.sshOn && (
          <Row
            act={
              <input
                className="in"
                aria-label="This machine's alias"
                style={{ width: 140 }}
                defaultValue={lower(me?.name ?? "")}
                onBlur={(e) => {
                  const alias = e.target.value.trim();
                  if (alias) void run({ kind: "sshAlias", data: { alias } });
                }}
              />
            }
          >
            This machine's alias
          </Row>
        )}
      </div>
      <Back />
    </Page>
  );
}

/** v2-ScreenSettingsConflicts. */
function Conflicts(c: SettingsCtx) {
  const backend = useBackend();
  const [list, reload] = useLoad(
    () => data(c.call, { kind: "conflicts" }, "conflicts") as Promise<ConflictView[]>,
    "conflicts",
  );
  const [clashes] = useLoad(
    () => data(c.call, { kind: "clashes" }, "clashes") as Promise<ClashView[]>,
    "conflicts-settings",
  );
  const same = (clashes ?? []).filter((x) => x.kind === "setting");
  const act = async (x: ConflictView, kind: "conflictOpen" | "conflictDelete") => {
    const r = await c.call({ kind, data: { repo: x.repo, path: x.path, saved_at: x.savedAt } });
    if ("err" in r) c.toast(r.err);
    else if (kind === "conflictDelete") reload();
    else if (r.ev.kind === "path" && r.ev.data) await backend.openTarget(r.ev.data);
    else c.toast("That copy is gone.");
  };
  return (
    <Page
      title="Conflicts"
      sub="When a file changed on both sides, the group version wins and your copy is saved here."
    >
      <div className="card">
        {list?.length === 0 && same.length === 0 && <p className="muted">No conflicts saved.</p>}
        {same.map((x) => (
          <Row key={`${x.target}|${x.b.source}`}>
            <span className="mono">{`${x.target}: set by group ${x.a.name}; not applied`}</span>
            <div className="sm muted">
              {x.b.name} sets {x.b.text}; {x.a.name} sets {x.a.text} and comes first.
            </div>
          </Row>
        ))}
        {list?.map((x) => (
          <Row
            key={`${x.repo}/${x.path}`}
            act={
              <>
                <button type="button" className="b g" onClick={() => void act(x, "conflictOpen")}>
                  Open copy
                </button>
                {x.repo.includes("/") && (
                  <a className="b" href={`#/group/${x.repo}/share/${x.path}`}>
                    Share mine
                  </a>
                )}
                <button type="button" className="b d" onClick={() => void act(x, "conflictDelete")}>
                  Delete copy
                </button>
              </>
            }
          >
            <span className="mono">{x.path}</span>
            <div className="sm muted">
              {x.repo.includes("/") ? x.repo : "Personal memory"} · {ago(x.savedAt, c.now)}
            </div>
          </Row>
        ))}
      </div>
      <Back />
    </Page>
  );
}

/** The Settings window: the route after `settings/` picks the tab or page. */
export function Settings(c: SettingsCtx & { route: string }) {
  const sub = c.route.split("/").slice(1);
  const pages: Record<string, () => ReactNode> = {
    "": () => <General {...c} />,
    personal: () => <Personal {...c} sub={sub} />,
    notifications: () => <Notifications {...c} />,
    account: () => <Account {...c} sub={sub} />,
    updates: () => <Updates {...c} />,
    advanced: () =>
      sub[1] === "packs" ? (
        <Packs {...c} />
      ) : sub[1] === "ssh" ? (
        <Ssh {...c} />
      ) : sub[1] === "conflicts" ? (
        <Conflicts {...c} />
      ) : (
        <Advanced {...c} />
      ),
  };
  return (pages[sub[0] ?? ""] ?? pages[""])?.() ?? null;
}
