// The consent for "Share activity and presence" (docs/architecture.md §22). No DOM:
// `activity-consent.test.mjs` runs it in Node.

export const ACTIVITY_COPY = {
  label: "Share activity and presence",
  heading: "Share activity and presence",
  paragraphs: [
    "When this is on, claude-sync reads one number from your operating system: how many seconds since the last mouse or keyboard input on this machine. It never reads which keys you press, what you click, your screen, your microphone or your camera.",
    "The value stays on this machine for now. It will only ever go to your own machines (the ones signed in to your account), never to your groups, to other people, or to us.",
    "Turn it off here at any time; turning it off deletes the value on this machine at once.",
  ],
  turnOn: "Turn on",
  notNow: "Not now",
  turnOff: "Turn off",
} as const;

/** It starts off, and a machine that never answered stays off. */
export const SHARE_DEFAULT = false;

export type Answer = "turnOn" | "notNow" | "turnOff";

/** Whether sharing is on after the answer. It turns on only after "Turn on". */
export function decide(current: boolean, answer: Answer): boolean {
  if (answer === "turnOn") return true;
  if (answer === "turnOff") return false;
  return current;
}

/** The row's status text. */
export function statusText(share: boolean): string {
  return share ? "On" : "Off";
}
