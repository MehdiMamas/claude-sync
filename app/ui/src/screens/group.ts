// One group: members, sharing a skill, recent changes, conflicts and collisions.
import { api, errorText, groupKey, type Collision, type GroupId, type GroupStatus } from "../api";
import { busy, confirmButton, errorBox, h, relTime, replace, spinner, toast } from "../dom";
import { go, onRefresh, store } from "../store";

function membersCard(g: GroupStatus, reload: () => void): HTMLElement {
  const you = store.auth?.login?.toLowerCase();
  const hosting = g.host.toLowerCase() === you;
  const login = h("input", { id: "m-login", placeholder: "GitHub login", autocomplete: "off", spellcheck: "false", required: true });
  const add = h("button", { class: "primary", type: "submit" }, "Add member");
  const form = h(
    "form",
    { class: "inline-form" },
    h("label", { for: "m-login", class: "sr-only" }, "GitHub login to add"),
    login,
    add,
  );
  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const who = login.value.trim().replace(/^@/, "");
    if (!who) return;
    busy(add, "Adding…", async () => {
      await api.addMember(g.id, who);
      toast(`Invited ${who}. They get an invitation from GitHub.`);
      login.value = "";
      reload();
    });
  });

  const memberRow = (m: string) => {
    const isHost = m.toLowerCase() === g.host.toLowerCase();
    const remove =
      !isHost &&
      confirmButton("Remove", `Remove ${m}?`, (btn) =>
        busy(btn, "Removing…", async () => {
          await api.removeMember(g.id, m);
          toast(`Removed ${m}`);
          reload();
        }),
      );
    if (remove) remove.setAttribute("aria-label", `Remove ${m}`);
    return h(
      "li",
      { class: "row" },
      h(
        "div",
        { class: "grow" },
        m,
        isHost && h("span", { class: "tag" }, "host"),
        m.toLowerCase() === you && h("span", { class: "tag" }, "you"),
      ),
      remove,
    );
  };

  return h(
    "section",
    { class: "card", "aria-labelledby": "members-h" },
    h("h2", { id: "members-h" }, "Members"),
    h("ul", { class: "list flat" }, g.members.map(memberRow)),
    g.invited.length > 0 && [
      h("h3", null, "Invited, not joined yet"),
      h("ul", { class: "list flat" }, g.invited.map((m) => h("li", { class: "row muted" }, m))),
    ],
    form,
    !hosting && h("p", { class: "muted small" }, `${g.host} hosts this group. Members can add and remove people too.`),
  );
}

function shareCard(id: GroupId): HTMLElement {
  const select = h("select", { id: "skill" });
  const share = h("button", { class: "primary" }, "Share");
  const body = h("div", null, spinner("Looking for skills"));
  share.addEventListener("click", () =>
    busy(share, "Sharing…", async () => {
      await api.shareSkill(id, select.value);
      toast(`Shared ${select.value} with ${groupKey(id)}`);
    }),
  );
  api.localSkills().then(
    (skills) => {
      if (skills.length === 0) {
        replace(body, h("p", { class: "muted" }, "No skills of your own in ~/.claude/skills yet."));
        return;
      }
      replace(select, skills.map((s) => h("option", { value: s }, s)));
      replace(
        body,
        h("label", { for: "skill" }, "Skill"),
        h("div", { class: "inline-form" }, select, share),
      );
    },
    (e) => replace(body, errorBox(errorText(e))),
  );
  return h(
    "section",
    { class: "card form", "aria-labelledby": "share-h" },
    h("h2", { id: "share-h" }, "Share a skill"),
    h("p", { class: "muted small" }, "Copies one of your skills into the group so every member gets it."),
    body,
  );
}

