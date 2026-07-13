# wisp-web-receiver

Browser (wasm32) peer for drift — both **receive** and **send** halves of the
`wisp/transfer/v1` v4 control protocol (shared schema from `wisp-wire`) over a
relay-only iroh endpoint + iroh-blobs `MemStore`. File bytes ride n0 public
relays end-to-end; the static page that loads this wasm module carries none of
them.

Two wasm-bindgen entry points, both driven from `web/app.js`:
- [`WebReceiver`] (`src/lib.rs`) — register a code, accept inbound
  `wisp/transfer/v1`, fetch the collection into `MemStore`, trigger downloads.
- [`WebSender`] (`src/send.rs`) — claim a code, dial the receiver, and send
  inline text/link (rides the offer) or a single file (staged in `MemStore` and
  served from the tab via an `iroh::protocol::Router` on the blobs ALPN, so the
  receiver dials back and fetches). RAM-bound; multi-file is future work.

Despite the crate name, it's the whole browser transfer surface; the name is
kept to avoid churn in `web/build.sh` / `sync-docs.sh` / the `pkg/` filenames.

## Building for the browser

Browser iroh pulls `ring` (crypto for QUIC/TLS), whose C is compiled for wasm by
**clang** — so a clang is required on `PATH` or via the `CC_*`/`AR_*` env vars.
Any recent LLVM works; the Android NDK's bundled clang is a convenient one if you
already have the NDK.

```sh
# From the repo root. Point cc-rs at a clang that can target wasm32.
# Example using the Android NDK's clang (adjust the NDK path/version):
NDK="$ANDROID_HOME/ndk/28.2.13676358/toolchains/llvm/prebuilt/windows-x86_64/bin"
export CC_wasm32_unknown_unknown="$NDK/clang.exe"
export AR_wasm32_unknown_unknown="$NDK/llvm-ar.exe"

cargo build --target wasm32-unknown-unknown -p wisp-web-receiver
```

Notes:

- `iroh` / `iroh-blobs` are pinned with `default-features = false` (drops native
  `metrics` and the `fs-store`/`rpc` backends, leaving the wasm-safe `MemStore`).
- getrandom's browser backend is selected in `../../.cargo/config.toml`
  (`--cfg getrandom_backend="wasm_js"`, wasm target only) plus the `wasm_js` /
  `js` features in `Cargo.toml`.
- The crate is a workspace member but excluded from `default-members`, so a bare
  native `cargo build` / `cargo test` skips it.

## Generating JS bindings (later)

Spike 1 will add a JS API via `wasm-bindgen`; build with
`wasm-pack build --target web` (or `wasm-bindgen-cli`) to emit the `pkg/` the
static `web/` front-end imports.
