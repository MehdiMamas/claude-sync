import { createContext, useContext, useSyncExternalStore } from "react";
import type { Backend } from "./backend";
import type { Snapshot } from "./generated/Snapshot";

/** A tiny store over the backend's snapshot event; the window never mutates state itself. */
export function createStore(backend: Backend, first: Snapshot) {
  let snap = first;
  const subs = new Set<() => void>();
  backend.subscribe((s) => {
    snap = s;
    for (const f of subs) f();
  });
  return {
    get: () => snap,
    sub(f: () => void) {
      subs.add(f);
      return () => void subs.delete(f);
    },
  };
}
export type Store = ReturnType<typeof createStore>;

export const BackendCtx = createContext<Backend | null>(null);
export const StoreCtx = createContext<Store | null>(null);

export function useBackend(): Backend {
  const b = useContext(BackendCtx);
  if (!b) throw new Error("no backend");
  return b;
}
export function useSnapshot(): Snapshot {
  const s = useContext(StoreCtx);
  if (!s) throw new Error("no store");
  return useSyncExternalStore(s.sub, s.get);
}
