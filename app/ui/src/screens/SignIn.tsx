import { useState } from "react";
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
};

const Card = ({ children, toast }: { children: React.ReactNode; toast?: string }) => (
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
    {toast && <Toast text={toast} />}
  </div>
);

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

/** Every v2-ScreenSignIn* frame: the shell's `signIn` state picks one. */
export function SignIn(p: Props) {
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
    default:
      return (
        <Card>
          {s.kind === "failed" && <Banner kind="err">{s.message}</Banner>}
          <h1>Share your Claude Code setup</h1>
          <p className="sub">
            Sign in with GitHub. claude-sync keeps your token in the system keychain and never sees
            a password.
          </p>
          <Start
            label={s.kind === "starting" ? "Starting…" : "Sign in with GitHub"}
            onClick={p.start}
            busy={s.kind === "starting"}
          />
          <p className="sm muted" style={{ margin: "14px 0 0" }}>
            Needs a free GitHub account.
          </p>
        </Card>
      );
  }
}
