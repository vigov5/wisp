/* Wisp web — service worker.
 *
 * Two jobs:
 *   1. Make the page installable (a registered SW with a fetch handler is a
 *      PWA-install prerequisite, alongside the manifest + HTTPS).
 *   2. Give the app shell an offline fallback so a launched PWA still opens
 *      when the network is down.
 *
 * Strategy is network-first for same-origin GETs: online users always get the
 * freshly deployed page/wasm (no stale-cache traps after a docs/ sync), and the
 * cache is only a fallback when the network fails. Everything else — cross-origin
 * requests (the rendezvous handshake) and non-GET — is left untouched so the
 * transfer path is never mediated by the SW.
 *
 * Bump CACHE_VERSION whenever the shipped shell changes so old caches are purged
 * on activate. web/sync-docs.sh keeps it in step with the app version.
 */
const CACHE_VERSION = 'v1.13.2';
const CACHE_NAME = `wisp-shell-${CACHE_VERSION}`;

// The static shell to precache for offline launch. Kept small: HTML/CSS/JS,
// Alpine, the wasm bundle, icons, and the manifest. Relative to the SW scope.
const SHELL = [
  './',
  './index.html',
  './style.css',
  './app.js',
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
  event.waitUntil(
    caches.open(CACHE_NAME).then((cache) =>
      Promise.allSettled(SHELL.map((url) => cache.add(url))),
    ),
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
      .then(() => self.clients.claim()),
  );
});

self.addEventListener('fetch', (event) => {
  const req = event.request;
  const url = new URL(req.url);

  // Only ever touch same-origin GETs. The rendezvous handshake and blob pull are
  // cross-origin / non-GET — leave them entirely to the browser.
  if (req.method !== 'GET' || url.origin !== self.location.origin) return;

  event.respondWith(
    fetch(req)
      .then((res) => {
        // Cache a copy of good same-origin responses for offline fallback.
        if (res && res.status === 200 && res.type === 'basic') {
          const copy = res.clone();
          caches.open(CACHE_NAME).then((cache) => cache.put(req, copy));
        }
        return res;
      })
      .catch(async () => {
        // Offline: serve the cached asset, falling back to the app shell for
        // navigations (so deep links / the standalone launch still open).
        const cached = await caches.match(req);
        if (cached) return cached;
        if (req.mode === 'navigate') {
          const shell = await caches.match('./index.html');
          if (shell) return shell;
        }
        return Response.error();
      }),
  );
});
