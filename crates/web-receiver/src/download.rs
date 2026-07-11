//! Trigger a browser file download from bytes held in wasm memory.

use anyhow::{Context, Result, anyhow};
use js_sys::{Array, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{Blob, HtmlAnchorElement, Url};

fn js_err(context: &str, v: JsValue) -> anyhow::Error {
    anyhow!("{context}: {v:?}")
}

/// Wrap `bytes` in a Blob and click a synthetic `<a download>` to save it as
/// `filename`. Returns the created object URL so the caller can also render a
/// persistent link (and revoke it later).
pub fn trigger_download(filename: &str, bytes: &[u8]) -> Result<String> {
    let array = Uint8Array::new_with_length(bytes.len() as u32);
    array.copy_from(bytes);

    // Blob wants a JS sequence of BufferSource parts.
    let parts = Array::new();
    parts.push(&array);
    let blob = Blob::new_with_u8_array_sequence(&parts).map_err(|v| js_err("creating blob", v))?;

    let url =
        Url::create_object_url_with_blob(&blob).map_err(|v| js_err("creating object url", v))?;

    let window = web_sys::window().context("no window")?;
    let document = window.document().context("no document")?;
    let anchor: HtmlAnchorElement = document
        .create_element("a")
        .map_err(|v| js_err("creating anchor", v))?
        .dyn_into()
        .map_err(|v| js_err("anchor is not an <a>", v.into()))?;
    anchor.set_href(&url);
    anchor.set_download(filename);
    anchor.click();

    Ok(url)
}
