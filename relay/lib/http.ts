import { redact } from "./redact.ts";

/** An error with an HTTP status. Its message is safe to show to the caller. */
export class HttpError extends Error {
  readonly status: number;
  readonly headers: Record<string, string>;

  constructor(status: number, message: string, headers: Record<string, string> = {}) {
    super(message);
    this.status = status;
    this.headers = headers;
  }
}

export function json(status: number, body: unknown, headers: Record<string, string> = {}): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      ...headers,
    },
  });
}

export function noContent(): Response {
  return new Response(null, { status: 204, headers: { "cache-control": "no-store" } });
}

/** Logs without ever printing a token: every message goes through redaction. */
export function logError(context: string, err: unknown): void {
  const message = err instanceof Error ? err.message : String(err);
  console.error(`[relay] ${context}: ${redact(message)}`);
}

export function logWarn(message: string): void {
  console.warn(`[relay] ${redact(message)}`);
}

/** Wraps a handler so thrown HttpErrors become JSON responses and anything else a 500. */
export function handler(name: string, fn: (req: Request) => Promise<Response>): (req: Request) => Promise<Response> {
  return async (req) => {
    try {
      return await fn(req);
    } catch (err) {
      if (err instanceof HttpError) {
        return json(err.status, { error: err.message }, err.headers);
      }
      logError(name, err);
      return json(500, { error: "internal error" });
    }
  };
}
