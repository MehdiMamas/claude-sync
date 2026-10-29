// Settings: device name, how often to check, start at login, pause, personal memory,
// notifications, account,
// updates.
import { coverageLines } from "../copy";
import { api, errorText, type Settings } from "../api";
import { busy, errorBox, h, replace, spinner, toast } from "../dom";
import { onRefresh, store } from "../store";
import { forgetHelperState } from "./helper";
import { personalCard } from "./personal";

const POLL: [number, string][] = [
  [30, "Every 30 seconds"],
  [60, "Every minute"],
  [120, "Every 2 minutes"],
  [300, "Every 5 minutes"],
  [900, "Every 15 minutes"],
];

function updateCard(): HTMLElement {
  const line = h("p", { role: "status" });
  const check = h("button", null, "Check for updates");
  const restart = h("button", { class: "primary" }, "Restart to update");
  restart.addEventListener("click", () => busy(restart, "Restarting…", () => api.installUpdate()));
  const show = () => {
    const u = store.update;
    const current = store.info?.version ?? u?.current ?? "?";
    if (u?.available) {
      line.textContent = u.downloaded
        ? `Version ${u.version} is ready. It installs when you quit claude-sync.`
        : `Version ${u.version} is downloading.`;
    } else {
      line.textContent = `You have version ${current}.`;
    }
    restart.hidden = !(u?.available && u.downloaded);
  };
  check.addEventListener("click", () =>
    busy(check, "Checking…", async () => {
      try {
        const u = await api.checkUpdate();
        store.update = u;
        show();
        if (!u.available) toast("claude-sync is up to date");
      } catch (e) {
        line.textContent = errorText(e);
      }
    }),
  );
  show();
  return h(
    "section",
    { class: "card", "aria-labelledby": "update-h" },
    h("h2", { id: "update-h" }, "Updates"),
    line,
    h("div", { class: "actions" }, check, restart),
  );
}

function helperCard(): HTMLElement {
  const status = h("p", { role: "status" }, "Checking…");
  api.helperStatus().then(
    (s) => {
      forgetHelperState();
      status.textContent = s.available ? "Claude Code found. The helper is on." : "Claude Code isn't installed or signed in. Install it and run `claude` once to sign in.";
    },
    (e) => (status.textContent = errorText(e)),
  );
  return h(
    "section",
    { class: "card", "aria-labelledby": "helper-h" },
    h("h2", { id: "helper-h" }, "Claude helper"),
    h(
      "p",
      { class: "muted small" },
      "Uses your Claude Code login on this computer. It adds Explain and Summarise buttons for conflicts, CLAUDE.md changes and group activity. Nothing is sent until you click one. Text with a secret in it is never sent.",
    ),
    status,
  );
}

function githubAppCard(s: Settings, save: (patch: Partial<Settings>, done?: string) => Promise<void>): HTMLElement {
  const missing = store.info?.appClientIdMissing ?? false;
  const toggle = h("input", { id: "s-ghapp", type: "checkbox", checked: s.githubApp && !missing, disabled: missing });
  const install = h("button", { type: "button" }, "Install the App");
  const list = h("ul", { class: "muted small", role: "status" });
  const extra = h("div", null, h("div", { class: "actions" }, install), list);
  const loadCoverage = async () => {
    try {
      const c = await api.appCoverage();
      replace(list, ...coverageLines(c).map((t) => h("li", null, t)));
    } catch (e) {
      replace(list, h("li", null, errorText(e)));
    }
  };
  const show = (on: boolean) => {
    extra.hidden = !on;
    if (on) void loadCoverage();
  };
  install.addEventListener("click", () => busy(install, "Opening…", () => api.openAppInstall(false)));
  toggle.addEventListener("change", async () => {
    await save({ githubApp: toggle.checked });
    show(toggle.checked);
  });
  show(toggle.checked);
  return h(
    "section",
    { class: "card", "aria-labelledby": "ghapp-h" },
    h("h2", { id: "ghapp-h" }, "GitHub App (beta)"),
    h("div", { class: "check" }, toggle, h("label", { for: "s-ghapp" }, "Sign in with the GitHub App (beta)")),
    h(
      "p",
      { class: "muted small" },
      missing
        ? "This build has no GitHub App."
        : "Takes effect at your next sign-in. The App only reaches repos it is installed on.",
    ),
    extra,
  );
}

