import Alpine from './vendor/alpine.esm.js';
import init, { WebReceiver } from './pkg/wisp_web_receiver.js';

// Rendezvous server the browser registers with. Override with ?rendezvous=... for
// local testing (e.g. ?rendezvous=http://localhost:8787). File bytes never touch
// this server — only the ~10 KB code/ticket handshake does.
const params = new URLSearchParams(location.search);
const RENDEZVOUS_URL =
  params.get('rendezvous') || 'https://rendezvous.wisp.mooo.com';

const isUrl = (s) => /^https?:\/\/\S+$/i.test(s.trim());

function formatBytes(n) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n, u = 0;
  while (v >= 1024 && u < units.length - 1) { v /= 1024; u++; }
  return u === 0 ? `${Math.round(n)} B` : `${v.toFixed(1)} ${units[u]}`;
}

function formatEta(secs) {
  if (secs < 60) return `${Math.ceil(secs)}s`;
  const m = Math.floor(secs / 60);
  const s = Math.ceil(secs % 60);
  return `${m}m ${s}s`;
}

// Identity badge, mirroring native pubkey_visual.dart exactly: hue is the sum of
// the endpoint id's char codes mod 360; the pill is a tint of HSL(hue,55%,55%)
// (background 15%, border 45%) with theme-adapted text (lightness 32% light /
// 78% dark), and the label is the uppercase "AAAA…ZZZZ" short form.
function pubkeyHue(pubkey) {
  let hue = 0;
  for (let i = 0; i < pubkey.length; i++) hue = (hue + pubkey.charCodeAt(i)) % 360;
  return hue;
}
function isDarkTheme() {
  const attr = document.documentElement.getAttribute('data-theme');
  if (attr) return attr === 'dark';
  return window.matchMedia('(prefers-color-scheme: dark)').matches;
}
function badgeStyleFor(pubkey) {
  const hue = pubkeyHue(pubkey || '');
  return (
    `background:hsl(${hue} 55% 55% / 0.15);` +
    `border-color:hsl(${hue} 55% 55% / 0.45);` +
    `color:hsl(${hue} 55% ${isDarkTheme() ? 78 : 32}%)`
  );
}
function shortPubkey(pubkey) {
  if (!pubkey) return '';
  const s = pubkey.toUpperCase();
  return s.length <= 9 ? s : `${s.slice(0, 4)}…${s.slice(-4)}`;
}

// Device-type glyphs (Material paths) so the sender's kind reads as an icon,
// like the native RecipientAvatar, instead of the raw "· phone" text.
const DEVICE_ICON_PATHS = {
  phone:
    'M17 1.01 7 1c-1.1 0-2 .9-2 2v18c0 1.1.9 2 2 2h10c1.1 0 2-.9 2-2V3c0-1.1-.9-1.99-2-1.99zM17 19H7V5h10v14z',
  laptop:
    'M20 18c1.1 0 1.99-.9 1.99-2L22 6c0-1.1-.9-2-2-2H4c-1.1 0-2 .9-2 2v10c0 1.1.9 2 2 2H0v2h24v-2h-4z',
};
function deviceIconSvg(type) {
  const path = DEVICE_ICON_PATHS[type] || DEVICE_ICON_PATHS.laptop;
  return `<svg viewBox="0 0 24 24" aria-hidden="true"><path d="${path}"/></svg>`;
}

