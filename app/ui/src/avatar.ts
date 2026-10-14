// A GitHub avatar: initials on a colour, with the picture laid over them once it loads.
// Offline, blocked or not a plain login: the initials stay. Fixed size, so nothing shifts.
import { avatarUrl, hue, initials } from "./activity";
import { h } from "./dom";

/** Decorative (`aria-hidden`): the login is always written next to it. */
export function avatar(login: string, size = 28): HTMLElement {
  const el = h(
    "span",
    { class: "avatar", style: `--h:${hue(login)};--size:${size}px`, title: login, "aria-hidden": "true" },
    initials(login),
  );
  const url = avatarUrl(login);
  if (url) {
    // src last, so the referrer policy and lazy loading apply to the first request.
    const img = h("img", { alt: "", width: size, height: size, loading: "lazy", decoding: "async", referrerpolicy: "no-referrer", src: url });
    img.addEventListener("load", () => el.classList.add("loaded"), { once: true });
    img.addEventListener("error", () => img.remove(), { once: true });
    el.appendChild(img);
  }
  return el;
}
