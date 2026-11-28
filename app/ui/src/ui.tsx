import type { ReactNode } from "react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";

export const Logo = () => (
  <svg aria-hidden="true" width="18" height="18" viewBox="0 0 18 18">
    <circle cx="6" cy="9" r="4.5" fill="none" stroke="var(--accent)" strokeWidth="2" />
    <circle cx="12" cy="9" r="4.5" fill="none" stroke="var(--accent)" strokeWidth="2" />
  </svg>
);

export const Spin = () => <span className="spin" />;

export type Kind = "ok" | "warn" | "err" | "n" | "";
export const Chip = ({ kind = "", children }: { kind?: Kind; children: ReactNode }) => (
  <span className={`chip ${kind}`.trim()}>{children}</span>
);

export const Banner = ({
  kind,
  actions,
  children,
}: {
  kind: "warn" | "err" | "info" | "ok";
  actions?: ReactNode;
  children: ReactNode;
}) => (
  <div className={`ban ${kind}`} role={kind === "err" ? "alert" : "status"}>
    <span>{children}</span>
    <span className="acts">{actions}</span>
  </div>
);

export const Toast = ({ text }: { text: string }) => (
  <div className="toast" role="status">
    {text}
  </div>
);

/** Hash routes: `#/groups`, `#/groups/new`, ... Links are plain anchors, back works, no router library. */
const sub = (f: () => void) => {
  addEventListener("hashchange", f);
  return () => removeEventListener("hashchange", f);
};
export const useRoute = () =>
  useSyncExternalStore(sub, () => location.hash.replace(/^#\/?/, "")) || "groups";
export const go = (route: string) => {
  location.hash = `/${route}`;
};

/** Sends the window to another route as soon as it shows. */
export function Go({ to }: { to: string }) {
  useEffect(() => go(to), [to]);
  return null;
}

/** Runs `load` on mount and whenever `key` changes or the returned `reload` is called. */
export function useLoad<T>(load: () => Promise<T>, key: string) {
  const [data, setData] = useState<T>();
  const [tick, setTick] = useState(0);
  const ref = useRef(load);
  ref.current = load;
  useEffect(() => {
    let on = true;
    ref.current().then(
      (d) => on && setData(d),
      () => {},
    );
    return () => {
      on = false;
    };
  }, [key, tick]);
  return [data, () => setTick((n) => n + 1)] as const;
}
