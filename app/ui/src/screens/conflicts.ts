// Groups that must not conflict (docs/architecture.md §7b): the "Conflicts" card lists rules
// this group shares with another group or the user's own CLAUDE.md, or contradicts, and
// settings two groups set differently; "Copy to another group" copies this group's content
// to another group the user is in.
import { api, errorText, type Clash, type ClashSide, type CopyFile, type CopyPreview, type GroupId, type GroupStatus } from "../api";
import { clashesOf, clashTitle, compose, isGroup, keys, sameGroup, sections, warning } from "../copy";
import { busy, errorBox, h, replace, spinner, toast } from "../dom";
import { store } from "../store";
import { explainButton } from "./helper";

const keepLabel = (s: ClashSide) => (s.source.kind === "own" ? "Keep yours" : `Keep ${s.name}'s`);

/** Lines of text that clash with what Claude already reads; nothing when none do. */
export function clashWarnings(clashes: Clash[]): HTMLElement | false {
  if (clashes.length === 0) return false;
  return h(
    "div",
    { class: "clash-warnings", role: "status" },
    h("p", { class: "warn-text small" }, "Some lines clash with what Claude already reads on this device:"),
    h("ul", { class: "small" }, clashes.slice(0, 20).map((c) => h("li", null, warning(c)))),
  );
}

/** Re-check `text` a moment after the user stops typing. */
export function checker(box: HTMLElement, check: (text: string) => Promise<Clash[]>): (text: string) => void {
  let timer: number | undefined;
  let seq = 0;
  return (text) => {
    window.clearTimeout(timer);
    const mine = ++seq;
    timer = window.setTimeout(() => {
      if (!text.trim()) {
        replace(box);
        return;
      }
      check(text).then(
        (cs) => mine === seq && replace(box, clashWarnings(cs)),
        () => mine === seq && replace(box),
      );
    }, 300);
  };
}

function sideRow(s: ClashSide, tag?: string): HTMLElement {
  return h(
    "div",
    { class: "clash-side" },
    h("span", { class: "clash-name" }, s.name, tag && h("span", { class: "tag" }, tag)),
    h("code", { class: "clash-text" }, s.text),
  );
}

function clashText(c: Clash): string {
  return `${c.kind}${c.pointer ? ` ${c.pointer}` : ""}\n${c.a.name}: ${c.a.text}\n${c.b.name}: ${c.b.text}`;
}

function clashItem(c: Clash, applies: boolean): HTMLElement {
  const done = (msg: string) => {
    toast(msg);
    store.bump();
  };
  const act = (label: string, working: string, fn: () => Promise<string>, primary = false) => {
    const b = h("button", { class: primary ? "primary" : undefined }, label);
    b.addEventListener("click", () => busy(b, working, async () => done(await fn())));
    return b;
  };
  const drop = (keep: ClashSide, other: ClashSide) =>
    act(keepLabel(keep), "Keeping…", async () => {
      await api.dropClaudeMdLine(other.source, other.text);
      return other.source.kind === "own" ? "Taken out of your CLAUDE.md (backed up first)" : `Left out of ${other.name}'s CLAUDE.md on this device`;
    });

  if (c.kind === "setting") {
    return h(
      "li",
      { class: "clash" },
      h("div", { class: "row-title" }, clashTitle(c)),
      sideRow(c.a, applies ? "applies now" : undefined),
      sideRow(c.b, applies ? undefined : "applies now"),
      explainButton("collision", "Explain", () => clashText(c)),
      h("p", { class: "muted small" }, `${c.a.name}'s value is used because it comes first in your group order. To use ${c.b.name}'s instead, move that group up.`),
    );
  }

  const edit = h("button", null, "Edit");
  const editor = h("div", { class: "form", hidden: true });
  edit.addEventListener("click", () => {
    const text = h("textarea", { id: `edit-${c.id}`, rows: 2, class: "mono", spellcheck: "false" });
    text.value = c.a.text;
    const save = h("button", { class: "primary" }, "Save");
    const cancel = h("button", null, "Cancel");
    cancel.addEventListener("click", () => (editor.hidden = true));
    save.addEventListener("click", () =>
      busy(save, "Saving…", async () => {
        await api.editClaudeMdLine(c.a.source, c.a.text, text.value);
        done(`Saved. Everyone in ${c.a.name} gets the change to review.`);
      }),
    );
    replace(
      editor,
      h("label", { for: text.id }, `New text for ${c.a.name}'s line`),
      text,
      h("p", { class: "muted small" }, `This changes the group's CLAUDE.md for everyone in ${c.a.name}. Leave it empty to remove the line.`),
      h("div", { class: "actions" }, save, cancel),
    );
    editor.hidden = false;
    text.focus();
  });
  return h(
    "li",
    { class: "clash" },
    h("div", { class: "row-title" }, clashTitle(c), h("span", { class: `tag ${c.kind === "contradiction" ? "warn" : ""}` }, c.kind === "duplicate" ? "duplicate" : "contradiction")),
    sideRow(c.a),
    sideRow(c.b),
    h("div", { class: "actions" }, drop(c.a, c.b), drop(c.b, c.a), c.a.source.kind === "group" && edit),
    h(
      "p",
      { class: "muted small" },
      c.kind === "duplicate" ? "Claude reads it twice. Keep one: the other is left out on this device only." : "Claude gets both. Keep one: the other is left out on this device only.",
    ),
    explainButton("collision", "Explain", () => clashText(c)),
    editor,
  );
}

