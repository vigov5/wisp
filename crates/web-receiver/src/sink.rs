//! Bridge to the page's download sink.
//!
//! Where the bytes actually land is a browser question, not a Rust one - a
//! service-worker stream that writes to disk as it arrives, or a Blob assembled
//! in the page when that is unavailable - so the page owns the implementation
//! and passes it in. This module is only the calling convention.
//!
//! The JS object is duck-typed rather than declared with `wasm_bindgen(extern)`
//! so the page is free to swap implementations per download without the two
//! sides having to agree on a class:
//!
//! ```js
//! {
//!   streaming(): boolean,          // does create() write straight to disk?
//!   create(name, size): Promise<{  // size is null when not known up front
//!     write(chunk: Uint8Array): Promise<void>,
//!     close(): Promise<string|null>,  // object URL, if the bytes were buffered
//!     abort(): Promise<void>,
//!   }>
//! }
//! ```

use anyhow::{Result, anyhow};
use js_sys::{Array, Function, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;

/// The page's download factory.
#[derive(Clone)]
pub(crate) struct Downloads(JsValue);

impl Downloads {
    pub(crate) fn new(value: JsValue) -> Self {
        Self(value)
    }

    /// Whether a download opened now goes straight to disk.
    ///
    /// False means the page has to hold the whole file before it can offer it,
    /// which puts the transfer back under a memory ceiling worth warning about.
    /// Asked per offer, not once at startup: a service worker takes over some
    /// time after the first load, so a tab can start without streaming and
    /// gain it.
    pub(crate) fn streams_to_disk(&self) -> bool {
        call(&self.0, "streaming", &[])
            .ok()
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    /// Open a download named `filename`. `size` is the final byte count when it
    /// is known before the first byte, which lets the browser show real progress
    /// in its own download UI.
    pub(crate) async fn open(&self, filename: &str, size: Option<u64>) -> Result<Sink> {
        let size = match size {
            Some(size) => JsValue::from_f64(size as f64),
            None => JsValue::NULL,
        };
        let created = call(&self.0, "create", &[JsValue::from_str(filename), size])?;
        Ok(Sink(settled(created).await?))
    }
}

/// One open download.
pub(crate) struct Sink(JsValue);

impl Sink {
    /// Append `data`. Resolves once the sink is ready for more, which is what
    /// paces the fetch behind it.
    pub(crate) async fn write(&self, data: &[u8]) -> Result<()> {
        // A fresh JS-heap array, not a view: growing wasm linear memory
        // detaches every outstanding view, and a queued write would then be
        // reading from a dead buffer.
        let chunk = Uint8Array::from(data);
        settled(call(&self.0, "write", &[chunk.into()])?).await?;
        Ok(())
    }

    /// Complete the download. Returns an object URL when the page buffered the
    /// bytes instead of streaming them, so the UI can offer a link.
    pub(crate) async fn close(self) -> Result<Option<String>> {
        Ok(settled(call(&self.0, "close", &[])?).await?.as_string())
    }

    /// Give up on the download. Best-effort: this runs on paths that are
    /// already failing, and it must not replace their error with its own.
    pub(crate) async fn abort(self) {
        if let Ok(pending) = call(&self.0, "abort", &[]) {
            let _ = settled(pending).await;
        }
    }
}

fn call(object: &JsValue, method: &str, args: &[JsValue]) -> Result<JsValue> {
    let function: Function = Reflect::get(object, &JsValue::from_str(method))
        .map_err(|value| js_error(method, value))?
        .dyn_into()
        .map_err(|_| anyhow!("the download sink has no {method}()"))?;
    let list = Array::new();
    for arg in args {
        list.push(arg);
    }
    Reflect::apply(&function, object, &list).map_err(|value| js_error(method, value))
}

/// Await `value` if it is a promise, otherwise take it as already settled -
/// so the page may implement any of these methods synchronously.
async fn settled(value: JsValue) -> Result<JsValue> {
    match value.dyn_into::<Promise>() {
        Ok(promise) => JsFuture::from(promise)
            .await
            .map_err(|value| js_error("the download sink", value)),
        Err(value) => Ok(value),
    }
}

fn js_error(context: &str, value: JsValue) -> anyhow::Error {
    // A cancelled stream rejects with no reason at all, and the debug form of
    // that ("JsValue(undefined)") tells the user nothing.
    if value.is_undefined() || value.is_null() {
        return anyhow!("{context} stopped without saying why");
    }
    let text = value
        .as_string()
        .or_else(|| {
            Reflect::get(&value, &JsValue::from_str("message"))
                .ok()
                .and_then(|message| message.as_string())
        })
        .unwrap_or_else(|| format!("{value:?}"));
    anyhow!("{context}: {text}")
}
