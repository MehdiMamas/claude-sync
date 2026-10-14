//! Core calls made by the background loop and the file watcher, with panics turned into
//! errors: a bug in one pass must not end the loop or the watcher thread for the rest of
//! the session. This is why the release profile keeps `panic = "unwind"`.
use futures_util::FutureExt;
use std::any::Any;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};

/// Error text shown to the user and written to the log.
pub type Error = String;

fn panic_text(what: &str, payload: Box<dyn Any + Send>) -> Error {
    let msg = payload
        .downcast_ref::<&str>()
        .map(|s| s.to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic".into());
    let err = format!("{what}: internal error ({msg})");
    tracing::error!("{err}");
    err
}

/// Log a core error and turn it into the text the UI shows.
pub fn logged(what: &'static str) -> impl Fn(claude_sync_core::Error) -> Error {
    move |e| {
        tracing::warn!("{what}: {e}");
        e.to_string()
    }
}

/// Run a synchronous core call.
pub fn call<T>(what: &str, f: impl FnOnce() -> claude_sync_core::Result<T>) -> Result<T, Error> {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            tracing::warn!("{what}: {e}");
            Err(e.to_string())
        }
        Err(p) => Err(panic_text(what, p)),
    }
}

/// Run an async core call.
pub async fn call_async<T>(
    what: &str,
    f: impl Future<Output = claude_sync_core::Result<T>>,
) -> Result<T, Error> {
    match AssertUnwindSafe(f).catch_unwind().await {
        Ok(Ok(v)) => Ok(v),
        Ok(Err(e)) => {
            tracing::warn!("{what}: {e}");
            Err(e.to_string())
        }
        Err(p) => Err(panic_text(what, p)),
    }
}

/// Log panics instead of printing them to a console the release build does not have.
pub fn install_panic_hook() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let loc = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()));
        tracing::error!(location = loc.as_deref().unwrap_or("?"), "panic: {info}");
        if cfg!(debug_assertions) {
            default(info);
        }
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panic_becomes_an_error() {
        let r: Result<(), Error> = call("thing", || panic!("boom"));
        assert_eq!(r.unwrap_err(), "thing: internal error (boom)");
    }

    #[test]
    fn core_error_is_passed_through() {
        let r: Result<(), Error> = call("thing", || Err(claude_sync_core::Error::NotSignedIn));
        assert_eq!(r.unwrap_err(), "not signed in");
    }

    #[test]
    fn async_panic_becomes_an_error() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let r: Result<u8, Error> = rt.block_on(call_async("later", async { panic!("late") }));
        assert_eq!(r.unwrap_err(), "later: internal error (late)");
        let ok = rt.block_on(call_async("fine", async { Ok(7u8) }));
        assert_eq!(ok, Ok(7));
    }
}
