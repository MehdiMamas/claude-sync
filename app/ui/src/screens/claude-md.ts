// "Share my CLAUDE.md": the user's own CLAUDE.md text (without the app's block and group
// imports, secrets redacted) in an editable preview, for a new group or a group they are in.
import { api, errorText, groupKey, type ClaudeMdShare, type GroupId } from "../api";
import { busy, errorBox, h, replace, spinner, toast } from "../dom";
import { checker } from "./conflicts";

const PRIVACY_WARNING =
  "Everyone in the group will see this text, so remove personal details such as emails, names and tokens.";

export interface ClaudeMdEditor {
  el: HTMLElement;
  /** What to share; null when sharing is off or the text is blank. */
  value(): ClaudeMdShare | null;
}

/** With `optional`, a "Share my CLAUDE.md with the group" box turns the preview on and off.
 * With `group`, lines that would clash with that group or the user's other groups are
 * listed under the text (§7b). */
export function claudeMdEditor(id: string, optional: boolean, group?: GroupId): ClaudeMdEditor {
  const share = h("input", { id: `${id}-share`, type: "checkbox", checked: false });
  const text = h("textarea", { id: `${id}-text`, rows: 10, class: "mono", spellcheck: "false", "aria-describedby": `${id}-warn` });
  const removeOwn = h("input", { id: `${id}-remove`, type: "checkbox", checked: true, "aria-describedby": `${id}-remove-hint` });
  const note = h("div", null, spinner("Reading your CLAUDE.md"));
  const warnings = h("div");
  const check = group ? checker(warnings, (t) => api.claudeMdCheck(group, t, null)) : () => {};
  text.addEventListener("input", () => check(text.value));
  const fields = h(
    "div",
    { class: "form", hidden: optional },
    h("label", { for: text.id }, "Text the group gets"),
    h("p", { class: "warn-text small", id: `${id}-warn` }, PRIVACY_WARNING),
    text,
    warnings,
    h(
      "div",
      { class: "check" },
      removeOwn,
      h(
        "label",
        { for: removeOwn.id },
        "Remove it from my own CLAUDE.md (it now comes from the group)",
        h("span", { class: "muted small", id: `${id}-remove-hint` }, " Claude would read it twice otherwise. Your file is backed up first."),
      ),
    ),
  );
  share.addEventListener("change", () => (fields.hidden = !share.checked));
  api.claudeMdPreview().then(
    (t) => {
      text.value = t;
      check(t);
      const empty = t.trim() === "";
      share.checked = !empty;
      fields.hidden = optional && empty;
      replace(note, empty && h("p", { class: "muted small" }, "Your CLAUDE.md has no text of your own yet."));
    },
    (e) => replace(note, errorBox(`Could not read your CLAUDE.md: ${errorText(e)}`)),
  );
  const el = h(
    "div",
    { class: "form" },
    optional && h("div", { class: "check" }, share, h("label", { for: share.id }, "Share my CLAUDE.md with the group")),
    note,
    fields,
  );
  return {
    el,
    value: () => (optional && !share.checked) || text.value.trim() === "" ? null : { text: text.value, removeOwn: removeOwn.checked },
  };
}

/** The group screen's "Share my CLAUDE.md" card: appends the user's text to the group's CLAUDE.md. */
export function claudeMdCard(id: GroupId): HTMLElement {
  const open = h("button", null, "Share my CLAUDE.md…");
  const slot = h("div");
  open.addEventListener("click", () => {
    const editor = claudeMdEditor("share-md", false, id);
    const submit = h("button", { class: "primary" }, "Share");
    const cancel = h("button", null, "Cancel");
    const close = () => {
      slot.replaceChildren();
      open.hidden = false;
    };
    cancel.addEventListener("click", close);
    submit.addEventListener("click", () =>
      busy(submit, "Sharing…", async () => {
        const share = editor.value();
        if (!share) {
          toast("There is no text to share", "error");
          return;
        }
        const done = await api.shareClaudeMd(id, share);
        const removed = done.removedOwn ? " It was taken out of your own CLAUDE.md." : "";
        toast(done.appended ? `Added your text to the CLAUDE.md of ${groupKey(id)}.${removed}` : `The group's CLAUDE.md already has this text.${removed}`);
        close();
      }),
    );
    open.hidden = true;
    replace(slot, editor.el, h("div", { class: "actions" }, submit, cancel));
    editor.el.querySelector("textarea")?.focus();
  });
  return h(
    "section",
    { class: "card form", "aria-labelledby": "share-md-h" },
    h("h2", { id: "share-md-h" }, "Share my CLAUDE.md"),
    h("p", { class: "muted small" }, "Adds your own instructions to the group's CLAUDE.md, under a heading with your GitHub login. You can edit the text first."),
    h("div", null, open),
    slot,
  );
}
