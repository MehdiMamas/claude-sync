// Notifications: every notification the app has shown, so a missed one can be read later.
// Opening this screen marks them all read; ones that arrive while it is open are marked
// read once the window has focus. Unread ones stay highlighted until the screen is left.
import { api, errorText, groupKey, type GroupId, type GroupStatus, type Notification, type NotificationKind } from "../api";
import { busy, confirmButton, errorBox, h, relTime, replace, spinner, toast } from "../dom";
import { store, subscribe } from "../store";

const KINDS: Record<NotificationKind, [glyph: string, label: string]> = {
  update: ["↑", "Update"],
  group: ["◉", "Group"],
  plugins: ["⧉", "Plugins"],
  "local-edit": ["✎", "Local change"],
  job: ["▶", "Job"],
  personal: ["●", "Personal sync"],
  other: ["•", "Other"],
};

// New notifications arrive as `notifications-changed`; reload the list on screen.
let live: (() => void) | null = null;
subscribe(() => live?.());

const groupPath = (g: GroupId) => `#/group/${encodeURIComponent(g.owner)}/${encodeURIComponent(g.repo)}`;

function item(n: Notification, unread: boolean, names: Map<string, string>): HTMLElement {
  const [glyph, label] = KINDS[n.kind] ?? KINDS.other;
  const when = new Date(n.time * 1000);
  const name = n.group && (names.get(groupKey(n.group)) ?? groupKey(n.group));
  const content = [
    h("span", { class: `kind-icon ${n.kind}`, "aria-hidden": "true" }, glyph),
    h(
      "div",
      { class: "grow" },
      h(
        "div",
        { class: "row-title" },
        n.title,
        unread && h("span", { class: "tag" }, "New"),
      ),
      n.body && h("div", { class: "small" }, n.body),
      h(
        "div",
        { class: "muted small" },
        label,
        name && ` · ${name}`,
      ),
    ),
    h(
      "div",
      { class: "right" },
      h("time", { class: "muted small", datetime: when.toISOString(), title: when.toLocaleString() }, relTime(n.time)),
    ),
  ];
  // A group's notification links to the group's screen, or to Groups when it is not joined.
  const row = n.group
    ? h("a", { class: "row link-row", href: names.has(groupKey(n.group)) ? groupPath(n.group) : "#/groups" }, content)
    : h("div", { class: "row" }, content);
  return h("li", { class: unread ? "unread" : null }, row);
}

export function notificationsScreen(view: HTMLElement): void {
  const body = h("div", null, spinner("Loading notifications"));
  const markAll = h("button", null, "Mark all as read");
  const clear = confirmButton("Clear", "Click again to clear", (btn) =>
    busy(btn, "Clearing…", async () => {
      await api.clearNotifications();
      toast("Notifications cleared");
      load();
    }),
  );
  // Ids that were unread when they reached the screen: highlighted until it is left.
  const fresh = new Set<number>();
  let names = new Map<string, string>();

  const markRead = () => {
    if (markAll.disabled || !document.hasFocus()) return;
    api.markAllNotificationsRead().then(
      () => (markAll.disabled = true),
      () => {},
    );
  };

  const load = async () => {
    try {
      const [items, groups] = await Promise.all([api.notifications(), api.groups().catch((): GroupStatus[] => [])]);
      names = new Map(groups.map((g) => [groupKey(g.id), g.name]));
      for (const n of items) if (!n.read) fresh.add(n.id);
      markAll.disabled = !items.some((n) => !n.read);
      clear.disabled = items.length === 0;
      if (items.length === 0) {
        replace(
          body,
          h(
            "div",
            { class: "empty" },
            h("p", null, "No notifications yet."),
            h("p", { class: "muted" }, "Updates, invitations, conflicts and other news show up here, even if you missed the desktop notification."),
          ),
        );
      } else {
        replace(body, h("ul", { class: "list notifications", "aria-label": "Notifications" }, items.map((n) => item(n, fresh.has(n.id), names))));
      }
      markRead();
    } catch (e) {
      replace(body, errorBox(errorText(e), load));
    }
  };

  markAll.addEventListener("click", () =>
    busy(markAll, "Marking…", async () => {
      await api.markAllNotificationsRead();
      fresh.clear();
      load();
    }),
  );

  replace(
    view,
    h("header", { class: "head" }, h("h1", null, "Notifications"), h("div", { class: "actions" }, markAll, clear)),
    body,
  );

  let seen = store.unread;
  live = () => {
    if (!view.isConnected) {
      live = null;
      return;
    }
    // Reload when something new arrived, not when this screen marked them read.
    if (store.unread !== seen) {
      const grew = store.unread > seen;
      seen = store.unread;
      if (grew) load();
    }
  };
  const onFocus = () => {
    if (!view.isConnected) {
      window.removeEventListener("focus", onFocus);
      return;
    }
    markRead();
  };
  window.addEventListener("focus", onFocus);
  load();
}
