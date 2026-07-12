# Wisp web receiver

A no-install browser receiver: open the page, get a 6-char code, a native Wisp
sender sends to that code, and the file downloads in the browser. File bytes ride
n0 public relays end-to-end (the browser is relay-only); this page and the
rendezvous server only carry the tiny code/ticket handshake.

Capabilities: sender identity + Accept/Decline, inline text/link receive
(Copy/Save/Open), multi-file with per-transfer progress/speed/ETA, mid-transfer
cancel, code TTL countdown + rotation, and an over-size warning. The visual
system mirrors the native app's theme tokens (`web/style.css` ← `flutter/lib/
theme/wisp_theme.dart`).

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

## Deploy (GitHub Pages from `main:/docs`)

GitHub Pages serves the Jekyll site in **`main:/docs`** (which also hosts the
privacy policy) at **web.wisp.mooo.com**. Pages allows a single source, so the
receiver is committed alongside it: `docs/{index.html,app.js,style.css}` +
`docs/pkg/` (the built wasm). The receiver uses relative asset paths and has no
Jekyll front matter, so it's copied verbatim and served at the site root,
independent of the site's `baseurl`; the privacy policy stays at
`/privacy-policy/`.

Because the wasm is committed (Pages can't build it on the fly), refresh it
whenever `crates/web-receiver`, `crates/wire`, or `web/{index.html,app.js,
style.css}` change:

```sh
web/sync-docs.sh          # release build + copy into docs/
git add docs/ && git commit -m "chore(web): sync receiver into docs"
```

`.github/workflows/deploy-web.yml` is a **compile gate** — it rebuilds the wasm
in CI on relevant changes so a stale/broken commit is caught, but does not
deploy (Pages deploys `docs/` itself).

The page defaults to the production rendezvous
(`https://rendezvous.wisp.mooo.com`, permissive CORS already set). File bytes
never touch it — only the code/ticket handshake — so the host carries zero
transfer bandwidth regardless of file size.

## Notes / limits

- **Relay-only, in-memory.** The browser can't hole-punch (no UDP) or persist to
  disk: every transfer rides an n0 relay and lives entirely in tab memory
  (`MemStore`). Huge transfers can exhaust the tab — the UI warns past ~1 GiB.
  LAN/mDNS, QR-scan, USB/AOA, direct P2P, and on-disk resume are out of scope
  in-browser by design.
- `web/pkg/` is generated and git-ignored; the CI builds a fresh copy on deploy.
