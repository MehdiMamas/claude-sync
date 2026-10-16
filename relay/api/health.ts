import { json } from "../lib/http.ts";

/** GET /api/health. No auth. */
export async function GET(): Promise<Response> {
  return json(200, { ok: true });
}
