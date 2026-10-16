// Groups: joined groups with status, pending invitations, groups to join, create a group.
import { api, errorText, groupKey, type GroupId, type GroupStatus, type Invitation } from "../api";
import { busy, errorBox, h, relTime, replace, spinner, toast } from "../dom";
import { countReview, go, onRefresh, store, subscribe } from "../store";
import { claudeMdEditor } from "./claude-md";

// Shown wherever the user can join a group (§4: a group can bundle plugins).
const PLUGINS_WARNING =
  "Plugins in this group run code on every member's machine; only join groups whose members you trust.";

// Invitations arrive as events; keep the list on screen current.
let liveInvites: (() => void) | null = null;
subscribe(() => liveInvites?.());

function groupRow(g: GroupStatus, pos: number, all: GroupStatus[], move: (from: number, to: number) => void): HTMLElement {
  const you = store.auth?.login?.toLowerCase();
  const hosting = g.host.toLowerCase() === you;
  const problems = g.conflicts.length + g.collisions.length;
  const arrow = (label: string, text: string, to: number) => {
    const b = h("button", { class: "icon-btn", type: "button", "aria-label": `${label} ${g.name}`, title: label }, text);
    if (to < 0 || to >= all.length) b.disabled = true;
    else b.addEventListener("click", () => move(pos, to));
    return b;
  };
  return h(
    "li",
    { class: "group-item" },
    all.length > 1 && h("div", { class: "reorder" }, arrow("Move up", "▲", pos - 1), arrow("Move down", "▼", pos + 1)),
    h(
      "a",
      { class: "row link-row grow", href: `#/group/${encodeURIComponent(g.id.owner)}/${encodeURIComponent(g.id.repo)}` },
      h(
        "div",
        { class: "grow" },
        h("div", { class: "row-title" }, g.name, hosting && h("span", { class: "tag" }, "you host")),
        h("div", { class: "muted small" }, `${groupKey(g.id)} · ${g.members.length} member${g.members.length === 1 ? "" : "s"}`),
        g.lastError && h("div", { class: "error-text small" }, g.lastError),
      ),
      h(
        "div",
        { class: "right" },
        g.reviewPending > 0 &&
          h("span", { class: "tag warn" }, `${g.reviewPending} CLAUDE.md change${g.reviewPending === 1 ? "" : "s"} to review`),
        problems > 0 && h("span", { class: "tag warn" }, `${problems} to review`),
        h("div", { class: "muted small" }, `Synced ${relTime(g.lastSyncUnix)}`),
      ),
    ),
  );
}

function invitationRow(inv: Invitation, reload: () => void): HTMLElement {
  const btn = h("button", { class: "primary" }, "Accept");
  btn.addEventListener("click", () =>
    busy(btn, "Joining…", async () => {
      const g = await api.acceptInvitation(inv.id);
      store.set({ invitations: store.invitations.filter((i) => i.id !== inv.id) });
      toast(`Joined ${g.name}`);
      reload();
    }),
  );
  return h(
    "li",
    { class: "row" },
    h("div", { class: "grow" }, h("strong", null, inv.inviter), " invited you to ", h("code", null, groupKey(inv.repo))),
    btn,
  );
}

function discoveredRow(id: GroupId, reload: () => void): HTMLElement {
  const btn = h("button", null, "Join");
  btn.addEventListener("click", () =>
    busy(btn, "Joining…", async () => {
      const g = await api.join(id);
      toast(`Joined ${g.name}`);
      reload();
    }),
  );
  return h("li", { class: "row" }, h("div", { class: "grow" }, h("code", null, groupKey(id))), btn);
}

function createForm(onDone: () => void): HTMLElement {
  const name = h("input", { id: "g-name", required: true, placeholder: "Mehdi & friends", autocomplete: "off" });
  const repo = h("input", { id: "g-repo", value: "claude-sync-data", autocomplete: "off", spellcheck: "false" });
  const submit = h("button", { class: "primary", type: "submit" }, "Create group");
  const claudeMd = claudeMdEditor("g-md", true);
  const form = h(
    "form",
    { class: "card form" },
    h("h2", null, "Create a group"),
    h("p", { class: "muted small" }, "A private repo is created on your GitHub account. You host the group; members you add get an invitation from GitHub."),
    h("label", { for: "g-name" }, "Group name"),
    name,
    h("label", { for: "g-repo" }, "Repo name"),
    repo,
    claudeMd.el,
    h("div", { class: "actions" }, submit),
  );
  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    busy(submit, "Creating…", async () => {
      const g = await api.createGroup(name.value, repo.value, claudeMd.value() ?? undefined);
      toast(`Created ${groupKey(g.id)}`);
      onDone();
      go(`group/${encodeURIComponent(g.id.owner)}/${encodeURIComponent(g.id.repo)}`);
    });
  });
  return form;
}

