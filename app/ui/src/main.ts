import "./style.css";
import { api, on, setBackend } from "./api";
import { go, onRefresh, store, subscribe } from "./store";
import { avatar } from "./avatar";
import { h, replace, relTime } from "./dom";
import { banners } from "./screens/banners";
import { signInScreen } from "./screens/signin";
import { groupsScreen } from "./screens/groups";
import { groupScreen } from "./screens/group";
import { settingsScreen } from "./screens/settings";
import { feedbackScreen } from "./screens/feedback";
import { notificationsScreen } from "./screens/notifications";

const root = document.getElementById("root")!;

type Route =
  | { name: "groups" }
  | { name: "group"; owner: string; repo: string }
  | { name: "settings" }
  | { name: "feedback" }
  | { name: "notifications" };

function parseRoute(): Route {
  const parts = location.hash.replace(/^#\/?/, "").split("/").filter(Boolean).map(decodeURIComponent);
  switch (parts[0]) {
    case "group":
      if (parts[1] && parts[2]) return { name: "group", owner: parts[1], repo: parts[2] };
      return { name: "groups" };
    case "settings":
      return { name: "settings" };
    case "feedback":
      return { name: "feedback" };
    case "notifications":
      return { name: "notifications" };
    default:
      return { name: "groups" };
  }
}

const NAV: { name: Route["name"]; path: string; label: string }[] = [
  { name: "groups", path: "groups", label: "Groups" },
  { name: "feedback", path: "feedback", label: "Send feedback" },
  { name: "settings", path: "settings", label: "Settings" },
];

function statusPill(): HTMLElement {
  const s = store.status;
  const last = s.lastSyncUnix ? ` · ${relTime(s.lastSyncUnix)}` : "";
  return h(
    "div",
    { class: `pill ${s.state}`, title: s.error ?? s.text },
    h("span", { class: "dot", "aria-hidden": "true" }),
    h("span", null, `${s.text}${last}`),
  );
}

const SVG = "http://www.w3.org/2000/svg";

/** The bell in the nav, with the unread count. Opens the Notifications screen. */
function bell(current: boolean): HTMLElement {
  const n = store.unread;
  const icon = document.createElementNS(SVG, "svg");
  icon.setAttribute("viewBox", "0 0 24 24");
  icon.setAttribute("aria-hidden", "true");
  const path = document.createElementNS(SVG, "path");
  path.setAttribute("d", "M12 3a6 6 0 0 0-6 6v3.6L4.3 15.5A1 1 0 0 0 5.2 17h13.6a1 1 0 0 0 .9-1.5L18 12.6V9a6 6 0 0 0-6-6Zm-2.4 15.5a2.5 2.5 0 0 0 4.8 0");
  icon.append(path);
  return h(
    "a",
    {
      class: "bell",
      href: "#/notifications",
      "aria-current": current ? "page" : false,
      "aria-label": n > 0 ? `Notifications, ${n} unread` : "Notifications",
      title: "Notifications",
    },
    icon,
    n > 0 && h("span", { class: "badge", "aria-hidden": "true" }, n > 99 ? "99+" : n),
  );
}

let currentKey = "";

function render(): void {
  if (!store.auth?.signedIn) {
    if (currentKey !== "signin") {
      currentKey = "signin";
      onRefresh(null);
      replace(root, signInScreen());
    }
    return;
  }
  const route = parseRoute();
  const key = JSON.stringify(route);
  // Same screen, only global state changed: update the status and banners, keep the forms.
  if (key === currentKey) {
    document.getElementById("status-slot")?.replaceChildren(statusPill());
    document.getElementById("bell-slot")?.replaceChildren(bell(route.name === "notifications"));
    document.getElementById("banners")?.replaceChildren(...banners());
    return;
  }
  currentKey = key;
  onRefresh(null);
  const navRoute = route.name === "group" ? "groups" : route.name;
  const view = h("section", { class: "view", tabindex: "-1" });
  replace(
    root,
    h(
      "div",
      { class: "app" },
      h(
        "nav",
        { class: "nav", "aria-label": "Main" },
        h(
          "div",
          { class: "brand" },
          h("span", { class: "logo", "aria-hidden": "true" }, "⟳"),
          h("span", { class: "grow" }, "claude-sync"),
          h("span", { id: "bell-slot" }, bell(route.name === "notifications")),
        ),
        h(
          "ul",
          null,
          NAV.map((n) =>
            h(
              "li",
              null,
              h("a", { href: `#/${n.path}`, "aria-current": n.name === navRoute ? "page" : false }, n.label),
            ),
          ),
        ),
        h(
          "div",
          { class: "nav-foot" },
          h("div", { id: "status-slot" }, statusPill()),
          h("div", { class: "muted small who" }, store.auth.login && avatar(store.auth.login, 18), `Signed in as ${store.auth.login ?? "?"}`),
        ),
      ),
      h("main", { class: "main" }, h("div", { id: "banners" }, banners()), view),
    ),
  );
  switch (route.name) {
    case "groups":
      groupsScreen(view);
      break;
    case "group":
      groupScreen(view, { owner: route.owner, repo: route.repo });
      break;
    case "settings":
      settingsScreen(view);
      break;
    case "feedback":
      feedbackScreen(view);
      break;
    case "notifications":
      notificationsScreen(view);
      break;
  }
  view.focus({ preventScroll: true });
}

async function boot(): Promise<void> {
  if (import.meta.env.DEV && !("__TAURI_INTERNALS__" in window)) {
    // `pnpm dev` in a plain browser: a fake backend for working on the UI.
    const mock = await import("./mock");
    setBackend(mock.backend);
  }
  window.addEventListener("hashchange", () => render());
  subscribe(render);
  await Promise.all([
    on("status", (status) => store.set({ status })),
    on("local-edits", (edits) => store.set({ edits })),
    on("invitations", (invitations) => store.set({ invitations })),
    on("update-available", (update) => store.set({ update })),
    on("signed-in", async () => store.set({ auth: await api.authStatus() })),
    on("sync-report", () => store.bump()),
    on("plugins-changed", () => store.set({ pluginsChanged: true })),
    on("notifications-changed", ({ unread }) => store.set({ unread })),
    on("navigate", (route) => go(route)),
    on("personal", (personal) => {
      store.set({ personal });
      store.bump();
    }),
  ]);
  const [auth, status, info, update] = await Promise.all([
    api.authStatus(),
    api.status(),
    api.appInfo(),
    api.updateStatus(),
  ]);
  store.set({ auth, status, info, update });
  api.notifications().then(
    (items) => store.set({ unread: items.filter((n) => !n.read).length }),
    () => {},
  );
  if (auth.signedIn) {
    api.localEdits().then(
      (edits) => store.set({ edits }),
      () => {},
    );
    api.personalStatus().then(
      (personal) => store.set({ personal }),
      () => {},
    );
  }
}

boot().catch((e) => {
  root.textContent = `claude-sync could not start its window: ${e}`;
});
