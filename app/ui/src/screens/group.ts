// One group: what it gives this device, members, sharing a skill, recent changes,
// conflicts and collisions.
import {
  api,
  errorText,
  groupKey,
  type Collision,
  type ContentItem,
  type GroupContents,
  type GroupId,
  type GroupStatus,
  type MarketplaceInfo,
} from "../api";
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

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** "a, b or c" */
function orList(words: string[]): string {
  return words.length < 2 ? words.join("") : `${words.slice(0, -1).join(", ")} or ${words[words.length - 1]}`;
}

function marketplaceBlock(mp: MarketplaceInfo): HTMLElement {
  const title = h("div", { class: "row-title" }, mp.name);
  if (mp.heldBy) {
    return h(
      "li",
      { class: "row" },
      h(
        "div",
        { class: "grow" },
        title,
        h("div", { class: "muted small" }, h("strong", null, groupKey(mp.heldBy)), ` also ships ${mp.name} and was here first, so its copy is the one you use.`),
      ),
      h("span", { class: "tag warn" }, "from another group"),
    );
  }
  if (!mp.path) {
    return h(
      "li",
      { class: "row" },
      h("div", { class: "grow" }, title, h("div", { class: "muted small" }, "Not installed: it has no valid .claude-plugin/marketplace.json.")),
      h("span", { class: "tag warn" }, "not installed"),
    );
  }
  const open = openButton("Open folder", mp.path, false, `Open the ${mp.name} folder`);
  return h(
    "li",
    { class: "mp" },
    h(
      "div",
      { class: "row" },
      h("div", { class: "grow" }, title, mp.description && h("div", { class: "muted small" }, mp.description), h("code", { class: "muted small" }, mp.path)),
      open,
    ),
    mp.plugins.length === 0
      ? h("p", { class: "muted small" }, "It lists no plugins.")
      : h(
          "ul",
          { class: "list flat plugins", "aria-label": `Plugins in ${mp.name}` },
          mp.plugins.map((p) =>
            h(
              "li",
              { class: "row" },
              h(
                "div",
                { class: "grow" },
                h("div", null, h("strong", null, p.name), p.version && h("span", { class: "muted small" }, ` ${p.version}`)),
                p.description && h("div", { class: "muted small clamp" }, p.description),
              ),
              p.enabled ? h("span", { class: "tag" }, "on") : h("span", { class: "tag off", title: "Not turned on in your settings.json" }, "off"),
            ),
          ),
        ),
  );
}

function itemList(label: string, items: ContentItem[]): HTMLElement {
  return h(
    "ul",
    { class: "list flat", "aria-label": label },
    items.map((i) => h("li", { class: "row" }, h("div", { class: "grow" }, h("strong", null, i.name), i.description && h("div", { class: "muted small clamp" }, i.description)))),
  );
}

function contentsBody(c: GroupContents): (HTMLElement | false)[] {
  const mods = c.marketplaces.reduce((n, mp) => n + (mp.path ? mp.plugins.length : 0), 0);
  const kinds: [string, string, ContentItem[]][] = [
    ["Skills", "skill", c.skills],
    ["Agents", "agent", c.agents],
    ["Commands", "command", c.commands],
    ["Output styles", "output style", c.outputStyles],
  ];
  const settings = c.settings.keys.length > 0;
  const summary = [
    plural(mods, "mod"),
    ...kinds.map(([, one, items]) => plural(items.length, one)),
    plural(c.settings.keys.length, "setting"),
    c.claudeMd ? "shared CLAUDE.md" : "",
  ].filter(Boolean);
  const empty = [
    c.marketplaces.length === 0 && "mods",
    ...kinds.filter(([, , items]) => items.length === 0).map(([title]) => title.toLowerCase()),
    !settings && "settings",
    !c.claudeMd && "CLAUDE.md",
  ].filter((x): x is string => Boolean(x));
  if (empty.length === kinds.length + 3) {
    return [h("p", { class: "muted" }, "This group does not share anything yet.")];
  }
  const on = c.settings.enabledPlugins.filter(([, v]) => v).map(([k]) => k);
  const off = c.settings.enabledPlugins.filter(([, v]) => !v).map(([k]) => k);
  const codes = (xs: string[]) => xs.flatMap((x, i) => [i > 0 ? ", " : "", h("code", null, x)]);
  return [
    h("p", { class: "muted small" }, summary.join(" · ")),
    c.marketplaces.length > 0 && h("h3", null, "Mods"),
    c.marketplaces.length > 0 && h("ul", { class: "list flat", "aria-label": "Plugin marketplaces" }, c.marketplaces.map(marketplaceBlock)),
    ...kinds.flatMap(([title, , items]) => (items.length > 0 ? [h("h3", null, title), itemList(title, items)] : [])),
    (settings || c.claudeMd !== null) && h("h3", null, "Settings and instructions"),
    settings &&
      h(
        "div",
        { class: "settings-keys" },
        h("p", null, "Sets in your settings.json: ", codes(c.settings.keys)),
        on.length > 0 && h("p", { class: "small" }, "Turns on: ", codes(on)),
        off.length > 0 && h("p", { class: "small" }, "Turns off: ", codes(off)),
        c.settings.keptLocal && h("p", { class: "muted small" }, "You kept your own settings on this device, so these are not applied."),
      ),
    c.claudeMd !== null &&
      h(
        "div",
        { class: "row flat-row" },
        h("div", { class: "grow" }, "Shared instructions (CLAUDE.md), added to your own CLAUDE.md"),
        openButton("Open", c.claudeMd, false, "Open the group's CLAUDE.md"),
      ),
    empty.length > 0 && h("p", { class: "muted small" }, `No ${orList(empty)} yet.`),
  ];
}

function contentsCard(id: GroupId): HTMLElement {
  const body = h("div", null, spinner("Reading what this group gives you"));
  api.groupContents(id).then(
    (c) => replace(body, contentsBody(c)),
    (e) => replace(body, errorBox(errorText(e))),
  );
  return h(
    "section",
    { class: "card", "aria-labelledby": "contents-h" },
    h("h2", { id: "contents-h" }, "From this group"),
    h("p", { class: "muted small" }, "What this group puts on this device."),
    body,
  );
}

function shareCard(id: GroupId): HTMLElement {
  const select = h("select", { id: "skill" });
  const share = h("button", { class: "primary" }, "Share");
  const body = h("div", null, spinner("Looking for skills"));
  const none = () =>
    replace(body, h("p", { class: "muted" }, "You have no skills to share. Skills you make in ~/.claude/skills show up here."));
  share.addEventListener("click", () =>
    busy(share, "Sharing…", async () => {
      const skill = select.value;
      await api.shareSkill(id, skill);
      toast(`Shared ${skill} with ${groupKey(id)}`);
      select.querySelector(`option[value="${CSS.escape(skill)}"]`)?.remove();
      if (select.options.length === 0) none();
    }),
  );
  api.localSkills().then(
    (skills) => {
      if (skills.length === 0) {
        none();
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
    h("h2", { id: "share-h" }, "Share one of your skills"),
    h("p", { class: "muted small" }, "Your own skills that the group does not have yet. Sharing copies the skill to every member."),
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

function openButton(label: string, path: string, reveal: boolean, ariaLabel?: string): HTMLButtonElement {
  const b = h("button", { "aria-label": ariaLabel }, label);
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
        contentsCard(g.id),
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