function form(s: Settings): HTMLElement[] {
  const save = async (patch: Partial<Settings>, done?: string) => {
    try {
      s = await api.setSettings(patch);
      if (done) toast(done);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };
  const personal = personalCard();
  onRefresh(personal.reload);

  const device =h("input", { id: "s-device", value: s.deviceName, maxlength: 64, autocomplete: "off" });
  const saveDevice = h("button", { type: "submit" }, "Save");
  const deviceForm = h(
    "form",
    { class: "form" },
    h("label", { for: "s-device" }, "Device name"),
    h("p", { class: "muted small", id: "s-device-hint" }, "Shown in the group's history next to your changes."),
    h("div", { class: "inline-form" }, device, saveDevice),
  );
  device.setAttribute("aria-describedby", "s-device-hint");
  deviceForm.addEventListener("submit", (ev) => {
    ev.preventDefault();
    busy(saveDevice, "Saving…", async () => {
      await save({ deviceName: device.value }, "Device name saved");
      personal.reload();
    });
  });

  const poll = h(
    "select",
    { id: "s-poll" },
    POLL.map(([v, label]) => h("option", { value: v, selected: v === s.pollSeconds }, label)),
  );
  if (!POLL.some(([v]) => v === s.pollSeconds)) {
    poll.prepend(h("option", { value: s.pollSeconds, selected: true }, `Every ${s.pollSeconds} seconds`));
  }
  poll.addEventListener("change", () => save({ pollSeconds: Number(poll.value) }, "Saved"));

  const autostart = h("input", { id: "s-autostart", type: "checkbox", checked: s.autostart });
  autostart.addEventListener("change", () => save({ autostart: autostart.checked }));

  const paused = h("input", { id: "s-paused", type: "checkbox", checked: s.paused });
  paused.addEventListener("change", () =>
    save({ paused: paused.checked }, paused.checked ? "Sync paused" : "Sync resumed"),
  );

  const desktop = h("input", { id: "s-desktop", type: "checkbox", checked: s.desktopNotifications });
  desktop.addEventListener("change", () => save({ desktopNotifications: desktop.checked }));
  const test = h("button", null, "Send a test notification");
  test.addEventListener("click", () => busy(test, "Sending…", () => api.testNotification()));

  const blocked = h(
    "p",
    { class: "muted small", hidden: true },
    "macOS is blocking notifications from claude-sync. ",
    h("button", { type: "button", onclick: () => api.openNotificationSettings() }, "Open notification settings"),
  );
  void api.notificationPermission().then((p) => (blocked.hidden = p !== "denied"));

  const signOut = h("button", { class: "danger" }, "Sign out");
  signOut.addEventListener("click", () =>
    busy(signOut, "Signing out…", async () => {
      await api.signOut();
      store.set({ auth: { signedIn: false, login: null, expiredLogin: null, clientIdMissing: store.auth?.clientIdMissing ?? false } });
    }),
  );

  return [
    h(
      "section",
      { class: "card", "aria-labelledby": "sync-h" },
      h("h2", { id: "sync-h" }, "Sync"),
      deviceForm,
      h("div", { class: "form" }, h("label", { for: "s-poll" }, "Check for changes"), poll),
      h("div", { class: "check" }, autostart, h("label", { for: "s-autostart" }, "Start claude-sync when I log in")),
      h("div", { class: "check" }, paused, h("label", { for: "s-paused" }, "Pause syncing")),
    ),
    personal.card,
    h(
      "section",
      { class: "card", "aria-labelledby": "notify-h" },
      h("h2", { id: "notify-h" }, "Notifications"),
      h("div", { class: "check" }, desktop, h("label", { for: "s-desktop" }, "Show desktop notifications")),
      h(
        "p",
        { class: "muted small" },
        "Either way, every notification is kept in ",
        h("a", { href: "#/notifications" }, "Notifications"),
        ".",
      ),
      blocked,
      h("div", { class: "actions" }, test),
    ),
    h(
      "section",
      { class: "card", "aria-labelledby": "account-h" },
      h("h2", { id: "account-h" }, "Account"),
      h("p", null, "Signed in to GitHub as ", h("strong", null, store.auth?.login ?? "?"), "."),
      h("p", { class: "muted small" }, "Signing out removes the token from this computer. Your groups stay on GitHub and your files stay where they are."),
      h("div", { class: "actions" }, signOut),
    ),
    githubAppCard(s, save),
    helperCard(),
    updateCard(),
    h(
      "p",
      { class: "muted small" },
      `claude-sync ${store.info?.version ?? ""} · ${store.info?.os ?? ""} · data in `,
      h("code", null, store.info?.stateDir ?? "?"),
    ),
  ];
}

export function settingsScreen(view: HTMLElement): void {
  const body = h("div", null, spinner("Loading settings"));
  replace(view, h("header", { class: "head" }, h("h1", null, "Settings")), body);
  const load = () =>
    api.getSettings().then(
      (s) => replace(body, form(s)),
      (e) => replace(body, errorBox(errorText(e), load)),
    );
  load();
}
