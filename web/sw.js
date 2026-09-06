/* Wisp web — service worker.
 *
 * Three jobs:
 *   1. Make the page installable (a registered SW with a fetch handler is a
 *      PWA-install prerequisite, alongside the manifest + HTTPS).
 *   2. Give the app shell an offline fallback so a launched PWA still opens
 *      when the network is down.
 *   3. Serve received files as streaming downloads — see below.
 *
 * Strategy is network-first for same-origin GETs: online users always get the
 * freshly deployed page/wasm (no stale-cache traps after a docs/ sync), and the
 * cache is only a fallback when the network fails. Everything else — cross-origin
 * requests (the rendezvous handshake) and non-GET — is left untouched so the
 * transfer path is never mediated by the SW.
 *
 * The download route is the one exception. A page can't ask the browser to write
 * a file to disk incrementally, but it can answer its own request with a stream:
 * download-sink.js hands us the readable half of a stream it is still filling
 * and then navigates a hidden iframe here, and we reply with that stream as an
 * attachment. The browser downloads it like any other file, so a received
 * transfer never has to fit in the tab.
 *
 * Bump CACHE_VERSION whenever the shipped shell changes so old caches are purged
 * on activate. web/sync-docs.sh keeps it in step with the app version.
 */
const CACHE_VERSION = 'v2.2.0';
const CACHE_NAME = `wisp-shell-${CACHE_VERSION}`;

// The static shell to precache for offline launch. Kept small: HTML/CSS/JS,
// Alpine, the wasm bundle, icons, and the manifest. Relative to the SW scope.
const SHELL = [
  './',
  './index.html',
  './style.css',
  './app.js',
  './download-sink.js',
  './manifest.webmanifest',
  './wisp-logo.png',
  './vendor/alpine.esm.js',
  './pkg/wisp_web_receiver.js',
  './pkg/wisp_web_receiver_bg.wasm',
  './icons/icon-192.png',
  './icons/icon-512.png',
  './icons/icon-maskable-512.png',
];

self.addEventListener('install', (event) => {
  // Precache the shell, but don't let one missing asset abort the whole install
  // (e.g. a renamed pkg file) — add them individually and ignore failures.
  //
  // A failure here must not fail the install: offline launch is a nicety, while
  // an installed worker is what lets received files stream to disk. Storage can
  // be unavailable outright (a locked-down profile, a browser set to block site
  // data), and `caches.open` rejecting used to take the whole worker down with
  // it — silently costing every transfer on that browser the streaming path.
  event.waitUntil(
    caches
      .open(CACHE_NAME)
      .then((cache) => Promise.allSettled(SHELL.map((url) => cache.add(url))))
      .catch((err) => {
        console.warn('shell precache unavailable', err);
      }),
  );
  // Take over as soon as installed so the very next navigation is controlled.
  self.skipWaiting();
});

self.addEventListener('activate', (event) => {
  // Drop caches from older versions, then claim open clients.
  event.waitUntil(
    caches
      .keys()
      .then((keys) =>
        Promise.all(
          keys
            .filter((k) => k.startsWith('wisp-shell-') && k !== CACHE_NAME)
            .map((k) => caches.delete(k)),
        ),
      )
      // Same as install: claiming clients is the part that matters, and it must
      // not be lost because the cache couldn't be read.
      .catch(() => {})
      .then(() => self.clients.claim()),
  );
});

/* Streaming downloads the page has handed over but not yet collected, keyed by
 * the id it minted. Each is claimed by the very next request for it. */
const pendingDownloads = new Map();
/** Path prefix the page navigates to to collect one. Resolved on first use
 *  rather than at load: `registration` isn't reliably readable while the script
 *  is still evaluating, and throwing here would fail the install outright. */
let downloadRoute = null;
function downloadRoutePath() {
  if (downloadRoute === null) {
    const base = (self.registration && self.registration.scope) || self.location.href;
    downloadRoute = new URL('__wisp-dl/', base).pathname;
  }
  return downloadRoute;
}
/** How long an unclaimed download is held before it's dropped. The page
 *  navigates within a tick of being acknowledged, so this only ever catches a
 *  page that went away in between. */
const CLAIM_TIMEOUT_MS = 30_000;

self.addEventListener('message', (event) => {
  const data = event.data;
  if (!data || data.type !== 'wisp-download') return;
  const port = event.ports && event.ports[0];
  pendingDownloads.set(data.id, {
    body: data.body,
    filename: data.filename,
    size: data.size,
    port,
  });
  setTimeout(() => pendingDownloads.delete(data.id), CLAIM_TIMEOUT_MS);
  // Acknowledge, so the page only navigates once the stream is really here.
  if (port) port.postMessage({ type: 'registered' });
});

function respondWithDownload(url) {
  const id = url.pathname.slice(downloadRoutePath().length).split('/')[0];
  const download = pendingDownloads.get(id);
  if (!download) {
    return new Response('This download is no longer available.', {
      status: 404,
      headers: { 'Content-Type': 'text/plain' },
    });
  }
  pendingDownloads.delete(id);
  // Tell the page the browser has come for it. Until this lands the page must
  // not write: a stream nobody is reading stalls the transfer silently, and the
  // page can still fall back to buffering.
  if (download.port) download.port.postMessage({ type: 'claimed' });

  // `filename*` is what carries the real name; the plain `filename` is an ASCII
  // fallback for anything that doesn't read the extended form.
  const encoded = encodeURIComponent(download.filename);
  const ascii = download.filename.replace(/[^\x20-\x7e]/g, '_').replace(/"/g, '');
  const headers = new Headers({
    'Content-Type': 'application/octet-stream',
    'Content-Disposition': `attachment; filename="${ascii}"; filename*=UTF-8''${encoded}`,
    'Cache-Control': 'no-store',
  });
  // Only set when the page knew the length up front (a single file does; a zip
  // being built entry by entry doesn't). It buys a real progress bar in the
  // browser's download UI, and makes a truncated stream read as a failure.
  if (download.size !== null && download.size !== undefined) {
    headers.set('Content-Length', String(download.size));
  }
  return new Response(download.body, { headers });
}

self.addEventListener('fetch', (event) => {
  const req = event.request;
  const url = new URL(req.url);

  // A received file the page is streaming to us. Answered from memory, never
  // cached, and checked before anything else so it can't be mistaken for shell.
  if (
    url.origin === self.location.origin &&
    url.pathname.startsWith(downloadRoutePath())
  ) {
    event.respondWith(respondWithDownload(url));
    return;
  }

  // Only ever touch same-origin GETs. The rendezvous handshake and blob pull are
  // cross-origin / non-GET — leave them entirely to the browser.
  if (req.method !== 'GET' || url.origin !== self.location.origin) return;

  event.respondWith(
    fetch(req)
      .then((res) => {
        // Cache a copy of good same-origin responses for offline fallback.
        if (res && res.status === 200 && res.type === 'basic') {
          const copy = res.clone();
          caches
            .open(CACHE_NAME)
            .then((cache) => cache.put(req, copy))
            .catch(() => {});
        }
        return res;
      })
      .catch(async () => {
        // Offline: serve the cached asset, falling back to the app shell for
        // navigations (so deep links / the standalone launch still open).
        try {
          const cached = await caches.match(req);
          if (cached) return cached;
          if (req.mode === 'navigate') {
            const shell = await caches.match('./index.html');
            if (shell) return shell;
          }
        } catch (err) {
          /* no storage to fall back on */
        }
        return Response.error();
      }),
  );
});
