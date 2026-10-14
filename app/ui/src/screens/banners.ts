// Notices shown above every screen: a ready update, plugins to reload, the personal memory
// offer, and local edits to shared files (§7).
import { api, groupKey, type EditAction, type LocalEdit } from "../api";
import { busy, h, toast } from "../dom";
import { store } from "../store";
import { personalOffer } from "./personal";

export const PLUGINS_CHANGED = "Plugins changed. Run /reload-plugins in open Claude Code sessions, or restart them.";

const ACTIONS: { action: EditAction; label: string; done: string }[] = [
  { action: "share", label: "Share with group", done: "Shared with the group" },
  { action: "keepLocal", label: "Keep local", done: "Kept on this device only" },
  { action: "undo", label: "Undo", done: "Restored the group's version" },
];

function editNotice(edit: LocalEdit): HTMLElement {
  const what = edit.deleted ? "was deleted on this device" : "was changed on this device";
  return h(
    "div",
    { class: "notice", role: "region", "aria-label": `Local change to ${edit.path}` },
    h(
      "div",
      { class: "notice-text" },
      h("code", null, edit.path),
      ` ${what}. It is shared by `,
      h("strong", null, groupKey(edit.group)),
      ".",
    ),
    h(
      "div",
      { class: "actions" },
      ACTIONS.map(({ action, label, done }) => {
        const btn = h("button", { class: action === "share" ? "primary" : "" }, label);
        btn.addEventListener("click", () =>
          busy(btn, "Working…", async () => {
            await api.resolveEdit(edit, action);
            store.set({ edits: store.edits.filter((e) => e !== edit) });
            toast(done);
          }),
        );
        return btn;
      }),
    ),
  );
}

export function banners(): HTMLElement[] {
  const out: HTMLElement[] = [];
  const u = store.update;
  if (u?.available && u.downloaded) {
    const btn = h("button", { class: "primary" }, "Restart to update");
    btn.addEventListener("click", () => busy(btn, "Restarting…", () => api.installUpdate()));
    out.push(
      h(
        "div",
        { class: "notice info", role: "region", "aria-label": "Update" },
        h("div", { class: "notice-text" }, `claude-sync ${u.version} is ready. It installs when you quit, or now:`),
        h("div", { class: "actions" }, btn),
      ),
    );
  }
  if (store.pluginsChanged) {
    const btn = h("button", null, "Dismiss");
    btn.addEventListener("click", () => store.set({ pluginsChanged: false }));
    out.push(
      h(
        "div",
        { class: "notice info", role: "region", "aria-label": "Plugins changed" },
        h("div", { class: "notice-text" }, PLUGINS_CHANGED),
        h("div", { class: "actions" }, btn),
      ),
    );
  }
  const offer = personalOffer();
  if (offer) out.push(offer);
  for (const e of store.edits) out.push(editNotice(e));
  return out;
}
