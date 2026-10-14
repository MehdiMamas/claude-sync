// One group: what it gives this device, its scheduled jobs, members, sharing a skill,
// recent changes, conflicts and collisions.
import {
  api,
  errorText,
  groupKey,
  type Collision,
  type ContentItem,
  type GroupContents,
  type GroupId,
  type GroupStatus,
  type JobsOverview,
  type JobView,
  type MarketplaceInfo,
  type PermissionMode,
} from "../api";
import { avatar } from "../avatar";
import { busy, confirmButton, errorBox, h, relTime, replace, spinner, toast } from "../dom";
import { go, onRefresh, store } from "../store";
import { claudeMdCard } from "./claude-md";
import { historyCard } from "./history";

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
        { class: "grow who" },
        avatar(m, 20),
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
      h("ul", { class: "list flat" }, g.invited.map((m) => h("li", { class: "row muted" }, h("div", { class: "grow who" }, avatar(m, 20), m)))),
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

function contentsCard(contents: Promise<GroupContents>): HTMLElement {
  const body = h("div", null, spinner("Reading what this group gives you"));
  contents.then(
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

/** Group plugins that could not be installed or updated after a sync, with a Retry. */
function pluginNotice(id: GroupId): HTMLElement {
  const box = h("div");
  const show = (warnings: string[]) => {
    if (warnings.length === 0) {
      replace(box);
      return;
    }
    const retry = h("button", null, "Retry");
    retry.addEventListener("click", () =>
      busy(retry, "Installing…", async () => {
        const left = await api.retryPlugins(id);
        show(left);
        if (left.length === 0) toast("The group's plugins are installed");
      }),
    );
    replace(
      box,
      h(
        "div",
        { class: "notice", role: "status" },
        h("div", { class: "notice-text" }, h("strong", null, "Plugins of this group are missing"), h("ul", { class: "small warnings" }, warnings.map((w) => h("li", null, w)))),
        retry,
      ),
    );
  };
  api.pluginWarnings(id).then(show, () => {});
  return box;
}

const MODES: Record<PermissionMode, string> = {
  default: "Claude Code asks before edits and commands. Nobody is there to answer, so it skips them.",
  acceptEdits: "Claude Code edits files without asking; it skips other commands that need asking.",
  plan: "Claude Code only reads and plans; it changes nothing.",
  bypassPermissions: "Claude Code runs any command and edits any file without asking.",
};

function permission(mode: PermissionMode): HTMLElement {
  const text = [h("strong", null, mode), ": ", MODES[mode] ?? ""];
  return mode === "bypassPermissions"
    ? h("div", { class: "notice perm" }, h("div", { class: "notice-text" }, ...text))
    : h("p", { class: "small" }, ...text);
}

function lastRunText(j: JobView): (string | HTMLElement)[] {
  if (j.running) return [h("span", { class: "spin", "aria-hidden": "true" }), " Running now"];
  const r = j.lastRun;
  if (!r) return ["Not run yet"];
  const exit = r.exitCode !== null && r.status !== "ok" ? ` (exit code ${r.exitCode})` : "";
  return [
    `Last run ${relTime(r.endUnix || r.startUnix)}${r.manual ? " (run by hand)" : ""}: `,
    h("span", { class: r.status === "ok" ? "tag" : "tag warn" }, r.status),
    exit,
  ];
}

function pendingJob(id: GroupId, j: JobView, unsupported: boolean, show: (o: JobsOverview) => void): HTMLElement {
  const accept = h("button", { class: "primary", disabled: unsupported }, "Accept and turn on");
  accept.addEventListener("click", () =>
    busy(accept, "Accepting…", async () => {
      show(await api.acceptJob(id, j.id, j.hash));
      toast(`${j.title} runs ${j.schedule}`);
    }),
  );
  const always = h("button", { disabled: unsupported }, `Always allow jobs from ${groupKey(id)}`);
  always.addEventListener("click", () =>
    busy(always, "Allowing…", async () => {
      show(await api.setJobsAutoAccept(id, true));
      toast(`Jobs from ${groupKey(id)} now run without asking`);
    }),
  );
  return h(
    "li",
    { class: "job pending" },
    h("div", { class: "row-title" }, j.title, h("span", { class: "tag warn" }, j.changed ? "changed, needs your OK again" : "needs your OK")),
    h("p", { class: "small" }, h("strong", null, j.schedule), ` (your time), for at most ${j.timeoutMinutes} min, in `, h("code", null, j.cwd), "."),
    h("p", { class: "small" }, "Claude Code gets this prompt:"),
    h("pre", { class: "logs prompt" }, j.prompt),
    permission(j.permissionMode),
    j.args.length > 0 && h("p", { class: "small" }, "Claude Code options: ", h("code", null, j.args.join(" ")), j.args.includes("--chrome") && " (uses Chrome through Claude in Chrome when it is running)"),
    j.report && h("p", { class: "muted small" }, "Its report: ", h("code", null, j.report)),
    h("p", { class: "muted small" }, "It only runs on this device after you accept it. If the group changes it, it stops until you accept it again."),
    h("div", { class: "actions" }, accept, always),
    h("p", { class: "muted small" }, `"Always allow" accepts this job and every new or changed job of ${groupKey(id)} from now on, without showing it to you first. You get a notification each time.`),
  );
}

function acceptedJob(id: GroupId, j: JobView, show: (o: JobsOverview) => void): HTMLElement {
  const toggleId = `job-on-${j.id}`;
  const toggle = h("input", { id: toggleId, type: "checkbox", checked: j.enabled });
  toggle.addEventListener("change", () => {
    const on = toggle.checked;
    toggle.disabled = true;
    api.setJobEnabled(id, j.id, on).then(
      (o) => {
        show(o);
        toast(on ? `${j.title} is on` : `${j.title} is off`);
      },
      (e) => {
        toggle.checked = !on;
        toggle.disabled = false;
        toast(errorText(e), "error");
      },
    );
  });
  const run = h("button", { disabled: j.running }, "Run now");
  run.addEventListener("click", () => busy(run, "Starting…", async () => show(await api.runJobNow(id, j.id))));
  const open = (label: string, kind: "log" | "report") => {
    const b = h("button", null, label);
    b.addEventListener("click", () => api.openJobFile(id, j.id, kind).catch((e) => toast(errorText(e), "error")));
    return b;
  };
  const mode = j.permissionMode === "bypassPermissions" ? "tag warn" : "tag off";
  return h(
    "li",
    { class: "job" },
    h(
      "div",
      { class: "row" },
      h(
        "div",
        { class: "grow" },
        h("div", { class: "row-title" }, j.title, h("span", { class: mode }, j.permissionMode)),
        h("div", { class: "muted small" }, j.enabled ? j.schedule : `Off · ${j.schedule}`),
        h("div", { class: "small" }, lastRunText(j)),
        j.error && h("div", { class: "error-text small" }, j.error),
      ),
      h("div", { class: "check" }, toggle, h("label", { for: toggleId }, "On")),
    ),
    h(
      "details",
      null,
      h("summary", { class: "small" }, "Prompt"),
      h("pre", { class: "logs prompt" }, j.prompt),
      j.args.length > 0 && h("p", { class: "small" }, "Options: ", h("code", null, j.args.join(" "))),
    ),
    h("div", { class: "actions" }, run, j.hasLog && open("Open log", "log"), j.hasReport && open("Open report", "report")),
  );
}

/** "Run this group's jobs without asking": per device, off by default. */
function autoAcceptSetting(id: GroupId, o: JobsOverview, show: (o: JobsOverview) => void): HTMLElement {
  const box = h("input", { id: "jobs-auto", type: "checkbox", checked: o.autoAccept, disabled: o.unsupported !== null });
  box.addEventListener("change", () => {
    const on = box.checked;
    box.disabled = true;
    api.setJobsAutoAccept(id, on).then(
      (next) => {
        show(next);
        toast(on ? `Jobs from ${groupKey(id)} now run without asking` : `New or changed jobs from ${groupKey(id)} need your OK again`);
      },
      (e) => {
        box.checked = !on;
        box.disabled = false;
        toast(errorText(e), "error");
      },
    );
  });
  const warning = "New or changed jobs from this group then run on this device without your review, with the permissions the group gives them (bypassPermissions included). You get a notification each time. Jobs you turned off stay off.";
  return h(
    "div",
    { class: "jobs-auto" },
    h("div", { class: "check" }, box, h("label", { for: "jobs-auto" }, "Run this group's jobs without asking")),
    o.autoAccept
      ? h("div", { class: "notice perm" }, h("div", { class: "notice-text" }, warning))
      : h("p", { class: "muted small" }, warning),
  );
}

function jobsBody(id: GroupId, o: JobsOverview, show: (o: JobsOverview) => void): (HTMLElement | false | null | HTMLElement[])[] {
  if (o.jobs.length === 0 && o.problems.length === 0) {
    return [h("p", { class: "muted" }, "This group schedules no jobs."), o.autoAccept && autoAcceptSetting(id, o, show)];
  }
  const pending = o.jobs.filter((j) => !j.accepted);
  const accepted = o.jobs.filter((j) => j.accepted);
  return [
    autoAcceptSetting(id, o, show),
    o.unsupported !== null && h("div", { class: "notice" }, h("div", { class: "notice-text" }, o.unsupported)),
    pending.length > 0 && h("ul", { class: "list flat jobs", "aria-label": "Jobs waiting for your OK" }, pending.map((j) => pendingJob(id, j, o.unsupported !== null, show))),
    accepted.length > 0 && h("ul", { class: "list flat jobs", "aria-label": "Accepted jobs" }, accepted.map((j) => acceptedJob(id, j, show))),
    o.problems.length > 0 && [
      h("h3", null, "Skipped"),
      h("p", { class: "muted small" }, "These entries of schedules.json are not valid, so they never run."),
      h(
        "ul",
        { class: "list flat", "aria-label": "Skipped jobs" },
        o.problems.map((p) => h("li", { class: "row" }, h("div", { class: "grow small" }, p.id !== null && h("code", null, p.id), p.id !== null && ": ", p.message))),
      ),
    ],
  ];
}

function jobsCard(id: GroupId): HTMLElement {
  const body = h("div", null, spinner("Reading the group's scheduled jobs"));
  const section = h(
    "section",
    { class: "card", "aria-labelledby": "jobs-h" },
    h("h2", { id: "jobs-h" }, "Scheduled jobs"),
    h("p", { class: "muted small" }, "Claude Code tasks this group runs on a schedule. Each one needs your OK on this device first, unless you let this group's jobs run without asking."),
    body,
  );
  let timer: number | undefined;
  const show = (o: JobsOverview) => {
    window.clearTimeout(timer);
    replace(body, jobsBody(id, o, show));
    // Follow a run until it ends; stop once the screen is gone.
    if (o.jobs.some((j) => j.running)) {
      timer = window.setTimeout(() => {
        if (section.isConnected) api.jobs(id).then(show, () => {});
      }, 2000);
    }
  };
  api.jobs(id).then(show, (e) => replace(body, errorBox(errorText(e))));
  return section;
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
      // Read once: the contents card lists it and the history card names mods and skills by it.
      const contents = api.groupContents(g.id);
      replace(
        body,
        g.lastError && h("div", { class: "error", role: "alert" }, g.lastError),
        pluginNotice(g.id),
        contentsCard(contents),
        jobsCard(g.id),
        problemsCard(g),
        membersCard(g, load),
        shareCard(g.id),
        claudeMdCard(g.id),
        historyCard(g.id, contents),
      );
    } catch (e) {
      replace(body, errorBox(errorText(e), load));
    }
  };
  load();
  onRefresh(load);
}
