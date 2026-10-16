import { createHmac, randomBytes, timingSafeEqual } from "node:crypto";
import { env } from "./env.ts";

export const SESSION_COOKIE = "cs_install";
export const STATE_COOKIE = "cs_install_state";
export const SESSION_MS = 24 * 60 * 60 * 1000;
export const STATE_MAX_AGE_S = 10 * 60;

function mac(payload: string, secret: string): string {
  return createHmac("sha256", secret).update(payload).digest("base64url");
}

/** `<base64url json {l: login, e: expiry ms}>.<HMAC-SHA256>` */
export function signSession(login: string, secret: string, now = Date.now()): string {
  const payload = Buffer.from(JSON.stringify({ l: login, e: now + SESSION_MS })).toString("base64url");
  return `${payload}.${mac(payload, secret)}`;
}

/** The login in a correctly signed, unexpired session value, else null. */
export function verifySession(value: string | undefined, secret: string, now = Date.now()): string | null {
  if (!value) return null;
  const dot = value.indexOf(".");
  if (dot < 1) return null;
  const payload = value.slice(0, dot);
  const given = Buffer.from(value.slice(dot + 1));
  const want = Buffer.from(mac(payload, secret));
  if (given.length !== want.length || !timingSafeEqual(given, want)) return null;
  try {
    const data = JSON.parse(Buffer.from(payload, "base64url").toString()) as { l?: unknown; e?: unknown };
    if (typeof data.l !== "string" || typeof data.e !== "number" || data.e <= now) return null;
    return data.l;
  } catch {
    return null;
  }
}

export function newState(): string {
  return randomBytes(24).toString("base64url");
}

export function readCookie(req: Request, name: string): string | undefined {
  for (const part of (req.headers.get("cookie") ?? "").split(";")) {
    const i = part.indexOf("=");
    if (i > 0 && part.slice(0, i).trim() === name) return part.slice(i + 1).trim();
  }
  return undefined;
}

export function setCookie(name: string, value: string, maxAgeS: number, path = "/"): string {
  return `${name}=${value}; Path=${path}; Max-Age=${maxAgeS}; HttpOnly; Secure; SameSite=Lax`;
}

/** The signed-in login from the install session cookie, or null (also null when the secret is unset). */
export function sessionLogin(req: Request): string | null {
  const secret = env.installSessionSecret();
  return secret ? verifySession(readCookie(req, SESSION_COOKIE), secret) : null;
}
