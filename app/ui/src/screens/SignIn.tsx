import { createContext, useContext, useState } from "react";
import type { InstallNote } from "../generated/InstallNote";
import type { SignInState } from "../generated/SignInState";
import { Banner, Logo, Spin, Toast } from "../ui";

type Props = {
  state: SignInState;
  expired: boolean;
  login: string | null;
  start: () => void;
  cancel: () => void;
  openUrl: (u: string) => void;
  proceed: () => void;
  install: InstallNote | null;
  /** Feedback needs a token, so from here it only explains. */
  toast: string | null;
  note: (t: string) => void;
  retry: () => void;
};

const Said = createContext<string | null>(null);

const Card = ({ children, toast }: { children: React.ReactNode; toast?: string }) => {
  const said = useContext(Said);
  return (
    <div className="w">
      <div className="full">
        <div className="card" style={{ width: 420, padding: "28px 30px", textAlign: "left" }}>
          <div className="brand" style={{ padding: "0 0 14px" }}>
            <Logo />
            claude-sync
          </div>
          {children}
        </div>
      </div>
      {(toast ?? said) && <Toast text={(toast ?? said) as string} />}
    </div>
  );
};

const Start = ({
  label,
  onClick,
  busy,
}: {
  label: string;
  onClick: () => void;
  busy?: boolean;
}) => (
  <button type="button" className="b p" onClick={onClick} disabled={busy}>
    {label}
  </button>
);

const Mono = ({ children }: { children: string }) => (
  <span style={{ fontFamily: "var(--font-mono)" }}>{children}</span>
);

const Log = ({ note }: { note: (t: string) => void }) => (
  <button
    type="button"
    className="b g"
    onClick={() => note("Sign in first: feedback is filed under your GitHub account.")}
  >
    Send feedback with the log
  </button>
);

/** v2-ScreenInstallV1Daemon: the old daemon did not stop, nothing was imported. */
function Daemon({
  i,
  retry,
  note,
}: {
  i: Extract<InstallNote, { kind: "daemon" }>;
  retry: () => void;
  note: (t: string) => void;
}) {
  return (
    <Card>
      <Banner kind="warn">
        An old claude-sync-cli {i.version} daemon (process {i.pid}) is still running. It was asked
        to stop and did not within 30 seconds, so nothing was imported or changed yet.
      </Banner>
      <h1>Finish the switch to v2</h1>
      <p className="sub">Stop it, then try again. Your v1 files are untouched.</p>
      <p className="sub">
        Linux with the service: <Mono>systemctl --user restart claude-sync</Mono>
        <br />
        Linux without systemd, and macOS (cron): <Mono>{`kill ${i.pid}`}</Mono>
        <br />
        Windows: <Mono>{`taskkill /PID ${i.pid} /F`}</Mono>
      </p>
      <Start label="Try again" onClick={retry} />
      <Log note={note} />
    </Card>
  );
}

/** Every v2-ScreenSignIn* and v2-ScreenInstall* frame: the shell's `signIn` state picks one. */
export const SignIn = (p: Props) => (
  <Said.Provider value={p.toast}>
    <SignInBody {...p} />
  </Said.Provider>
);

function SignInBody(p: Props) {
  const [copied, setCopied] = useState(false);
  const s = p.state;
  if (p.login) {
    return (
      <Card>
        <h1>Signed in as {p.login}</h1>
        <p className="sub">
          <Spin />
          Loading your groups…
        </p>
        <button type="button" className="b p" onClick={p.proceed}>
          Continue
        </button>
      </Card>
    );
  }
  if (p.install?.kind === "daemon" && s.kind !== "code")
    return <Daemon i={p.install} retry={p.retry} note={p.note} />;
  if (p.expired && s.kind !== "code" && s.kind !== "starting") {
    return (
      <Card>
        <Banner kind="warn">
          GitHub no longer accepts your sign-in. Syncing is stopped until you sign in again.
        </Banner>
        <h1>Sign in again</h1>
        <p className="sub">Your groups and files stay as they are.</p>
        <Start label="Sign in with GitHub" onClick={p.start} />
      </Card>
    );
  }
  switch (s.kind) {
    case "code": {
      const copy = () => {
        navigator.clipboard?.writeText(s.user_code).catch(() => {});
        setCopied(true);
      };
      return (
        <Card toast={copied ? "Code copied" : undefined}>
          <h1>Enter this code on GitHub</h1>
          <p className="sub">github.com/login/device opened in your browser.</p>
          <div className="code">{s.user_code}</div>
          <div className="acts" style={{ margin: "16px 0 0" }}>
            <button type="button" className="b" onClick={copy}>
              {copied ? "Copied" : "Copy code"}
            </button>
            <button type="button" className="b" onClick={() => p.openUrl(s.verification_uri)}>
              Open GitHub again
            </button>
            <button type="button" className="b g" onClick={p.cancel}>
              Cancel
            </button>
          </div>
          <p className="sm muted" style={{ margin: "14px 0 0" }}>
            <Spin />
            Waiting for you to approve.
            {copied ? "" : ` The code works for ${Math.round(s.expires_in / 60)} minutes.`}
          </p>
        </Card>
      );
    }
    case "expired":
      return (
        <Card>
          <Banner kind="err">The code expired before it was entered.</Banner>
          <h1>Start again</h1>
          <p className="sub">A new code takes a second.</p>
          <Start label="Get a new code" onClick={p.start} />
        </Card>
      );
    case "denied":
      return (
        <Card>
          <Banner kind="err">You chose Cancel on GitHub. Nothing was stored.</Banner>
          <h1>Sign in again?</h1>
          <p className="sub">claude-sync needs the repo scope to read your private group repos.</p>
          <Start label="Start again" onClick={p.start} />
        </Card>
      );
    case "offline":
      return (
        <Card>
          <Banner kind="err">Cannot reach github.com. Check the connection.</Banner>
          <h1>Share your Claude Code setup</h1>
          <Start label="Try again" onClick={p.start} />
        </Card>
      );
    case "unavailable":
      return (
        <Card>
          <Banner kind="err">Sign-in is not available in this build (no GitHub client id).</Banner>
          <h1>Use a release build</h1>
          <p className="sub">Download claude-sync from its GitHub Releases page.</p>
        </Card>
      );
    default: {
      const skipped = p.install?.kind === "skipped" ? p.install : null;
      return (
        <Card>
          {skipped && (
            <Banner kind="warn">
              Could not read {skipped.files.length} of {skipped.total} old v1 files (
              {skipped.files.join(", ")}). {skipped.files.length === 1 ? "It was" : "They were"}{" "}
              skipped and kept in ~/.claude-sync/backup/; nothing was deleted.
            </Banner>
          )}
          {s.kind === "failed" && <Banner kind="err">{s.message}</Banner>}
          <h1>Share your Claude Code setup</h1>
          <p className="sub">
            {skipped
              ? "Sign in to continue. Skipped jobs show again after the first sync and ask for your yes."
              : "Sign in with GitHub. claude-sync keeps your token in the system keychain and never sees a password."}
          </p>
          <Start
            label={s.kind === "starting" ? "Starting…" : "Sign in with GitHub"}
            onClick={p.start}
            busy={s.kind === "starting"}
          />
          {skipped ? (
            <Log note={p.note} />
          ) : (
            <p className="sm muted" style={{ margin: "14px 0 0" }}>
              Needs a free GitHub account.
            </p>
          )}
        </Card>
      );
    }
  }
}