export function groupsScreen(view: HTMLElement): void {
  const list = h("div");
  const invites = h("div");
  const discovered = h("div");
  const formSlot = h("div");
  const createBtn = h("button", null, "Create group");
  const syncBtn = h("button", { class: "primary" }, "Sync now");

  createBtn.addEventListener("click", () => {
    if (formSlot.firstChild) {
      formSlot.replaceChildren();
      return;
    }
    formSlot.replaceChildren(createForm(() => formSlot.replaceChildren()));
    formSlot.querySelector("input")?.focus();
  });
  syncBtn.addEventListener("click", () =>
    busy(syncBtn, "Syncing…", async () => {
      const r = await api.syncNow();
      const failed = r.groups.filter((g) => g.error).length;
      toast(failed ? `${failed} group(s) had an error` : "Up to date", failed ? "error" : "ok");
      loadGroups();
    }),
  );

  const loadGroups = async () => {
    if (!list.firstChild) list.replaceChildren(spinner("Loading groups"));
    try {
      const groups = await api.groups();
      countReview(groups);
      if (groups.length === 0) {
        replace(
          list,
          h(
            "div",
            { class: "empty" },
            h("p", null, "You are not in a group yet."),
            h("p", { class: "muted" }, "Create one and add your friends by GitHub login, or ask a friend to add you. Their invitation shows up here."),
          ),
        );
      } else {
        const move = async (from: number, to: number) => {
          const next = groups.slice();
          next.splice(to, 0, ...next.splice(from, 1));
          try {
            await api.reorderGroups(next.map((g) => g.id));
          } catch (e) {
            toast(errorText(e), "error");
          }
          await loadGroups();
          // Keep the keyboard on the moved group's button.
          const moved = list.querySelectorAll<HTMLElement>("li.group-item")[to];
          const btns = moved?.querySelectorAll<HTMLButtonElement>(".reorder button");
          (btns?.[to === 0 ? 1 : to > from ? 1 : 0] ?? btns?.[0])?.focus();
        };
        replace(
          list,
          groups.length > 1 && h("p", { class: "muted small" }, "Groups higher in the list win when two groups set the same thing."),
          h("ul", { class: "list", "aria-label": "Your groups" }, groups.map((g, i) => groupRow(g, i, groups, move))),
        );
      }
    } catch (e) {
      replace(list, errorBox(errorText(e), loadGroups));
    }
  };

  const renderInvites = () => {
    const inv = store.invitations;
    replace(
      invites,
      inv.length > 0 && [
        h("h2", null, "Invitations"),
        h("p", { class: "muted small" }, PLUGINS_WARNING),
        h("ul", { class: "list", "aria-label": "Pending invitations" }, inv.map((i) => invitationRow(i, loadAll))),
      ],
    );
  };

  const loadInvites = async () => {
    try {
      store.invitations = await api.invitations();
    } catch {
      // Shown by the sync status; the list just stays as it was.
    }
    renderInvites();
  };

  const loadDiscovered = async () => {
    try {
      const ids = await api.discover();
      replace(
        discovered,
        ids.length > 0 && [
          h("h2", null, "Groups you can join"),
          h("p", { class: "muted small" }, "You already have access to these on GitHub, but they are not set up on this device."),
          h("p", { class: "muted small" }, PLUGINS_WARNING),
          h("ul", { class: "list" }, ids.map((id) => discoveredRow(id, loadAll))),
        ],
      );
    } catch (e) {
      replace(discovered, h("p", { class: "muted small" }, `Could not look for other groups: ${errorText(e)}`));
    }
  };

  const loadAll = () => {
    loadGroups();
    loadInvites();
    loadDiscovered();
  };

  replace(
    view,
    h("header", { class: "head" }, h("h1", null, "Groups"), h("div", { class: "actions" }, createBtn, syncBtn)),
    formSlot,
    invites,
    h("h2", { class: "sr-only" }, "Your groups"),
    list,
    discovered,
  );
  renderInvites();
  let shown = store.invitations;
  liveInvites = () => {
    if (view.isConnected && store.invitations !== shown) {
      shown = store.invitations;
      renderInvites();
    }
  };
  loadAll();
  onRefresh(loadGroups);
}
