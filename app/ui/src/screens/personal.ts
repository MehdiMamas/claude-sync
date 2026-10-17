// Settings → Personal memory: the user's own CLAUDE.md text and Claude's memories, shared
// between this account's devices through a private repo. Groups never see it.
import { api, errorText, type Kinds, type PersonalStatus } from "../api";
import { busy, confirmButton, errorBox, h, relTime, replace, spinner, toast } from "../dom";
import { store } from "../store";

const OS: Record<string, string> = { windows: "Windows", macos: "macOS", linux: "Linux" };

function check(id: string, label: string, checked: boolean, onchange: (box: HTMLInputElement) => Promise<void>, hint?: string): HTMLElement {
  const box = h("input", { id, type: "checkbox", checked });
  box.addEventListener("change", async () => {
    box.disabled = true;
    try {
      await onchange(box);
    } catch (e) {
      box.checked = !box.checked;
      toast(errorText(e), "error");
    } finally {
      box.disabled = false;
    }
  });
  const hintId = hint ? `${id}-hint` : undefined;
  if (hintId) box.setAttribute("aria-describedby", hintId);
  return h(
    "div",
    { class: "check" },
    box,
    h("label", { for: id }, label, hint && h("span", { class: "muted small", id: hintId }, ` ${hint}`)),
  );
}

function devices(st: PersonalStatus, show: (st: PersonalStatus) => void): HTMLElement {
  return h(
    "ul",
    { class: "list flat", "aria-label": "Your devices" },
    st.devices.map((d) => {
      const remove =
        !d.thisDevice &&
        confirmButton("Remove", `Remove ${d.name}?`, (btn) =>
          busy(btn, "Removing…", async () => {
            show(await api.personalRemoveDevice(d.id));
            toast(`Removed ${d.name}`);
          }),
        );
      if (remove) remove.setAttribute("aria-label", `Remove ${d.name}`);
      const seen = d.thisDevice ? "Rename it with Device name above" : `Last seen ${relTime(d.lastSeenUnix)}`;
      return h(
        "li",
        { class: "row" },
        h(
          "div",
          { class: "grow" },
          d.name,
          d.thisDevice && h("span", { class: "tag" }, "this device"),
          h("div", { class: "muted small" }, `${OS[d.os] ?? d.os} · ${seen}`),
        ),
        remove,
      );
    }),
  );
}

function conflicts(st: PersonalStatus): HTMLElement[] {
  if (st.conflicts.length === 0) return [];
  return [
    h("h3", null, "To review"),
    h("p", { class: "muted small" }, "The same file changed on two of your devices. The other device's version is in place; this device's version was saved next to it."),
    h(
      "ul",
      { class: "list flat", "aria-label": "Conflicts" },
      st.conflicts.map((c) => {
        const open = (label: string, reveal: boolean) => {
          const b = h("button", null, label);
          b.addEventListener("click", () => api.openPath(c.savedTo, reveal).catch((e) => toast(errorText(e), "error")));
          return b;
        };
        return h(
          "li",
          { class: "row" },
          h("div", { class: "grow" }, h("code", null, c.path), h("div", { class: "muted small" }, relTime(c.atUnix))),
          c.savedTo && h("div", { class: "actions" }, open("Open saved copy", false), open("Show in folder", true)),
        );
      }),
    ),
  ];
}

function body(st: PersonalStatus, show: (st: PersonalStatus) => void): (HTMLElement | false | null)[] {
  store.set({ personal: st });
  const master = check("p-enabled", "Sync my personal memory between my devices", st.enabled, async (box) => {
    const next = box.checked ? await api.personalEnable() : await api.personalDisable();
    show(next);
    toast(next.enabled ? "Personal memory is on" : "Personal memory is off on this device");
  });
  const intro = h(
    "p",
    { class: "muted small" },
    "Your own CLAUDE.md text and Claude's memories, shared with your other devices through ",
    h("code", null, st.repo),
    ", a private repo on your GitHub account. Groups never see it. Turning it off leaves your files and the repo as they are.",
  );
  if (!st.enabled) {
    return [
      intro,
      st.offer && h("p", null, "Your other devices already share their personal memory. Turn it on to merge it with this device's."),
      master,
    ];
  }
  const setKinds = async (patch: Partial<Kinds>) => show(await api.personalSetKinds({ ...st.kinds, ...patch }));
  const repo = h("button", null, "Open repo");
  repo.addEventListener("click", () => api.openUrl(st.repoUrl).catch((e) => toast(errorText(e), "error")));
  return [
    intro,
    master,
    check("p-claude-md", "My CLAUDE.md", st.kinds.claudeMd, (b) => setKinds({ claudeMd: b.checked }), "(without the groups' part)"),
    check("p-memory", "Claude's memories", st.kinds.memory, (b) => setKinds({ memory: b.checked }), "(the memory folder of each project)"),
    check(
      "p-state",
      "Mod settings (~/.claude/personal-state)",
      st.kinds.state,
      (b) => setKinds({ state: b.checked }),
      "Small settings your mods save, such as opus-only mode.",
    ),
    st.newDevicesJoin !== null &&
      check("p-join", "New devices of mine join automatically", st.newDevicesJoin, async (b) => {
        show(await api.personalSetNewDevicesJoin(b.checked));
      }),
    h("h3", null, "Devices"),
    devices(st, show),
    h("p", { class: "muted small", role: "status" }, `Last synced ${relTime(st.lastSyncUnix)}.`),
    st.lastError ? h("div", { class: "error", role: "alert" }, st.lastError) : null,
    st.skipped.length > 0 &&
      h(
        "details",
        null,
        h("summary", { class: "muted small" }, `${st.skipped.length} file${st.skipped.length === 1 ? "" : "s"} stay on this device`),
        h("ul", { class: "muted small" }, st.skipped.map((s) => h("li", null, s))),
      ),
    ...conflicts(st),
    h("div", { class: "actions" }, repo),
  ];
}

/** The card, and how to reload it after a sync. */
export function personalCard(): { card: HTMLElement; reload: () => void } {
  const inner = h("div", null, spinner("Loading personal memory"));
  const card = h("section", { class: "card", id: "personal", "aria-labelledby": "personal-h" }, h("h2", { id: "personal-h" }, "Personal memory"), inner);
  const show = (st: PersonalStatus) => replace(inner, body(st, show));
  const reload = () =>
    api.personalStatus().then(show, (e) => replace(inner, errorBox(errorText(e), reload)));
  reload();
  return { card, reload };
}

/** The notice above every screen while this device was not asked yet. */
export function personalOffer(): HTMLElement | false {
  if (!store.personal?.offer) return false;
  const on = h("button", { class: "primary" }, "Turn on");
  on.addEventListener("click", () =>
    busy(on, "Turning on…", async () => {
      store.set({ personal: await api.personalEnable() });
      toast("Personal memory is on");
      store.bump();
    }),
  );
  const later = h("button", null, "Not now");
  later.addEventListener("click", () =>
    busy(later, "Working…", async () => {
      await api.personalDismissOffer();
      store.set({ personal: { ...store.personal!, offer: false, decided: true } });
      store.bump();
    }),
  );
  return h(
    "div",
    { class: "notice info", role: "region", "aria-label": "Personal memory" },
    h(
      "div",
      { class: "notice-text" },
      "Your other devices share personal memory: your CLAUDE.md text and Claude's memories. Turn it on here too? You can change this in Settings.",
    ),
    h("div", { class: "actions" }, on, later),
  );
}
