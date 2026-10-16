// A fetch mock: routes by method + URL, records every call, and fails the test on
// any request it does not know, so no test can reach the network.

export interface Call {
  method: string;
  url: string;
  headers: Record<string, string>;
  body: unknown;
  redirect: RequestInit["redirect"];
}

type Responder = (call: Call) => Response | Promise<Response>;

interface Route {
  method: string;
  match: string | RegExp;
  respond: Responder;
}

export interface FetchMock {
  calls: Call[];
  on(method: string, match: string | RegExp, respond: Responder | Response | number | object): FetchMock;
  restore(): void;
}

const API = "https://api.github.com";

export function mockFetch(): FetchMock {
  const original = globalThis.fetch;
  const routes: Route[] = [];
  const calls: Call[] = [];
  const mock: FetchMock = {
    calls,
    on(method, match, respond) {
      const r: Responder =
        typeof respond === "function"
          ? (respond as Responder)
          : respond instanceof Response
            ? () => respond.clone()
            : typeof respond === "number"
              ? () => new Response(null, { status: respond })
              : () => Response.json(respond);
      // Later routes win, so a test can override a default.
      routes.unshift({ method, match: typeof match === "string" && match.startsWith("/") ? API + match : match, respond: r });
      return mock;
    },
    restore() {
      globalThis.fetch = original;
    },
  };
  globalThis.fetch = (async (input: string | URL | Request, init?: RequestInit) => {
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url;
    const method = (init?.method ?? "GET").toUpperCase();
    const headers: Record<string, string> = {};
    new Headers(init?.headers).forEach((v, k) => (headers[k] = v));
    const body = typeof init?.body === "string" ? JSON.parse(init.body) : undefined;
    const call: Call = { method, url, headers, body, redirect: init?.redirect };
    calls.push(call);
    const route = routes.find(
      (r) => r.method === method && (typeof r.match === "string" ? r.match === url : r.match.test(url)),
    );
    if (!route) throw new Error(`unexpected fetch: ${method} ${url}`);
    return route.respond(call);
  }) as typeof fetch;
  return mock;
}

export const USER_TOKEN = "gho_" + "u".repeat(36);
export const RELAY_TOKEN = "github_pat_" + "r".repeat(60);

export function setEnv(extra: Record<string, string | undefined> = {}): void {
  const base: Record<string, string | undefined> = {
    RELAY_GITHUB_TOKEN: RELAY_TOKEN,
    RELEASES_REPO: "MehdiMamas/claude-sync",
    FEEDBACK_REPO: undefined,
    BLOCKED_GITHUB_USERS: undefined,
    RELAY_PUBLIC_URL: undefined,
    ...extra,
  };
  for (const [k, v] of Object.entries(base)) {
    if (v === undefined) delete process.env[k];
    else process.env[k] = v;
  }
}

/** Routes GET /user to `login` for USER_TOKEN and 401 for anything else. */
export function mockUser(fm: FetchMock, login: string): void {
  fm.on("GET", "/user", (call) =>
    call.headers.authorization === `Bearer ${USER_TOKEN}`
      ? Response.json({ login })
      : Response.json({ message: "Bad credentials" }, { status: 401 }),
  );
}

export function req(path: string, init: RequestInit & { token?: string | null } = {}): Request {
  const headers = new Headers(init.headers);
  const token = init.token === undefined ? USER_TOKEN : init.token;
  if (token !== null) headers.set("authorization", `Bearer ${token}`);
  return new Request(`https://relay.example.test${path}`, { ...init, headers });
}