function historyCard(id: GroupId): HTMLElement {
  const body = h("div", null, spinner("Loading changes"));
  api.recentCommits(id, 20).then(
    (commits) =>
      replace(
        body,
        commits.length === 0
          ? h("p", { class: "muted" }, "No changes yet.")
          : h(
              "ol",
              { class: "list flat", "aria-label": "Recent changes" },
              commits.map((c) =>
                h(
                  "li",
                  { class: "row" },
                  h("div", { class: "grow" }, h("div", null, c.message.split("\n")[0]), h("div", { class: "muted small" }, `${c.author} · ${relTime(c.atUnix)}`)),
                  h("code", { class: "muted small", title: c.sha }, c.sha.slice(0, 7)),
                ),
              ),
            ),
      ),
    (e) => replace(body, errorBox(errorText(e))),
  );
  return h("section", { class: "card", "aria-labelledby": "history-h" }, h("h2", { id: "history-h" }, "Recent changes"), body);
}

function openButton(label: string, path: string, reveal: boolean): HTMLButtonElement {
  const b = h("button", null, label);
  b.addEventListener("click", () => api.openPath(path, reveal).catch((e) => toast(errorText(e), "error")));
  return b;
}

function collisionText(c: Collision): (string | HTMLElement)[] {
  if (c.kind === "userFile") {
    return [h("code", null, c.path), " is your own file, so the group's copy was not written over it. Rename one of them to use both."];
  }
  return [h("code", null, c.path), " is also shared by ", h("strong", null, groupKey(c.owner)), "; that group's copy wins on this device."];
}

function problemsCard(g: GroupStatus): HTMLElement | false {
  if (g.conflicts.length === 0 && g.collisions.length === 0) return false;
  return h(
    "section",
    { class: "card", "aria-labelledby": "problems-h" },
    h("h2", { id: "problems-h" }, "To review"),
    g.conflicts.length > 0 && [
      h("p", { class: "muted small" }, "Two people changed the same file. The group's version is in place; your version was saved next to it."),
      h(
        "ul",
        { class: "list flat", "aria-label": "Conflicts" },
        g.conflicts.map((c) =>
          h(
            "li",
            { class: "row" },
            h("div", { class: "grow" }, h("code", null, c.path), h("div", { class: "muted small" }, relTime(c.atUnix))),
            h("div", { class: "actions" }, openButton("Open saved copy", c.savedTo, false), openButton("Show in folder", c.savedTo, true)),
          ),
        ),
      ),
    ],
    g.collisions.length > 0 &&
      h("ul", { class: "list flat", "aria-label": "Name clashes" }, g.collisions.map((c) => h("li", { class: "row" }, h("div", { class: "grow" }, collisionText(c))))),
  );
}

export function groupScreen(view: HTMLElement, id: GroupId): void {
  const head = h("header", { class: "head" }, h("div", null, h("a", { href: "#/groups", class: "back" }, "← Groups"), h("h1", null, groupKey(id))));
  const body = h("div", null, spinner("Loading group"));
  replace(view, head, body);

  const load = async () => {
    try {
      const groups = await api.groups();
      const g = groups.find((x) => x.id.owner === id.owner && x.id.repo === id.repo);
      if (!g) {
        replace(body, errorBox(`You are not in ${groupKey(id)} on this device.`));
        return;
      }
      const leave = confirmButton("Leave group", "Click again to leave", (btn) =>
        busy(btn, "Leaving…", async () => {
          await api.leaveGroup(id);
          toast(`Left ${g.name}. Its files were removed from this device.`);
          go("groups");
        }),
      );
      replace(
        head,
        h(
          "div",
          null,
          h("a", { href: "#/groups", class: "back" }, "← Groups"),
          h("h1", null, g.name),
          h("div", { class: "muted small" }, `${groupKey(g.id)} · synced ${relTime(g.lastSyncUnix)}`),
        ),
        h("div", { class: "actions" }, leave),
      );
      replace(
        body,
        g.lastError && h("div", { class: "error", role: "alert" }, g.lastError),
        problemsCard(g),
        membersCard(g, load),
        shareCard(g.id),
        historyCard(g.id),
      );
    } catch (e) {
      replace(body, errorBox(errorText(e), load));
    }
  };
  load();
  onRefresh(load);
}