// Single reactive store the whole UI binds to (see index.html). The wasm
// receiver pushes events into `onEvent`, which mutates state; Alpine re-renders.
// Button handlers call the wasm methods through `receiver`. Fields prefixed with
// `_` are bookkeeping the template never reads.
Alpine.store('rx', {
  receiver: null,

  code: '------',
  ttl: '',
  status: 'Loading…',

  codeVisible: false,
  offerVisible: false,
  senderVisible: false,
  decisionVisible: false,
  progressVisible: false,

  sender: { name: '', deviceType: '', pubkey: '' },
  senderBadge: '',
  badgeStyle: '',
  deviceIcon: '',

  files: [],
  warning: '',

  progressPct: 0,
  progressText: '',

  downloads: [],

  text: null,
  isLink: false,
  textHref: '',

  copyCodeLabel: 'Copy code',

  _totalBytes: 0,
  _progressStart: 0,
  _ttlTimer: null,
  _lastText: '',

  resetOffer() {
    this.offerVisible = false;
    this.senderVisible = false;
    this.decisionVisible = false;
    this.progressVisible = false;
    this.files = [];
    this.progressPct = 0;
    this.progressText = '';
  },

  startCountdown(iso) {
    const end = Date.parse(iso);
    if (this._ttlTimer) clearInterval(this._ttlTimer);
    if (!end) { this.ttl = ''; return; }
    const tick = () => {
      const remaining = Math.max(0, Math.floor((end - Date.now()) / 1000));
      const m = Math.floor(remaining / 60);
      const s = String(remaining % 60).padStart(2, '0');
      this.ttl =
        remaining > 0 ? `Code expires in ${m}:${s}` : 'Code expired — refreshing…';
      if (remaining <= 0) clearInterval(this._ttlTimer);
    };
    tick();
    this._ttlTimer = setInterval(tick, 1000);
  },

  onEvent(event) {
    switch (event.type) {
      case 'registered':
        this.code = event.code;
        this.codeVisible = true;
        this.startCountdown(event.expiresAt);
        this.status = 'Waiting for a sender…';
        break;

      case 'connecting':
        this.resetOffer();
        this.offerVisible = true;
        this.senderVisible = true;
        this.sender = {
          name: event.senderName || 'Unknown device',
          deviceType: event.senderDeviceType || '',
          pubkey: event.senderPubkey || '',
        };
        this.senderBadge = shortPubkey(event.senderPubkey);
        this.badgeStyle = badgeStyleFor(event.senderPubkey);
        this.deviceIcon = deviceIconSvg(event.senderDeviceType);
        this.status = 'A sender is connecting…';
        break;

      case 'offer':
        this.offerVisible = true;
        this._totalBytes = event.totalBytes;
        this.files = event.files.map((f) => ({
          path: f.path,
          label: `${f.path} — ${formatBytes(f.size)}`,
        }));
        this.warning = event.tooLarge
          ? "This transfer is large and may exceed your browser tab's memory — it might fail."
          : '';
        this.decisionVisible = true;
        this.status = `Accept ${event.files.length} file(s), ${formatBytes(this._totalBytes)}?`;
        break;

      case 'transferStarted':
        this.decisionVisible = false;
        this.progressVisible = true;
        this._progressStart = Date.now();
        this.status = 'Transfer started — downloading over relay…';
        break;

      case 'progress': {
        // Self-heal the UI: bytes are flowing, so the decision is settled even if
        // the transferStarted event was missed or arrived late.
        this.decisionVisible = false;
        this.progressVisible = true;
        // The blob fetch reports total wire bytes (collection metadata + file
        // payload), which can exceed the manifest's file-content total, so clamp
        // for display.
        const received = Math.min(event.bytesReceived, this._totalBytes);
        this.progressPct = this._totalBytes
          ? Math.min(100, (received / this._totalBytes) * 100)
          : 0;
        const secs = Math.max(0.001, (Date.now() - this._progressStart) / 1000);
        const speed = received / secs;
        let line = `${formatBytes(received)} / ${formatBytes(this._totalBytes)}`;
        if (speed > 0) {
          line += ` · ${formatBytes(speed)}/s`;
          const remaining = Math.max(0, this._totalBytes - received);
          const eta = remaining / speed;
          if (received < this._totalBytes && isFinite(eta)) line += ` · ETA ${formatEta(eta)}`;
        }
        this.progressText = line;
        break;
      }

      case 'fileReady':
        this.downloads.push({
          path: event.path,
          url: event.url,
          label: `${event.path} (${formatBytes(event.size)})`,
        });
        break;

      case 'textReady':
        this.decisionVisible = false;
        this._lastText = event.text;
        this.text = event.text;
        this.isLink = isUrl(event.text);
        this.textHref = event.text.trim();
        break;

      case 'completed':
        // Clear the incoming offer UI (sender badge, file list, progress) so a
        // stale "INCOMING" card doesn't linger after the files land. The
        // Downloads list and any received text live in separate sections and
        // stay visible.
        this.resetOffer();
        this.status = 'Transfer complete ✓';
        break;

      case 'declined':
        this.resetOffer();
        this.status = 'Declined. Waiting for a sender…';
        break;

      case 'cancelled':
        this.resetOffer();
        this.status = 'Cancelled. Waiting for a sender…';
        break;

      case 'error':
        // The receiver stays live (accept loop keeps running), so clear any
        // in-flight offer/progress UI and fall back to the waiting state.
        this.resetOffer();
        this.status = `${event.message} — waiting for a sender…`;
        break;

      default:
        console.warn('unknown event', event);
    }
  },

  accept() {
    if (!this.receiver) return;
    // Hide the decision immediately; transferStarted/textReady take over next.
    this.decisionVisible = false;
    this.status = 'Accepted — starting…';
    this.receiver.accept();
  },

  decline() {
    if (!this.receiver) return;
    this.decisionVisible = false;
    this.receiver.decline();
  },

  cancel() {
    if (!this.receiver) return;
    this.status = 'Cancelling…';
    this.receiver.cancel();
  },

  async copyCode() {
    if (!this.code || this.code === '------') return;
    try {
      await navigator.clipboard.writeText(this.code);
      this.copyCodeLabel = 'Copied ✓';
      setTimeout(() => { this.copyCodeLabel = 'Copy code'; }, 1500);
    } catch {
      this.status = 'Copy failed — select the code manually.';
    }
  },

  async copyText() {
    try {
      await navigator.clipboard.writeText(this._lastText);
      this.status = 'Copied to clipboard ✓';
    } catch {
      this.status = 'Copy failed — select and copy manually.';
    }
  },

  saveText() {
    const blob = new Blob([this._lastText], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'message.txt';
    a.click();
    URL.revokeObjectURL(url);
  },
});

window.Alpine = Alpine;
Alpine.start();

async function main() {
  const rx = Alpine.store('rx');
  rx.status = 'Loading…';
  await init();
  try {
    rx.receiver = await WebReceiver.start(RENDEZVOUS_URL, (e) => rx.onEvent(e));
    console.log('receiver started, code:', rx.receiver.code());
  } catch (err) {
    rx.status = `Failed to start: ${err}`;
    console.error(err);
  }
}

main();
