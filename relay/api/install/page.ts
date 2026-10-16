import { findInstallers } from "../../lib/assets.ts";
import { env } from "../../lib/env.ts";
import { discard, github, type Release, repoPath } from "../../lib/github.ts";
import { handler } from "../../lib/http.ts";
import { sessionLogin } from "../../lib/session.ts";

const esc = (s: string): string =>
  s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c] ?? c);

const CSS = `:root{color-scheme:light dark;--bg:#fff;--fg:#1a1a1a;--mut:#666;--card:#f4f4f6;--acc:#2f5fe3}
@media(prefers-color-scheme:dark){:root{--bg:#141416;--fg:#ececec;--mut:#9a9a9f;--card:#1f1f23;--acc:#6b8cff}}
*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:16px/1.5 system-ui,sans-serif}
main{max-width:34rem;margin:0 auto;padding:2rem 1rem}h1{font-size:1.6rem;margin:0 0 .25rem}h2{font-size:1.1rem;margin:1.5rem 0 .5rem}
p{margin:.5rem 0}.mut{color:var(--mut)}.card{background:var(--card);border-radius:.75rem;padding:1rem;margin:1rem 0}
.btn{display:block;text-align:center;background:var(--acc);color:#fff;text-decoration:none;font-weight:600;padding:.8rem 1rem;border-radius:.6rem;margin:.6rem 0}
.btn.off{background:var(--card);color:var(--mut)}code{background:var(--card);padding:.1rem .3rem;border-radius:.3rem;word-break:break-all}
ol{padding-left:1.2rem}a{color:var(--acc)}`;

function shell(body: string, status = 200): Response {
  const html = `<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Install claude-sync</title><style>${CSS}</style></head><body><main><h1>claude-sync</h1><p class="mut">Install page</p>${body}</main></body></html>`;
  return new Response(html, {
    status,
    headers: {
      "content-type": "text/html; charset=utf-8",
      "cache-control": "no-store",
      "content-security-policy": "default-src 'none'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'self'",
      "x-content-type-options": "nosniff",
      "referrer-policy": "no-referrer",
    },
  });
}

const ERRORS: Record<string, string> = {
  state: "Sign-in expired or was tampered with. Please try again.",
  denied: "Sign-in was cancelled.",
  github: "GitHub did not complete the sign-in. Please try again.",
};

async function latestRelease(): Promise<Release | null> {
  const res = await github(`${repoPath(env.releasesRepo())}/releases/latest`, { token: env.relayToken() });
  if (!res.ok) {
    discard(res);
    return null;
  }
  const release = (await res.json()) as Release;
  return release.draft ? null : release;
}

function downloads(release: Release | null): string {
  const version = release?.tag_name.replace(/^v/, "") ?? "";
  const found = release ? findInstallers(release.assets, version) : null;
  if (!release || !found) return `<p>No release is available yet. Please check back soon.</p>`;
  const button = (label: string, asset: { id: number } | null): string =>
    asset
      ? `<a class="btn" href="/api/download/${asset.id}">${label}</a>`
      : `<span class="btn off">${label} (not uploaded yet)</span>`;
  return `<p>Latest version: <strong>${esc(version)}</strong></p>
${button("Download for Windows", found.windows)}
${button("Download for macOS", found.macos)}
<div class="card"><h2>First run on macOS</h2>
<p class="mut">The app is not signed by Apple yet, so macOS asks before opening it.</p>
<ol><li>Double-click the downloaded <code>.dmg</code> and drag claude-sync to Applications.</li>
<li>Open it once. When macOS refuses, click <strong>Done</strong>.</li>
<li>Open <strong>System Settings</strong>, then <strong>Privacy &amp; Security</strong>, scroll down and click <strong>Open Anyway</strong>.</li></ol>
<p>Still blocked? Run this in Terminal:</p>
<p><code>xattr -dr com.apple.quarantine /Applications/claude-sync.app</code></p></div>`;
}

/** GET /install: the sign-in and download page. */
export const GET = handler("install", async (req) => {
  if (!env.installConfigured()) {
    return shell(`<div class="card"><p>Install sign-in is not configured on this relay yet.</p><p class="mut">Ask the person who invited you for the installer.</p></div>`, 503);
  }
  const error = ERRORS[new URL(req.url).searchParams.get("error") ?? ""];
  const login = sessionLogin(req);
  if (!login) {
    return shell(`${error ? `<p class="card">${esc(error)}</p>` : ""}<p>Sign in with GitHub to download claude-sync.</p><a class="btn" href="/api/install/login">Sign in with GitHub</a><p class="mut">Only your GitHub username is read.</p>`);
  }
  const out = `<p class="mut">Signed in as <strong>${esc(login)}</strong> · <a href="/api/install/logout">Sign out</a></p>`;
  if (env.blockedUsers().has(login.toLowerCase())) {
    return shell(`${out}<div class="card"><p>Your GitHub account (${esc(login)}) isn't on the list yet. Ask the person who invited you.</p></div>`, 403);
  }
  return shell(`${out}${downloads(await latestRelease())}`);
});
