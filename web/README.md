# Drift web receiver (Spike 1)

A no-install browser receiver: open the page, get a 6-char code, a native drift
sender sends to that code, and the file downloads in the browser. File bytes ride
n0 public relays end-to-end (the browser is relay-only); this page and the
rendezvous server only carry the tiny code/ticket handshake.

## Build

```sh
web/build.sh            # debug wasm + JS bindings → web/pkg/
# or:  web/build.sh release   (optimized, much smaller wasm)
```

Requires the wasm target and a clang for `ring` (see `crates/web-receiver/README.md`).
`web/build.sh` auto-detects the Android NDK clang if `$ANDROID_HOME` is set.

## End-to-end test (single machine — the browser is always relay-only)

You do **not** need two networks: a browser iroh endpoint can't hole-punch, so
the transfer rides an n0 relay even when the sender is on the same box.

1. **Rendezvous server** (local, with CORS for the static page):
   ```sh
   cargo run --bin wisp-server -- serve --listen 127.0.0.1:8787
   ```

2. **Serve the static page:**
   ```sh
   python -m http.server 8080 --directory web
   ```

3. **Open the receiver** in a browser, pointing it at the local rendezvous:
   ```
   http://localhost:8080/?rendezvous=http://localhost:8787
   ```
   A 6-char code appears. (Leave off `?rendezvous=…` to use the production
   server at `https://rendezvous.wisp.mooo.com`.)

4. **Send a file** to that code from the native sender:
   ```sh
   WISP_RENDEZVOUS_URL=http://127.0.0.1:8787 \
     cargo run --bin wisp -- send -c <CODE> web/sample.txt
   ```

**Expected:** the browser shows the offer, auto-downloads a byte-identical file,
and the status reaches “Transfer complete ✓” while the sender exits cleanly.

## Notes / known Spike-1 limits

- File transfers only (inline text/QR/multi-file polish are later stages).
- Accept is automatic (no accept/decline prompt yet).
- MemStore: the whole transfer lives in tab memory — keep test files modest.
- `web/pkg/` is generated; don't commit it (host a freshly built copy).
