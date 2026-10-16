// Optional Claude helper (docs/architecture.md §19): on-demand "Explain" and "Summarise"
// buttons that appear only when the user saved an Anthropic API key in Settings.
import { api, errorText, type HelperTask } from "../api";
import { busy, h, replace } from "../dom";

let saved: Promise<boolean> | null = null;

/** Whether a key is saved. Cached; Settings calls `forgetHelperState` after a change. */
export function helperSaved(): Promise<boolean> {
  return (saved ??= api.helperStatus().then(
    (s) => s.saved,
    () => false,
  ));
}

export function forgetHelperState(): void {
  saved = null;
}

/** A button that sends `text()` to Claude and shows the answer under it. Hidden without a key.
 * Nothing is sent until the user clicks. */
export function explainButton(task: HelperTask, label: string, text: () => string): HTMLElement {
  const button = h("button", { type: "button" }, label);
  const out = h("div", { class: "helper-out", role: "status", "aria-live": "polite", hidden: true });
  const box = h("div", { class: "helper", hidden: true }, button, out);
  button.addEventListener("click", () =>
    busy(button, "Asking Claude…", async () => {
      out.hidden = false;
      out.classList.remove("error");
      try {
        const answer = await api.helperExplain(task, text());
        replace(out, h("p", { class: "small" }, answer), h("p", { class: "muted small" }, "Written by Claude from the text above. It can be wrong."));
      } catch (e) {
        out.classList.add("error");
        replace(out, h("p", { class: "small" }, errorText(e)));
      }
    }),
  );
  helperSaved().then((ok) => (box.hidden = !ok));
  return box;
}
