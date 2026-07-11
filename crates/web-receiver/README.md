# wisp-web-receiver

Browser (wasm32) file receiver for drift. A relay-only iroh endpoint +
iroh-blobs `MemStore` that speaks the receiver half of the `wisp/transfer/v1`
v4 control protocol (shared schema from `wisp-wire`). File bytes ride n0 public
relays end-to-end; the static page that loads this wasm module carries none of
them.

Status: **Spike 0 (compile gate) is green.** `src/lib.rs` currently just proves
the iroh 0.97 + iroh-blobs 0.99 (MemStore) + wisp-wire surface links for wasm.
The real receiver (rendezvous register → accept `wisp/transfer/v1` → MemStore
fetch → browser download) lands in Spike 1+.

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
