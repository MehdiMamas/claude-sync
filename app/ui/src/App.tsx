import { useCallback, useEffect, useRef, useState } from "react";
import type { Command } from "./generated/Command";
import type { Event } from "./generated/Event";
import { GroupPage } from "./screens/Group";
import { type Flash, Groups } from "./screens/Groups";
import { Join, NewGroup } from "./screens/GroupsForms";
import { Later, Shell } from "./screens/Shell";
import { SignIn } from "./screens/SignIn";
import { useBackend, useSnapshot } from "./store";
import { go, useRoute } from "./ui";

const text = (e: unknown) => (e instanceof Error ? e.message : String(e));
const none: Flash = { toast: null, joining: null, joinError: null, fresh: null, created: null };

export function App() {
  const b = useBackend();
  const [hidden, setHidden] = useState<number[]>([]);
  const real = useSnapshot();
  const snap = { ...real, invitations: real.invitations.filter((v) => !hidden.includes(v.id)) };
  const route = useRoute();
  const [flash, setFlash] = useState<Flash>(none);
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000));
  const [approved, setApproved] = useState(false);
  const wasOut = useRef(snap.login === null);
  const patch = useCallback((f: Partial<Flash>) => setFlash((x) => ({ ...x, ...f })), []);

  // Show "Signed in as ..." once when this window saw the sign-in finish.
  useEffect(() => {
    if (snap.login && wasOut.current) setApproved(true);
    wasOut.current = snap.login === null;
  }, [snap.login]);
  useEffect(() => {
    const t = setInterval(() => setNow(Math.floor(Date.now() / 1000)), 30_000);
    return () => clearInterval(t);
  }, []);
  useEffect(() => {
    if (!flash.toast) return;
    const t = setTimeout(() => patch({ toast: null }), 4000);
    return () => clearTimeout(t);
  }, [flash.toast, patch]);
  useEffect(() => {
    if (flash.created && !route.startsWith(`group/${flash.created.repo}`)) patch({ created: null });
  }, [route, flash.created, patch]);
  useEffect(() => void b.send({ kind: "windowOpened" }).catch(() => {}), [b]);
  useEffect(() => {
    if (route === "groups/join") b.send({ kind: "discover" }).catch(() => {});
  }, [route, b]);

  const call = useCallback(
    async (cmd: Command): Promise<{ ev: Event } | { err: string }> => {
      try {
        return { ev: await b.send(cmd) };
      } catch (e) {
        return { err: text(e) };
      }
    },
    [b],
  );

  if (!snap.login || snap.sessionExpired !== null || approved) {
    return (
      <SignIn
        state={snap.signIn}
        expired={snap.sessionExpired !== null}
        login={snap.login && approved ? snap.login : null}
        start={() => void call({ kind: "signInStart", data: { app: false } })}
        cancel={() => void call({ kind: "signInCancel" })}
        openUrl={(u) => void b.openTarget(u)}
        proceed={() => {
          setApproved(false);
          go("groups");
        }}
      />
    );
  }

  const act = {
    syncNow: async () => {
      const r = await call({ kind: "syncNow" });
      if ("err" in r) return patch({ toast: r.err });
      if (r.ev.kind !== "synced") return;
      const files = r.ev.data.files;
      const repos = r.ev.data.repos;
      patch({
        fresh: files ? { repos, files } : null,
        toast: files
          ? `Synced: ${files} files updated${repos.length ? ` in ${repos.join(", ")}` : ""}`
          : "Synced: nothing changed",
      });
    },
    retryPlugins: (repo: string) => void call({ kind: "pluginsRetry", data: { repo } }),
    setOff: (repo: string, off: boolean) => void call({ kind: "setGroupOff", data: { repo, off } }),
    allOn: () =>
      void Promise.all(
        snap.groups.map((g) => call({ kind: "setGroupOff", data: { repo: g.repo, off: false } })),
      ),
    move: async (repo: string, dir: -1 | 1) => {
      const order = snap.groups.map((g) => g.repo);
      const i = order.indexOf(repo);
      let j = i + dir;
      while (snap.groups[j]?.status === "off") j += dir;
      if (j < 0 || j >= order.length) return;
      [order[i], order[j]] = [order[j] as string, order[i] as string];
      const r = await call({ kind: "reorderGroups", data: order });
      patch({ toast: "err" in r ? r.err : "Order saved on all your machines" });
    },
    accept: async (id: number, repo: string) => {
      patch({ joining: repo, joinError: null });
      go("groups");
      const r = await call({ kind: "acceptInvitation", data: { id } });
      patch({
        joining: null,
        joinError: "err" in r ? `Could not join ${repo}: ${r.err}` : null,
        toast: "ev" in r && r.ev.kind === "joined" && r.ev.data.warning ? r.ev.data.warning : null,
      });
    },
    // v1 never declined through GitHub either: Decline hides the invitation in this window only.
    decline: (id: number) => setHidden((h) => [...h, id]),
    join: async (repo: string) => {
      patch({ joining: repo, joinError: null });
      go("groups");
      const r = await call({ kind: "join", data: { repo } });
      patch({
        joining: null,
        joinError: "err" in r ? `Could not join ${repo}: ${r.err}` : null,
      });
    },
  };

  let body: React.ReactNode;
  if (route === "groups") {
    body = <Groups snap={snap} now={now} flash={flash} act={act} />;
  } else if (route === "groups/new") {
    body = (
      <NewGroup
        login={snap.login}
        create={async (name) => {
          const repo = `${snap.login}/${name}`;
          const r = await call({ kind: "createGroup", data: { name, repo } });
          if ("err" in r) return r.err;
          if (r.ev.kind === "created") patch({ created: { repo, lock: r.ev.data.lock } });
          go(`group/${repo}`);
          return null;
        }}
      />
    );
  } else if (route === "groups/join") {
    body = <Join snap={snap} accept={act.accept} decline={act.decline} join={act.join} />;
  } else if (route.startsWith("group/")) {
    const [, owner, name, ...rest] = route.split("/");
    const repo = `${owner}/${name}`;
    body = (
      <GroupPage
        snap={snap}
        g={snap.groups.find((x) => x.repo === repo)}
        now={now}
        flash={flash}
        call={call}
        toast={(toast) => patch({ toast })}
        syncNow={act.syncNow}
        sub={rest[0] === "leave" ? [] : rest}
        leaving={rest[0] === "leave"}
      />
    );
  } else {
    body = <Later title={route[0]?.toUpperCase() + route.slice(1)} part="8c" />;
  }
  return (
    <Shell snap={snap} route={route} toast={flash.toast}>
      {body}
    </Shell>
  );
}
