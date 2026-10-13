import "./style.css";
import { api, on, setBackend } from "./api";
import { go, onRefresh, store, subscribe } from "./store";
import { h, replace, relTime } from "./dom";
import { banners } from "./screens/banners";
import { signInScreen } from "./screens/signin";
import { groupsScreen } from "./screens/groups";
import { groupScreen } from "./screens/group";
import { settingsScreen } from "./screens/settings";
import { feedbackScreen } from "./screens/feedback";

const root = document.getElementById("root")!;

type Route =
  | { name: "groups" }
  | { name: "group"; owner: string; repo: string }
  | { name: "settings" }
  | { name: "feedback" };

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
        h("div", { class: "brand" }, h("span", { class: "logo", "aria-hidden": "true" }, "⟳"), "claude-sync"),
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
          h("div", { class: "muted small" }, `Signed in as ${store.auth.login ?? "?"}`),
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
    on("navigate", (route) => go(route)),
  ]);
  const [auth, status, info, update] = await Promise.all([
    api.authStatus(),
    api.status(),
    api.appInfo(),
    api.updateStatus(),
  ]);
  store.set({ auth, status, info, update });
  if (auth.signedIn) {
    api.localEdits().then(
      (edits) => store.set({ edits }),
      () => {},
    );
  }
}

boot().catch((e) => {
  root.textContent = `claude-sync could not start its window: ${e}`;
});
