import type { NotificationView } from "../generated/NotificationView";
import type { Snapshot } from "../generated/Snapshot";
import { Chip } from "../ui";
import type { Call } from "./Group";

const titles: Record<string, string> = {
  review: "Group CLAUDE.md changed",
  invitation: "Invitation",
  update: "Update ready",
  feedback: "Feedback",
  job: "Job",
  personal: "Personal memory",
  rename: "Group renamed",
  signin: "Sign-in",
  test: "Test notification",
};

/** Where a notification's button goes; none for plain notices. */
const target = (n: NotificationView): [string, string] | null => {
  if (n.kind === "review" && n.repo) return ["Review", `#/group/${n.repo}/review`];
  if (n.kind === "invitation") return ["Open", "#/groups/join"];
  if (n.kind === "update") return ["Open", "#/settings/updates"];
  if (n.kind === "personal") return ["Open", "#/settings/personal"];
  if (n.kind === "job" && n.repo) return ["Open", `#/group/${n.repo}/jobs`];
  if (n.kind === "rename" && n.repo) return ["Open", `#/group/${n.repo}`];
  return null;
};

/** v2-ScreenNotifications and NotificationsEmpty: mark read, mark all read, clear all. */
export function Notifications({ snap, call }: { snap: Snapshot; call: Call }) {
  const list = snap.notifications;
  if (list.length === 0)
    return (
      <>
        <h1>Notifications</h1>
        <div className="card">
          <div style={{ textAlign: "center", padding: "40px 0" }}>
            <h2>All caught up</h2>
            <p className="muted">New invitations, changes and updates show here.</p>
            <a className="b" href="#/groups">
              Back to groups
            </a>
          </div>
        </div>
      </>
    );
  return (
    <>
      <div style={{ display: "flex", justifyContent: "space-between" }}>
        <h1>Notifications</h1>
        <div className="acts">
          <button
            type="button"
            className="b"
            disabled={snap.unread === 0}
            onClick={() => void call({ kind: "markAllRead" })}
          >
            Mark all read
          </button>
          <button
            type="button"
            className="b"
            onClick={() => void call({ kind: "clearNotifications" })}
          >
            Clear all
          </button>
        </div>
      </div>
      <p className="sub">Also shown as desktop notifications when they are on.</p>
      <div className="card">
        {list.map((n) => {
          const t = target(n);
          return (
            <div className="row" key={n.id}>
              <div>
                <b>{titles[n.kind] ?? "Notice"}</b> {!n.read && <Chip>New</Chip>}
                <div className="sm muted">{n.text}</div>
              </div>
              <div className="acts">
                {!n.read && !t && (
                  <button
                    type="button"
                    className="b"
                    onClick={() => void call({ kind: "markRead", data: { id: n.id } })}
                  >
                    Mark read
                  </button>
                )}
                {t && (
                  <a
                    className="b"
                    href={t[1]}
                    onClick={() => void call({ kind: "markRead", data: { id: n.id } })}
                  >
                    {t[0]}
                  </a>
                )}
              </div>
            </div>
          );
        })}
      </div>
    </>
  );
}
