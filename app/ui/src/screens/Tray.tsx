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

/** v2-ScreenInstallUnsigned: the system's own warning before the first start. Not app code; this
 * preview only exists in the dev server so the frame can be compared. */
export function UnsignedPreview() {
  return (
    <div className="w" style={{ background: "var(--bg)" }}>
      <div className="full" style={{ maxWidth: 560, margin: "0 auto", textAlign: "left" }}>
        <h1>Windows protected your PC</h1>
        <p className="muted">
          SmartScreen shows this for a new app without a paid certificate. Choose <b>More info</b>,
          then <b>Run anyway</b>. On macOS, Gatekeeper says the app cannot be checked: right-click
          it, choose <b>Open</b>, then <b>Open</b> again. Each release lists its SHA-256 to compare.
        </p>
        <div className="acts">
          <a className="b p" href="?view=tray">
            Run anyway
          </a>
          <a className="b" href="?view=trayQuit">
            Don't run
          </a>
        </div>
      </div>
    </div>
  );
}