/** Rules and settings of group `id` that clash with another group or the user's own text. */
export function conflictsCard(id: GroupId): HTMLElement {
  const title = h("h2", { id: "clash-h" }, "Conflicts");
  const body = h("div", null, spinner("Looking for clashing rules"));
  const card = h("section", { class: "card", "aria-labelledby": "clash-h" }, title, body);
  api.conflicts().then(
    (all) => {
      const mine = clashesOf(all, id);
      if (mine.length === 0) {
        card.remove();
        return;
      }
      replace(title, "Conflicts", h("span", { class: "tag warn" }, String(mine.length)));
      replace(
        body,
        h("p", { class: "muted small" }, "Rules Claude reads from this group and from another group or your own CLAUDE.md that say the same thing or the opposite, and settings two groups set differently."),
        h("ul", { class: "list flat clash-list", "aria-label": "Conflicts" }, mine.map((c) => clashItem(c, all.some((x) => x.id === c.id && isGroup(x.a.source, id))))),
      );
    },
    () => card.remove(),
  );
  return card;
}

function fileChecks(kind: "skill" | "agent", files: CopyFile[], toName: string, picked: Set<string>): HTMLElement {
  return h("div", null, files.map((f) => {
    const box = h("input", { type: "checkbox", id: `copy-${kind}-${f.name}`, disabled: f.inTarget ? true : undefined });
    box.addEventListener("change", () => (box.checked ? picked.add(f.name) : picked.delete(f.name)));
    const note = f.same ? `${toName} has it already` : f.inTarget ? `${toName} has a different ${kind} by this name, so it is left out` : null;
    return h("div", { class: "check" }, box, h("label", { for: box.id }, h("code", null, f.name), note && h("span", { class: "muted small" }, ` ${note}`)));
  }));
}

