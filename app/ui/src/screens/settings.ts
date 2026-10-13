// Settings: device name, how often to check, start at login, pause, account, updates.
import { api, errorText, type Settings } from "../api";
import { busy, errorBox, h, replace, spinner, toast } from "../dom";
import { store } from "../store";

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

function form(s: Settings): HTMLElement[] {
  const save = async (patch: Partial<Settings>, done?: string) => {
    try {
      s = await api.setSettings(patch);
      if (done) toast(done);
    } catch (e) {
      toast(errorText(e), "error");
    }
  };

  const device = h("input", { id: "s-device", value: s.deviceName, maxlength: 64, autocomplete: "off" });
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
    busy(saveDevice, "Saving…", () => save({ deviceName: device.value }, "Device name saved"));
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

  const signOut = h("button", { class: "danger" }, "Sign out");
  signOut.addEventListener("click", () =>
    busy(signOut, "Signing out…", async () => {
      await api.signOut();
      store.set({ auth: { signedIn: false, login: null, clientIdMissing: store.auth?.clientIdMissing ?? false } });
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
    h(
      "section",
      { class: "card", "aria-labelledby": "account-h" },
      h("h2", { id: "account-h" }, "Account"),
      h("p", null, "Signed in to GitHub as ", h("strong", null, store.auth?.login ?? "?"), "."),
      h("p", { class: "muted small" }, "Signing out removes the token from this computer. Your groups stay on GitHub and the files stay on this device."),
      h("div", { class: "actions" }, signOut),
    ),
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
