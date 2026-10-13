// Sign in with the GitHub device flow (§3).
import { api, errorText, on, type DeviceCode } from "../api";
import { h, replace, toast } from "../dom";
import { store } from "../store";

let onFailed: (message: string) => void = () => {};
let listening = false;

export function signInScreen(): HTMLElement {
  if (!listening) {
    listening = true;
    on("sign-in-failed", ({ message }) => onFailed(message));
  }
  const box = h("div", { class: "card signin" });
  const page = h(
    "div",
    { class: "center" },
    h("div", { class: "brand big" }, h("span", { class: "logo", "aria-hidden": "true" }, "⟳"), "claude-sync"),
    box,
  );

  const start = () => {
    if (store.auth?.clientIdMissing) {
      replace(
        box,
        h("h1", null, "Sign-in is not available"),
        h(
          "p",
          null,
          "This copy of claude-sync was built without a GitHub client ID (",
          h("code", null, "CLAUDE_SYNC_GITHUB_CLIENT_ID"),
          "), so it cannot sign in. Install an official build, or rebuild with the client ID set.",
        ),
      );
      return;
    }
    const btn = h("button", { class: "primary large" }, "Sign in with GitHub");
    replace(
      box,
      h("h1", null, "Sign in with GitHub"),
      h(
        "p",
        { class: "muted" },
        "claude-sync keeps your group's Claude Code settings in a private GitHub repo. Sign in with your own GitHub account; nothing is stored outside GitHub and this computer.",
      ),
      btn,
    );
    btn.addEventListener("click", async () => {
      btn.disabled = true;
      btn.textContent = "Asking GitHub for a code…";
      try {
        showCode(await api.startSignIn());
      } catch (e) {
        replace(box, h("h1", null, "Sign in with GitHub"), h("div", { class: "error", role: "alert" }, errorText(e)), retry());
      }
    });
  };

  const retry = () => {
    const b = h("button", { class: "primary" }, "Try again");
    b.addEventListener("click", start);
    return b;
  };

  const showCode = (code: DeviceCode) => {
    const copy = h("button", null, "Copy code");
    copy.addEventListener("click", async () => {
      await navigator.clipboard.writeText(code.userCode);
      toast("Code copied");
    });
    const open = h("button", { class: "primary" }, "Open GitHub");
    open.addEventListener("click", () => api.openUrl(code.verificationUri).catch((e) => toast(errorText(e), "error")));
    const cancel = h("button", { class: "link" }, "Cancel");
    cancel.addEventListener("click", async () => {
      await api.cancelSignIn();
      start();
    });
    replace(
      box,
      h("h1", null, "Enter this code on GitHub"),
      h("p", { class: "muted" }, "Your browser opened ", h("code", null, code.verificationUri), ". Paste the code there and approve claude-sync."),
      h("output", { class: "user-code", "aria-label": "Device code", "aria-live": "polite" }, code.userCode),
      h("div", { class: "actions center-row" }, copy, open),
      h("div", { class: "loading", role: "status" }, h("span", { class: "spin", "aria-hidden": "true" }), "Waiting for you to approve on GitHub…"),
      cancel,
    );
  };

  onFailed = (message) => {
    if (message === "cancelled") return;
    replace(box, h("h1", null, "Sign-in did not finish"), h("div", { class: "error", role: "alert" }, message), retry());
  };
  start();
  return page;
}