function copyBody(from: GroupStatus, p: CopyPreview, again: () => void): (HTMLElement | false)[] {
  if (!p.canPush) {
    return [
      h(
        "div",
        { class: "error", role: "alert" },
        `You can't push to ${p.toName}: your GitHub account can read it but not write to it. Ask its host (${p.host}) for write access.`,
      ),
    ];
  }
  const all = sections(p.claudeMd);
  const picked = new Set<string>();
  const text = h("textarea", { id: "copy-md-text", rows: 8, class: "mono", spellcheck: "false" });
  const warnings = h("div");
  const check = checker(warnings, (t) => api.claudeMdCheck(p.to, t, from.id));
  const sync = () => {
    text.value = compose(all, picked);
    check(text.value);
  };
  text.addEventListener("input", () => check(text.value));
  const boxes = new Map<string, HTMLInputElement>();
  const sectionBox = new Map<string, HTMLInputElement>();
  const refreshSections = () => {
    for (const s of all) {
      const ks = keys(all, s.key);
      const on = ks.filter((k) => picked.has(k)).length;
      const box = sectionBox.get(s.key);
      if (box) {
        box.checked = on === ks.length && ks.length > 0;
        box.indeterminate = on > 0 && on < ks.length;
      }
    }
  };
  const mdPicker = all.map((s) => {
    const sbox = h("input", { type: "checkbox", id: `copy-s-${s.key}` });
    sectionBox.set(s.key, sbox);
    sbox.addEventListener("change", () => {
      for (const k of keys(all, s.key)) {
        if (sbox.checked) picked.add(k);
        else picked.delete(k);
        const b = boxes.get(k);
        if (b) b.checked = sbox.checked;
      }
      refreshSections();
      sync();
    });
    return h(
      "li",
      null,
      h("div", { class: "check" }, sbox, h("label", { for: sbox.id }, h("strong", null, s.heading?.replace(/^#+\s*/, "") ?? "Top of the file"))),
      h(
        "ul",
        { class: "pick-list" },
        s.blocks.map((b) => {
          const box = h("input", { type: "checkbox", id: `copy-b-${b.key}` });
          boxes.set(b.key, box);
          box.addEventListener("change", () => {
            if (box.checked) picked.add(b.key);
            else picked.delete(b.key);
            refreshSections();
            sync();
          });
          return h("li", { class: "check" }, box, h("label", { for: box.id, class: "pick-line" }, b.lines.join("\n")));
        }),
      ),
    );
  });

  const skills = new Set<string>();
  const agents = new Set<string>();
  const settings = new Set<string>();
  const settingChecks = p.settings.map((s) => {
    const same = s.target === s.value;
    const box = h("input", { type: "checkbox", id: `copy-set-${s.pointer}`, disabled: s.clash || same ? true : undefined });
    box.addEventListener("change", () => (box.checked ? settings.add(s.pointer) : settings.delete(s.pointer)));
    const note = same ? `${p.toName} has it already` : s.clash ? `${p.toName} sets it to ${s.target}` : s.target ? `merged with ${p.toName}'s ${s.target}` : null;
    return h("div", { class: "check" }, box, h("label", { for: box.id }, h("code", null, `${s.pointer} = ${s.value}`), note && h("span", { class: "muted small" }, ` ${note}`)));
  });

  const copy = h("button", { class: "primary" }, `Copy to ${p.toName}`);
  copy.addEventListener("click", () =>
    busy(copy, "Copying…", async () => {
      const done = await api.copyToGroup(from.id, p.to, { claudeMd: text.value, skills: [...skills], agents: [...agents], settings: [...settings] });
      toast(done.committed ? `Copied to ${p.toName}. Its other members review the CLAUDE.md lines.` : `${p.toName} already has all of it`);
      again();
    }),
  );
  const off = (what: string) => h("p", { class: "muted small" }, `${p.toName} does not sync ${what}.`);
  const nothing = !p.claudeMd && p.skills.length + p.agents.length + p.settings.length === 0;
  return [
    nothing && h("p", { class: "muted small" }, `${from.name} has nothing to copy yet.`),
    !!p.claudeMd && h("h3", null, "CLAUDE.md"),
    !!p.claudeMd && !p.syncs.claudeMd && off("CLAUDE.md"),
    !!p.claudeMd && p.syncs.claudeMd && h("div", { class: "form" }, h("ul", { class: "list flat pick-sections", "aria-label": "Sections and lines to copy" }, mdPicker), h("label", { for: text.id }, `Text ${p.toName} gets, under "From ${from.name}"`), text, warnings),
    p.skills.length > 0 && h("h3", null, "Skills"),
    p.skills.length > 0 && (p.syncs.skills ? fileChecks("skill", p.skills, p.toName, skills) : off("skills")),
    p.agents.length > 0 && h("h3", null, "Agents"),
    p.agents.length > 0 && (p.syncs.agents ? fileChecks("agent", p.agents, p.toName, agents) : off("agents")),
    p.settings.length > 0 && h("h3", null, "Settings"),
    p.settings.length > 0 && (p.syncs.settings ? h("div", null, settingChecks) : off("settings")),
    !nothing && h("p", { class: "muted small" }, "Secrets are redacted and checked before anything is pushed. What the other group already has in another version is refused, never overwritten."),
    !nothing && h("div", { class: "actions" }, copy),
  ];
}

/** "Copy to another group": this group's CLAUDE.md lines, skills, agents and settings. */
export function copyCard(g: GroupStatus, groups: GroupStatus[]): HTMLElement | false {
  const others = groups.filter((x) => x.isHost && !sameGroup(x.id, g.id));
  if (others.length === 0) return false;
  const select = h(
    "select",
    { id: "copy-to" },
    h("option", { value: "" }, "Pick a group"),
    others.map((x, i) => h("option", { value: String(i) }, x.name)),
  );
  const body = h("div");
  const load = () => {
    const to = others[Number(select.value)];
    if (!select.value || !to) {
      replace(body);
      return;
    }
    replace(body, spinner(`Reading ${to.name}`));
    api.copyPreview(g.id, to.id).then(
      (p) => replace(body, copyBody(g, p, load)),
      (e) => replace(body, errorBox(errorText(e), load)),
    );
  };
  select.addEventListener("change", load);
  return h(
    "section",
    { class: "card form", "aria-labelledby": "copy-h" },
    h("h2", { id: "copy-h" }, "Copy to another group"),
    h("p", { class: "muted small" }, `Copy lines of ${g.name}'s CLAUDE.md, its skills, agents and settings to another group you host.`),
    h("label", { for: "copy-to" }, "Group"),
    select,
    body,
  );
}
