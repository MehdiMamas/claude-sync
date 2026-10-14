// What the sign-in screen says (docs/architecture.md §3). No DOM: `signin.test.mjs` runs
// it in Node.
import type { AuthStatus } from "./api";

export interface SignInCopy {
  heading: string;
  text: string;
  button: string;
}

/** A first sign-in, or a sign-in again after GitHub stopped accepting the old one. */
export function signInCopy(auth: AuthStatus | null): SignInCopy {
  if (auth?.expiredLogin) {
    return {
      heading: "Your GitHub sign-in expired",
      text: `GitHub no longer accepts the sign-in of ${auth.expiredLogin}, so syncing stopped. Sign in again to pick up where you left off; your groups and files on this computer are kept.`,
      button: "Sign in again",
    };
  }
  return {
    heading: "Sign in with GitHub",
    text: "claude-sync keeps your group's Claude Code settings in a private GitHub repo. Sign in with your own GitHub account; nothing is stored outside GitHub and this computer.",
    button: "Sign in with GitHub",
  };
}
