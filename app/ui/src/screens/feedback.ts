// Send feedback (§12, issue #22): a public GitHub issue opened under the user's account through the relay. The optional logs and screenshot go privately to the maintainer, not into the issue.
import { api, errorText, type FeedbackInput, type Screenshot } from "../api";
import { busy, bytes, errorBox, h, replace, toast } from "../dom";
import { store } from "../store";

const MAX_FILE = 20 * 1024 * 1024;

function toBase64(buf: ArrayBuffer): string {
  const u8 = new Uint8Array(buf);
  let s = "";
  for (let i = 0; i < u8.length; i += 0x8000) s += String.fromCharCode(...u8.subarray(i, i + 0x8000));
  return btoa(s);
}

export function feedbackScreen(view: HTMLElement): void {
  let shot: Screenshot | null = null;

  const kindBug = h("input", { type: "radio", name: "kind", id: "k-bug", value: "bug", checked: true });
  const kindIdea = h("input", { type: "radio", name: "kind", id: "k-idea", value: "idea" });
  const title = h("input", { id: "f-title", required: true, maxlength: 200, autocomplete: "off" });
  const description = h("textarea", { id: "f-desc", rows: 6, required: true });
  // Off by default: the logs can show file paths and repo names, even though only the maintainer sees them.
  const includeLogs = h("input", { id: "f-logs", type: "checkbox", checked: false });
  const logsPre = h("pre", { class: "logs", tabindex: "0", "aria-label": "Recent log lines" }, "Loading…");
  const logsDetails = h("details", null, h("summary", null, "See what is sent"), logsPre);
  logsDetails.addEventListener("toggle", () => {
    if (!logsDetails.open || logsPre.dataset.loaded) return;
    logsPre.dataset.loaded = "1";
    api.feedbackLogs().then(
      (t) => (logsPre.textContent = t || "(no log lines yet)"),
      (e) => (logsPre.textContent = `Logs are not available: ${errorText(e)}`),
    );
  });

  const shotSlot = h("div", { class: "shot" });
  const capture = h("button", { type: "button" }, "Capture screen");
  const fileInput = h("input", { type: "file", accept: "image/png,image/jpeg", class: "sr-only", id: "f-file", tabindex: "-1" });
  const attach = h("button", { type: "button" }, "Attach image");
  const paste = h("button", { type: "button" }, "Paste image");
  const none = h("button", { type: "button", class: "link" }, "No screenshot");

  const showShot = () => {
    none.hidden = !shot;
    capture.textContent = shot ? "Retake" : "Capture screen";
    replace(
      shotSlot,
      shot &&
        h(
          "figure",
          null,
          h("img", { src: `data:${shot.mime};base64,${shot.data}`, alt: "Screenshot to attach" }),
          h("figcaption", { class: "muted small" }, `${shot.width}×${shot.height} · ${bytes(shot.bytes)}`),
        ),
    );
  };
  capture.addEventListener("click", () =>
    busy(capture, "Capturing…", async () => {
      shot = await api.feedbackCaptureScreen();
      showShot();
    }),
  );
  attach.addEventListener("click", () => fileInput.click());
  fileInput.addEventListener("change", async () => {
    const f = fileInput.files?.[0];
    fileInput.value = "";
    if (!f) return;
    if (f.size > MAX_FILE) {
      toast("That image is too big (over 20 MB)", "error");
      return;
    }
    await busy(attach, "Reading…", async () => {
      shot = await api.feedbackAttachFile(toBase64(await f.arrayBuffer()));
      showShot();
    });
  });
  const pasteImage = () =>
    busy(paste, "Pasting…", async () => {
      shot = await api.feedbackPasteImage();
      showShot();
    });
  paste.addEventListener("click", pasteImage);
  none.addEventListener("click", () => {
    shot = null;
    showShot();
  });
  showShot();

  const submit = h("button", { type: "submit", class: "primary" }, "Send");
  const result = h("div", { "aria-live": "polite" });
  const form = h(
    "form",
    { class: "card form" },
    h(
      "fieldset",
      { class: "radios" },
      h("legend", null, "What is it?"),
      h("div", { class: "check" }, kindBug, h("label", { for: "k-bug" }, "Something is wrong")),
      h("div", { class: "check" }, kindIdea, h("label", { for: "k-idea" }, "An idea")),
    ),
    h("label", { for: "f-title" }, "Title"),
    title,
    h("label", { for: "f-desc" }, "What happened, or what would you like?"),
    description,
    h("div", { class: "check" }, includeLogs, h("label", { for: "f-logs" }, "Include recent logs (only the maintainer sees them; tokens are removed, but file paths and repo names can show)")),
    logsDetails,
    h("h2", null, "Screenshot"),
    h("p", { class: "muted small" }, "Only the maintainer sees the screenshot; it is not put in the issue. Capture hides this window for a moment and takes a picture of your main screen. Paste image (or Ctrl+V here) uses the image on the clipboard."),
    h("div", { class: "actions" }, capture, attach, paste, none, fileInput),
    shotSlot,
    h("div", { class: "actions" }, submit),
    result,
  );

  // Ctrl+V of an image anywhere in the form; text still pastes into the fields.
  form.addEventListener("paste", (ev) => {
    const items = [...(ev.clipboardData?.items ?? [])];
    if (items.some((i) => i.type === "text/plain") || !items.some((i) => i.kind === "file" && i.type.startsWith("image/"))) return;
    ev.preventDefault();
    pasteImage();
  });

  form.addEventListener("submit", (ev) => {
    ev.preventDefault();
    const report: FeedbackInput = {
      kind: kindIdea.checked ? "idea" : "bug",
      title: title.value.trim(),
      description: description.value.trim(),
      includeLogs: includeLogs.checked,
      screenshot: shot,
    };
    busy(submit, "Sending…", async () => {
      try {
        const r = await api.feedbackSubmit(report);
        if (r.queued) {
          replace(result, h("div", { class: "notice info", role: "status" }, h("div", { class: "notice-text" }, "The feedback server could not be reached. The report is saved on this computer and is sent automatically later.")));
        } else {
          const link = h("button", { type: "button", class: "link" }, `issue #${r.number ?? "?"}`);
          link.addEventListener("click", () => r.url && api.openUrl(r.url).catch((e) => toast(errorText(e), "error")));
          replace(result, h("div", { class: "notice info", role: "status" }, h("div", { class: "notice-text" }, "Thanks! It is ", link, ".")));
        }
        title.value = "";
        description.value = "";
        shot = null;
        showShot();
      } catch (e) {
        replace(result, errorBox(errorText(e)));
      }
    });
  });

  replace(
    view,
    h("header", { class: "head" }, h("h1", null, "Send feedback")),
    store.info && !store.info.relayConfigured &&
      h("div", { class: "notice", role: "note" }, h("div", { class: "notice-text" }, "This build has no feedback server set up (CLAUDE_SYNC_RELAY_URL). Reports are saved on this computer and sent once a build that has one is installed.")),
    h(
      "div",
      { class: "notice", role: "note" },
      h(
        "div",
        { class: "notice-text" },
        "Your report is opened as an issue under your GitHub account, on the public repo MehdiMamas/claude-sync. Anyone can see its title, the description and your username. The logs (tokens removed) and the screenshot go privately to the maintainer. Claude reads the report and posts a summary on the issue without copying private details from them.",
      ),
    ),
    form,
  );
}
