import type { ReactNode } from "react";
import type { Snapshot } from "../generated/Snapshot";
import { Chip, Logo, Toast } from "../ui";

const items = [
  ["groups", "Groups"],
  ["settings", "Settings"],
  ["feedback", "Feedback"],
  ["notifications", "Notifications"],
] as const;

export const initials = (login: string) => {
  const parts = login.split(/[-_]/).filter(Boolean);
  const i = parts.length > 1 ? parts.map((p) => p[0]).join("") : login.slice(0, 2);
  return i.slice(0, 2).toUpperCase();
};

/** The window frame of every signed-in screen: left nav (188px), content, toast. */
export function Shell(p: {
  snap: Snapshot;
  route: string;
  toast?: string | null;
  children: ReactNode;
}) {
  const section = p.route.split("/")[0];
  return (
    <div className="w">
      <nav className="nav" aria-label="Main">
        <div className="brand">
          <Logo />
          claude-sync
        </div>
        {items.map(([id, label]) => (
          <a
            key={id}
            href={`#/${id}`}
            className={section === id || (id === "groups" && section === "group") ? "on" : ""}
            aria-current={section === id ? "page" : undefined}
          >
            <span>{label}</span>
            {id === "notifications" && p.snap.unread > 0 && <Chip>{p.snap.unread}</Chip>}
          </a>
        ))}
        <div className="me">
          <span className="av">{initials(p.snap.login ?? "")}</span>
          {p.snap.login}
        </div>
      </nav>
      <main className="cnt">{p.children}</main>
      {p.toast && <Toast text={p.toast} />}
    </div>
  );
}

/** Screens that belong to a later UI part: the nav and a plain line, nothing invented. */
export function Later({ title, part }: { title: string; part: string }) {
  return (
    <>
      <h1>{title}</h1>
      <p className="sub">This screen is built in UI part {part}.</p>
    </>
  );
}
