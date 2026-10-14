// A local stand-in for Vercel and GitHub, for the self-update proof (#27) and the
// end-to-end test: the relay's handlers on a real HTTP server, a stub GitHub REST API on
// another, and a stub asset host on a third (GitHub's signed asset URLs are on a
// different origin, so fetch drops the Authorization header on the way there).
// Nothing here reaches the network: only https://api.github.com is rerouted, to the stub.

import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import type { AddressInfo } from "node:net";
import { GET as download } from "../api/download/[id].ts";
import { GET as health } from "../api/health.ts";
import { GET as update } from "../api/update.ts";
import { GITHUB_API } from "../lib/github.ts";

export interface StubAsset {
  name: string;
  bytes: Uint8Array;
}

export interface StubRelease {
  repo: string;
  tag: string;
  body: string;
  published_at: string;
  draft?: boolean;
  assets: StubAsset[];
}

export interface StubOptions {
  release: StubRelease | null;
  /** The relay's own token (RELAY_GITHUB_TOKEN); release calls without it get 401. */
  relayToken: string;
  /** User token to login, for GET /user. */
  users: Record<string, string>;
}

export interface Running {
  url: string;
  close(): Promise<void>;
}

export interface StubGitHub extends Running {
  objectsUrl: string;
  /** Every request either stub saw, as "METHOD path". */
  log: string[];
}

export const FIRST_ASSET_ID = 5000;

function listen(server: Server): Promise<string> {
  return new Promise((resolve) => {
    server.listen(0, "127.0.0.1", () => resolve(`http://127.0.0.1:${(server.address() as AddressInfo).port}`));
  });
}

function closer(server: Server): () => Promise<void> {
  return () =>
    new Promise((resolve) => {
      server.closeAllConnections();
      server.close(() => resolve());
    });
}

function send(res: ServerResponse, status: number, body?: unknown, headers: Record<string, string> = {}): void {
  if (body === undefined) {
    res.writeHead(status, headers).end();
  } else if (body instanceof Uint8Array) {
    res.writeHead(status, { "content-type": "application/octet-stream", "content-length": String(body.length), ...headers });
    res.end(body);
  } else {
    res.writeHead(status, { "content-type": "application/json", ...headers }).end(JSON.stringify(body));
  }
}

/** A stub of the few GitHub REST calls /api/update and /api/download make. */
export async function startStubGitHub(opts: StubOptions): Promise<StubGitHub> {
  const log: string[] = [];
  const assets = (opts.release?.assets ?? []).map((a, i) => ({ id: FIRST_ASSET_ID + i, ...a }));

  // GitHub's asset host: needs no auth, and like S3 refuses a request that carries some.
  const objects = createServer((req, res) => {
    log.push(`objects ${req.method} ${req.url}`);
    const m = /^\/assets\/(\d+)\?sig=stub$/.exec(req.url ?? "");
    const asset = m && assets.find((a) => a.id === Number(m[1]));
    if (req.headers.authorization) return send(res, 400, { message: "only one auth mechanism allowed" });
    if (!asset) return send(res, 404, { message: "Not Found" });
    send(res, 200, asset.bytes);
  });
  const objectsUrl = await listen(objects);

  const api = createServer((req: IncomingMessage, res) => {
    const path = req.url ?? "";
    log.push(`api ${req.method} ${path}`);
    const auth = req.headers.authorization ?? "";
    const token = /^Bearer (\S+)$/.exec(auth)?.[1] ?? "";
    if (req.method !== "GET") return send(res, 404, { message: "Not Found" });

    if (path === "/user") {
      const login = opts.users[token];
      return login ? send(res, 200, { login }) : send(res, 401, { message: "Bad credentials" });
    }
    const rel = opts.release;
    const base = rel ? `/repos/${rel.repo}` : null;
    if (token !== opts.relayToken) return send(res, 401, { message: "Bad credentials" });
    if (!rel || !base) return send(res, 404, { message: "Not Found" });

    // /releases/latest skips drafts, like GitHub.
    if (path === `${base}/releases/latest`) {
      if (rel.draft) return send(res, 404, { message: "Not Found" });
      return send(res, 200, {
        tag_name: rel.tag,
        name: `claude-sync ${rel.tag}`,
        body: rel.body,
        draft: false,
        prerelease: false,
        published_at: rel.published_at,
        assets: assets.map((a) => ({ id: a.id, name: a.name, size: a.bytes.length })),
      });
    }
    const m = new RegExp(`^${base.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}/releases/assets/(\\d+)$`).exec(path);
    if (m) {
      const asset = assets.find((a) => a.id === Number(m[1]));
      if (!asset) return send(res, 404, { message: "Not Found" });
      if (req.headers.accept === "application/octet-stream") {
        return send(res, 302, undefined, { location: `${objectsUrl}/assets/${asset.id}?sig=stub` });
      }
      return send(res, 200, { id: asset.id, name: asset.name, size: asset.bytes.length });
    }
    send(res, 404, { message: "Not Found" });
  });
  const url = await listen(api);
  const closeApi = closer(api);
  const closeObjects = closer(objects);
  return { url, objectsUrl, log, close: async () => void (await Promise.all([closeApi(), closeObjects()])) };
}

/** Sends the relay's GitHub calls to `apiUrl` instead of https://api.github.com. Returns the undo. */
export function routeGitHubTo(apiUrl: string): () => void {
  const original = globalThis.fetch;
  globalThis.fetch = ((input: string | URL | Request, init?: RequestInit) => {
    const href = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    if (href.startsWith(`${GITHUB_API}/`)) {
      if (input instanceof Request) return original(new Request(apiUrl + href.slice(GITHUB_API.length), input), init);
      return original(apiUrl + href.slice(GITHUB_API.length), init);
    }
    return original(input, init);
  }) as typeof fetch;
  return () => {
    globalThis.fetch = original;
  };
}

/** The relay's GET endpoints on a local HTTP server, as Vercel would route them. */
export async function startRelay(): Promise<Running> {
  const server = createServer(async (req, res) => {
    const origin = `http://${req.headers.host ?? "127.0.0.1"}`;
    const url = new URL(req.url ?? "/", origin);
    const headers = new Headers();
    for (const [k, v] of Object.entries(req.headers)) if (typeof v === "string") headers.set(k, v);
    const request = new Request(url, { method: req.method, headers });
    let response: Response;
    if (req.method !== "GET") response = Response.json({ error: "method not allowed" }, { status: 405 });
    else if (url.pathname === "/api/health") response = await health();
    else if (url.pathname === "/api/update") response = await update(request);
    else if (/^\/api\/download\/[^/]+$/.test(url.pathname)) response = await download(request);
    else response = Response.json({ error: "not found" }, { status: 404 });
    const out: Record<string, string> = {};
    response.headers.forEach((v, k) => (out[k] = v));
    res.writeHead(response.status, out);
    res.end(Buffer.from(await response.arrayBuffer()));
  });
  const url = await listen(server);
  return { url, close: closer(server) };
}
