//! The actor: one task owns [`Core`] and runs commands one at a time on a blocking thread
//! (the engine, store and GitHub client are synchronous). The loop wakes every second, asks
//! the schedule what is due and publishes a fresh snapshot after every change.
use crate::core::{Core, Env};
use crate::error::{Error, Result};
use crate::model::{Command, Event, Snapshot};
use std::time::Duration;
use tokio::sync::{mpsc, oneshot, watch};

pub struct Msg {
    pub cmd: Command,
    pub reply: Option<oneshot::Sender<Result<Event>>>,
}

#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Msg>,
    snap: watch::Receiver<Snapshot>,
}

impl Handle {
    pub async fn send(&self, cmd: Command) -> Result<Event> {
        let (r, rx) = oneshot::channel();
        self.tx
            .send(Msg {
                cmd,
                reply: Some(r),
            })
            .await
            .map_err(|_| Error::new("claude-sync is shutting down"))?;
        rx.await
            .map_err(|_| Error::new("claude-sync is shutting down"))?
    }

    /// The latest read-only snapshot; the window and tray never touch state themselves.
    pub fn snapshot(&self) -> Snapshot {
        self.snap.borrow().clone()
    }

    pub fn watch(&self) -> watch::Receiver<Snapshot> {
        self.snap.clone()
    }
}

/// Open the state and start the actor. `tick` is how often the loop checks the schedule.
pub async fn spawn(env: Env, tick: Duration) -> Result<Handle> {
    spawn_with(env, tick, None).await
}

/// [`spawn`] with the machine side of the jobs replaced (tests).
pub async fn spawn_with(
    env: Env,
    tick: Duration,
    jobs: Option<crate::jobs::JobsEnv>,
) -> Result<Handle> {
    // the blocking GitHub client must be built and dropped off the async threads
    let mut core = tokio::task::spawn_blocking(move || {
        Core::open(env).map(|mut c| {
            if let Some(j) = jobs {
                c.set_jobs_env(j);
            }
            c
        })
    })
    .await
    .map_err(|_| Error::new("could not start"))??;
    let (tx, mut rx) = mpsc::channel::<Msg>(32);
    core.tx = Some(tx.downgrade());
    let (stx, srx) = watch::channel(core.snapshot());
    core.publish = Some(stx.clone());
    tokio::spawn(async move {
        let mut core = Some(core);
        let mut timer = tokio::time::interval(tick);
        loop {
            let (cmd, reply) = tokio::select! {
                m = rx.recv() => match m {
                    Some(m) => (Some(m.cmd), m.reply),
                    None => break,
                },
                _ = timer.tick() => (None, None),
            };
            let Some(mut c) = core.take() else { break };
            let joined = tokio::task::spawn_blocking(move || {
                let r = match cmd {
                    Some(cmd) => c.handle(cmd),
                    None => c.tick(),
                };
                let s = c.snapshot();
                (c, r, s)
            })
            .await;
            match joined {
                Ok((c, r, s)) => {
                    core = Some(c);
                    let _ = stx.send(s);
                    if let Some(reply) = reply {
                        let _ = reply.send(r);
                    }
                }
                Err(_) => break, // a panic in a command: stop rather than run on broken state
            }
        }
        if let Some(c) = core {
            let _ = tokio::task::spawn_blocking(move || drop(c)).await;
        }
    });
    Ok(Handle { tx, snap: srx })
}
