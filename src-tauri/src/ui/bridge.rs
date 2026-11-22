//! What `window.aeval` was: a page calls an `ipc` channel and gets its
//! answer back on the UI thread. The call runs on the async runtime
//! exactly as a webview's invoke did (`ipc::invoke`: the allowlist, the
//! per-window rules, the panic guard), with the payload as a `Value`, never
//! as text.

use std::future::Future;

use serde_json::Value;

use crate::shell::{async_runtime, AppHandle, Manager};

/// The call itself; resolves to the channel's answer.
pub fn invoke(
    label: &str,
    channel: &str,
    payload: Value,
) -> impl Future<Output = Result<Value, String>> + 'static {
    let label = label.to_string();
    let channel = channel.to_string();
    let job = async_runtime::spawn(async move {
        let Some(w) = AppHandle::get().get_webview_window(&label) else {
            return Err(format!("no window {label}"));
        };
        let payload = if payload.is_null() { None } else { Some(payload) };
        crate::ipc::invoke(w, channel, payload).await
    });
    async move {
        match job.await {
            Ok(r) => r,
            Err(e) => Err(e.to_string()),
        }
    }
}

/// Calls `channel` and hands the answer to `then` on the UI thread.
pub fn call(
    label: &str,
    channel: &str,
    payload: Value,
    then: impl FnOnce(Result<Value, String>) + 'static,
) {
    let fut = invoke(label, channel, payload);
    let _ = slint::spawn_local(async move {
        then(fut.await);
    });
}

/// Calls `channel` and forgets the answer.
pub fn send(label: &str, channel: &str, payload: Value) {
    let fut = invoke(label, channel, payload);
    async_runtime::spawn(async move {
        let _ = fut.await;
    });
}
