// Tiny DOM helpers: no framework, just elements.

type Child = Node | string | number | false | null | undefined | Child[];
type Attrs = Record<string, unknown>;

/** `h("button", { class: "primary", onclick }, "Save")` */
export function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  attrs: Attrs | null = null,
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs ?? {})) {
    if (v === undefined || v === null || v === false) continue;
    if (k.startsWith("on") && typeof v === "function") {
      el.addEventListener(k.slice(2), v as EventListener);
    } else if (k === "class") {
      el.className = String(v);
    } else if (k === "value" && "value" in el) {
      (el as HTMLInputElement).value = String(v);
    } else if (k === "checked" && el instanceof HTMLInputElement) {
      el.checked = Boolean(v);
    } else if (v === true) {
      el.setAttribute(k, "");
    } else {
      el.setAttribute(k, String(v));
    }
  }
  append(el, children);
  return el;
}

function append(el: Node, children: Child[]): void {
  for (const c of children) {
    if (c === null || c === undefined || c === false) continue;
    if (Array.isArray(c)) append(el, c);
    else el.appendChild(typeof c === "string" || typeof c === "number" ? document.createTextNode(String(c)) : c);
  }
}

export function replace(el: Element, ...children: Child[]): void {
  el.replaceChildren();
  append(el, children);
}

export function relTime(unix: number | null | undefined): string {
  if (!unix) return "never";
  const s = Math.round(Date.now() / 1000 - unix);
  if (s < 45) return "just now";
  if (s < 3600) return `${Math.round(s / 60)} min ago`;
  if (s < 86400) return `${Math.round(s / 3600)} h ago`;
  return new Date(unix * 1000).toLocaleString();
}

export function bytes(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  return `${(n / 1024 / 1024).toFixed(1)} MB`;
}

export function errorBox(message: string, retry?: () => void): HTMLElement {
  return h(
    "div",
    { class: "error", role: "alert" },
    h("span", null, message),
    retry && h("button", { class: "link", onclick: retry }, "Try again"),
  );
}

export function spinner(label = "Loading"): HTMLElement {
  return h("div", { class: "loading", role: "status" }, h("span", { class: "spin", "aria-hidden": "true" }), label);
}

let toastTimer: number | undefined;
export function toast(message: string, kind: "ok" | "error" = "ok"): void {
  let el = document.getElementById("toast");
  if (!el) {
    el = h("div", { id: "toast", role: "status" });
    document.body.appendChild(el);
  }
  el.textContent = message;
  el.className = `toast show ${kind}`;
  window.clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => el?.classList.remove("show"), 3500);
}

/** Run an async action with the button disabled; show errors as a toast. */
export async function busy<T>(btn: HTMLButtonElement, label: string, fn: () => Promise<T>): Promise<T | undefined> {
  const old = btn.textContent;
  btn.disabled = true;
  btn.textContent = label;
  try {
    return await fn();
  } catch (e) {
    toast(typeof e === "string" ? e : String(e), "error");
    return undefined;
  } finally {
    btn.disabled = false;
    btn.textContent = old;
  }
}

/** A button that asks for a second click before doing something drastic. */
export function confirmButton(label: string, confirmLabel: string, fn: (btn: HTMLButtonElement) => void): HTMLButtonElement {
  let armed = false;
  let timer: number | undefined;
  const btn = h("button", { class: "danger" }, label);
  btn.addEventListener("click", () => {
    if (!armed) {
      armed = true;
      btn.textContent = confirmLabel;
      timer = window.setTimeout(() => {
        armed = false;
        btn.textContent = label;
      }, 4000);
      return;
    }
    window.clearTimeout(timer);
    armed = false;
    btn.textContent = label;
    fn(btn);
  });
  return btn;
}
