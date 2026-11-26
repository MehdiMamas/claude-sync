/** v2-ScreenTray and v2-ScreenTrayQuit. The real menu is native (app/src-tauri/src/tray.rs); this
 * preview only exists in the dev server so the frames can be compared. */
export function TrayPreview({ quit }: { quit: boolean }) {
  if (quit)
    return (
      <div className="w" style={{ background: "var(--bg)" }}>
        <div className="full">
          <p className="muted">claude-sync has quit. Nothing syncs until it starts again.</p>
        </div>
      </div>
    );
  return (
    <div className="w" style={{ background: "var(--bg)" }}>
      <div className="tray">
        <div className="muted">Synced 2 min ago</div>
        <hr />
        <a href="?scenario=groups-syncing#/groups">Sync now</a>
        <a href="?scenario=groups#/groups">Open claude-sync</a>
        <a href="?scenario=groups#/notifications">Notifications (2)</a>
        <a href="?scenario=groups#/settings">Pause syncing</a>
        <a href="?scenario=groups-all-off#/groups">Turn groups off</a>
        <a href="?scenario=groups#/feedback">Send feedback…</a>
        <hr />
        <a href="?view=trayQuit">Quit</a>
      </div>
    </div>
  );
}
