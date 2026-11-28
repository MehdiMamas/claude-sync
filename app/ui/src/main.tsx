import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { App } from "./App";
import { type Backend, inTauri, tauriBackend } from "./backend";
import "./style.css";
import { BackendCtx, createStore, StoreCtx } from "./store";

async function backend(): Promise<Backend> {
  if (inTauri()) return tauriBackend();
  // The fake backend is a dev-server feature only: it never reaches the release bundle.
  if (import.meta.env.DEV) return (await import("./fake")).fakeBackend(location.search);
  throw new Error("claude-sync must run inside its app window");
}

async function main() {
  const b = await backend();
  const store = createStore(b, await b.snapshot());
  const view = new URLSearchParams(location.search).get("view");
  let root: React.ReactNode = <App />;
  if (import.meta.env.DEV && view) {
    const { TrayPreview, UnsignedPreview } = await import("./screens/Tray");
    root = view === "unsigned" ? <UnsignedPreview /> : <TrayPreview quit={view === "trayQuit"} />;
  }
  const el = document.getElementById("root");
  if (!el) return;
  createRoot(el).render(
    <StrictMode>
      <BackendCtx.Provider value={b}>
        <StoreCtx.Provider value={store}>{root}</StoreCtx.Provider>
      </BackendCtx.Provider>
    </StrictMode>,
  );
}
void main();
