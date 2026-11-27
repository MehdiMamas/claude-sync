import { useState } from "react";
import pkg from "../../package.json";
import type { FeedbackInput } from "../generated/FeedbackInput";
import { Banner } from "../ui";
import type { Call } from "./Group";
import { Toggle } from "./GroupMembers";

type Outcome =
  | { kind: "sent"; number: number | null }
  | { kind: "queued" }
  | { kind: "limited" }
  | { kind: "refused"; text: string }
  | { kind: "large" };

const os = /Win/.test(navigator.platform)
  ? "Windows"
  : /Mac/.test(navigator.platform)
    ? "macOS"
    : "Linux";

/** The relay says "too large" for a 413 and the engine says "larger than 3 MB" for a screenshot. */
const isLarge = (t: string) => /too large|larger than/i.test(t);

const base64 = (b: ArrayBuffer) => {
  let s = "";
  const bytes = new Uint8Array(b);
  for (let i = 0; i < bytes.length; i += 0x8000)
    s += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(s);
};

/**
 * v2-ScreenFeedback, FeedbackEmpty, FeedbackSent, FeedbackQueued, FeedbackRateLimited,
 * FeedbackRefused and FeedbackTooLarge. The form keeps its text through every failure.
 */
export function Feedback(c: { call: Call; toast: (t: string) => void }) {
  const [kind, setKind] = useState<"bug" | "idea">("bug");
  const [title, setTitle] = useState("");
  const [text, setText] = useState("");
  const [logs, setLogs] = useState(true);
  const [shot, setShot] = useState<string | null>(null);
  const [empty, setEmpty] = useState(false);
  const [busy, setBusy] = useState(false);
  const [out, setOut] = useState<Outcome | null>(null);

  const reset = () => {
    setTitle("");
    setText("");
    setShot(null);
    setOut(null);
  };
  const paste = async () => {
    try {
      for (const item of await navigator.clipboard.read()) {
        const type = item.types.find((t) => t === "image/png" || t === "image/jpeg");
        if (type) return setShot(base64(await (await item.getType(type)).arrayBuffer()));
      }
      c.toast("No image on the clipboard");
    } catch {
      c.toast("Could not read the clipboard");
    }
  };
  const capture = async () => {
    try {
      const stream = await navigator.mediaDevices.getDisplayMedia({ video: true });
      const v = document.createElement("video");
      v.srcObject = stream;
      await v.play();
      const cv = document.createElement("canvas");
      cv.width = v.videoWidth;
      cv.height = v.videoHeight;
      cv.getContext("2d")?.drawImage(v, 0, 0);
      for (const t of stream.getTracks()) t.stop();
      setShot(cv.toDataURL("image/png").split(",")[1] ?? null);
    } catch {
      c.toast("Capture is not available here. Copy a screenshot and choose Paste.");
    }
  };
  const send = async () => {
    const body = text.trim();
    if (!body) return setEmpty(true);
    setEmpty(false);
    setBusy(true);
    const input: FeedbackInput = {
      kind,
      title: title.trim() || body.split("\n")[0]?.slice(0, 80) || body,
      description: body,
      appVersion: pkg.version,
      os,
      // gap: the shell has no log command yet, so nothing is attached until it has one
      logs: "",
      screenshot: shot,
    };
    const r = await c.call({ kind: "sendFeedback", data: input });
    setBusy(false);
    if ("err" in r) setOut(isLarge(r.err) ? { kind: "large" } : { kind: "refused", text: r.err });
    else if (r.ev.kind !== "feedback") setOut({ kind: "sent", number: null });
    else if (!r.ev.data.queued) setOut({ kind: "sent", number: r.ev.data.number });
    else setOut({ kind: r.ev.data.limited ? "limited" : "queued" });
  };

  if (out) {
    const again = (label: string, f: () => void) => (
      <button type="button" className="b" onClick={f}>
        {label}
      </button>
    );
    return (
      <>
        <h1>Send feedback</h1>
        {out.kind === "sent" && (
          <Banner kind="ok" actions={again("Send another", reset)}>
            {out.number ? `Sent as issue #${out.number}.` : "Sent."} You will get a reply there.
          </Banner>
        )}
        {out.kind === "queued" && (
          <Banner kind="warn" actions={again("Write another", reset)}>
            You are offline. Your report is saved and is sent when the connection is back.
          </Banner>
        )}
        {out.kind === "limited" && (
          <Banner kind="warn" actions={again("Write another", reset)}>
            Too many reports right now (429, from GitHub or the relay limit per user). Your report
            is saved and sent later.
          </Banner>
        )}
        {out.kind === "refused" && (
          <Banner
            kind="err"
            actions={
              <>
                {again("Try again", () => {
                  setOut(null);
                  void send();
                })}
                {again("Copy text", () => {
                  navigator.clipboard?.writeText(text).catch(() => {});
                  c.toast("Text copied");
                })}
              </>
            }
          >
            Feedback not sent: {out.text}. Your text is kept.
          </Banner>
        )}
        {out.kind === "large" && (
          <Banner kind="err" actions={again("Edit report", () => setOut(null))}>
            The report is too large (413). Remove the screenshot or the logs and send again.
          </Banner>
        )}
      </>
    );
  }
  return (
    <>
      <h1>Send feedback</h1>
      <p className="sub">
        Files an issue under your GitHub account. Logs leave out tokens; paths and repo names can
        show.
      </p>
      <div className="card">
        <div className="row">
          <div>Kind</div>
          <div className="acts">
            {(["bug", "idea"] as const).map((k) => (
              <button
                type="button"
                key={k}
                className={`chip chipb${kind === k ? " ok" : ""}`}
                aria-pressed={kind === k}
                onClick={() => setKind(k)}
              >
                {k === "bug" ? "Bug" : "Idea"}
              </button>
            ))}
          </div>
        </div>
        <label className="fld">
          <span>Title</span>
          <input className="in" value={title} onChange={(e) => setTitle(e.target.value)} />
        </label>
        <label className="fld">
          <span>What happened</span>
          <textarea
            className={`in${empty ? " bad" : ""}`}
            style={{ minHeight: empty ? 84 : 64, resize: "none", font: "inherit" }}
            value={text}
            onChange={(e) => {
              setText(e.target.value);
              setEmpty(false);
            }}
          />
          {empty && <div className="err">Write a few words first.</div>}
        </label>
        <div className="row">
          <div>Attach recent logs (last 200 KiB)</div>
          <div className="acts">
            <Toggle on={logs} label={logs ? "Turn off" : "Turn on"} set={() => setLogs(!logs)} />
          </div>
        </div>
        <div className="row">
          <div>
            {shot
              ? `Screenshot attached (${Math.ceil((shot.length * 3) / 4 / 1024)} KiB)`
              : "Screenshot: capture this window or paste an image"}
          </div>
          <div className="acts">
            {shot ? (
              <button type="button" className="b" onClick={() => setShot(null)}>
                Remove
              </button>
            ) : (
              <>
                <button type="button" className="b" onClick={() => void capture()}>
                  Capture
                </button>
                <button type="button" className="b" onClick={() => void paste()}>
                  Paste
                </button>
              </>
            )}
          </div>
        </div>
        <div className="acts" style={{ marginTop: 8 }}>
          <button type="button" className="b p" disabled={busy} onClick={() => void send()}>
            {busy ? "Sending…" : "Send"}
          </button>
        </div>
      </div>
    </>
  );
}
