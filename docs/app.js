import Alpine from './vendor/alpine.esm.js';
import init, { WebReceiver, WebSender } from './pkg/wisp_web_receiver.js';

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

  // 'receive' | 'send' — which half of the card is showing. The receiver is
  // lazy: it only binds an endpoint + registers a code once receive mode is
  // actually entered (see ensureReceiver), so a send-only visit never burns a
  // pairing code.
  mode: 'receive',
  _receiverStarted: false,

  // 'dark' | 'light' — the inline <head> script already stamped data-theme
  // before paint; this mirrors it for the toggle glyph, defaulting to dark.
  theme: 'dark',

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
  // The inline text/link riding in the offer, shown as a preview *before*
  // Accept so the user knows what they're accepting. null on a file offer.
  offerText: null,
  warning: '',

  progressPct: 0,
  progressText: '',

  downloads: [],

  text: null,
  isLink: false,
  textHref: '',

  copyCodeLabel: 'Copy code',

  // ── Send (text/link/files) ──────────────────────────────────────────────────
  // Form inputs and the flow's phase, driven by WebSender events.
  sendForm: { code: '', text: '', fileLabel: '' },
  // idle | connecting | waiting | accepted | done | declined | cancelled | error
  sendPhase: 'idle',
  sendStatus: '',
  sendProgressPct: 0,
  sendReceiver: { name: '', deviceType: '', web: false, pubkey: '' },
  sendBadge: '',
  sendBadgeStyle: '',
  sendDeviceIcon: '',
  _sender: null,
  // Picked files (kept off the reactive tree — only a summary label is bound):
  // [{ file, path }], where path is the folder-relative path for a folder pick.
  _files: [],

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
    this.offerText = null;
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
        // Inline text/link: no files, no size — an empty manifest. Show the
        // text itself and prompt "Accept this text/link?" instead of the
        // nonsensical "Accept 0 file(s), 0 B?" the file path would produce.
        if (event.inlineText != null) {
          this.offerText = event.inlineText;
          this.files = [];
          this.warning = '';
          this.decisionVisible = true;
          this.status = isUrl(event.inlineText)
            ? 'Accept this link?'
            : 'Accept this text?';
          break;
        }
        this.offerText = null;
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

  // ── Theme ────────────────────────────────────────────────────────────────
  toggleTheme() {
    this.theme = this.theme === 'dark' ? 'light' : 'dark';
    document.documentElement.setAttribute('data-theme', this.theme);
    try {
      localStorage.setItem('wisp-theme', this.theme);
    } catch (e) {
      /* private mode / storage disabled — the toggle still works this session */
    }
    // Identity badges are tinted per-theme (text lightness flips), so recolor
    // any that are currently on screen.
    if (this.sender.pubkey) this.badgeStyle = badgeStyleFor(this.sender.pubkey);
    if (this.sendReceiver.pubkey) this.sendBadgeStyle = badgeStyleFor(this.sendReceiver.pubkey);
  },

  // ── Mode + lazy receiver ────────────────────────────────────────────────
  setMode(mode) {
    this.mode = mode;
    if (mode === 'receive') this.ensureReceiver();
  },

  // Bind the receiver + register a code the first time receive mode is shown.
  // Idempotent: switching back and forth reuses the one live receiver.
  async ensureReceiver() {
    if (this._receiverStarted) return;
    this._receiverStarted = true;
    this.status = 'Loading…';
    try {
      await wasmReady;
      this.receiver = await WebReceiver.start(RENDEZVOUS_URL, (e) => this.onEvent(e));
    } catch (err) {
      this._receiverStarted = false;
      this.status = `Failed to start: ${err}`;
      console.error(err);
    }
  },

  // ── Send (text/link/files) ──────────────────────────────────────────────────
  // Both the file picker and the folder picker feed here; a folder pick carries
  // webkitRelativePath so the tree round-trips (the receiver zips it back). Picks
  // are *additive* (deduped by path) so you can combine loose files with a
  // folder; a repeat pick of the same path just refreshes it.
  ingestFiles(event) {
    const incoming = Array.from(event.target.files || []).map((f) => ({
      file: f,
      path: f.webkitRelativePath || f.name,
    }));
    const byPath = new Map(this._files.map((e) => [e.path, e]));
    for (const e of incoming) byPath.set(e.path, e);
    this._files = Array.from(byPath.values());
    this._refreshFileLabel();
    // Let the same file/folder be re-picked later (change won't fire otherwise).
    event.target.value = '';
  },

  // Summary label: name after the common top folder only when *every* file
  // shares one (matching the receiver's zip-naming rule — else it's
  // `wisp-files.zip`); otherwise just a count.
  _refreshFileLabel() {
    const files = this._files;
    const total = files.reduce((s, e) => s + e.file.size, 0);
    if (files.length === 0) {
      this.sendForm.fileLabel = '';
    } else if (files.length === 1) {
      this.sendForm.fileLabel = `${files[0].path} · ${formatBytes(total)}`;
    } else {
      const tops = new Set(
        files.map((e) => (e.path.includes('/') ? e.path.split('/')[0] : null)),
      );
      const prefix = tops.size === 1 && !tops.has(null) ? `${[...tops][0]}/ — ` : '';
      this.sendForm.fileLabel = `${prefix}${files.length} files · ${formatBytes(total)}`;
    }
  },

  clearFiles() {
    this._files = [];
    this.sendForm.fileLabel = '';
  },

  async doSend() {
    const code = this.sendForm.code.trim().toUpperCase();
    const text = this.sendForm.text;
    const files = this._files;
    if (code.length !== 6) {
      this.sendPhase = 'error';
      this.sendStatus = 'Enter the 6-character code from the receiver.';
      return;
    }
    if (!files.length && !text.trim()) {
      this.sendPhase = 'error';
      this.sendStatus = 'Enter some text or a link, or add files.';
      return;
    }
    this.sendPhase = 'connecting';
    this.sendProgressPct = 0;
    this.sendStatus = 'Connecting…';
    try {
      await wasmReady;
      const cb = (e) => this.onSendEvent(e);
      if (files.length) {
        // Read every file into memory — the wasm store is RAM-bound, so the
        // whole batch has to fit in the tab.
        const paths = files.map((e) => e.path);
        const blobs = await Promise.all(
          files.map(async (e) => new Uint8Array(await e.file.arrayBuffer())),
        );
        this._sender = await WebSender.sendFiles(
          RENDEZVOUS_URL,
          code,
          paths,
          blobs,
          'Browser',
          cb,
        );
      } else {
        this._sender = await WebSender.sendText(RENDEZVOUS_URL, code, text, 'Browser', cb);
      }
    } catch (err) {
      // Rejected before the background task started — almost always a bad or
      // expired code, or a claim/connect failure worth showing on the form.
      this.sendPhase = 'error';
      this.sendStatus = `${err}`.replace(/^Error:\s*/, '');
      console.error(err);
    }
  },

  onSendEvent(event) {
    switch (event.type) {
      case 'connecting':
        this.sendPhase = 'connecting';
        this.sendStatus = 'Connecting over relay…';
        break;

      case 'waitingForDecision':
        this.sendPhase = 'waiting';
        this.sendReceiver = {
          name: event.receiverName || 'Device',
          deviceType: event.receiverDeviceType || '',
          web: !!event.receiverWeb,
          pubkey: event.receiverPubkey || '',
        };
        this.sendBadge = shortPubkey(event.receiverPubkey);
        this.sendBadgeStyle = badgeStyleFor(event.receiverPubkey);
        // A web receiver reads as a laptop glyph (matching native).
        this.sendDeviceIcon = deviceIconSvg(
          event.receiverWeb ? 'laptop' : event.receiverDeviceType,
        );
        this.sendStatus = `Waiting for ${this.sendReceiver.name} to accept…`;
        break;

      case 'accepted':
        this.sendPhase = 'accepted';
        this.sendStatus = 'Accepted — delivering…';
        break;

      case 'transferStarted':
        this.sendPhase = 'accepted';
        this.sendProgressPct = 0;
        this.sendStatus = 'Sending file over relay…';
        break;

      case 'progress': {
        this.sendPhase = 'accepted';
        this.sendProgressPct = event.totalBytes
          ? Math.min(100, (event.bytesSent / event.totalBytes) * 100)
          : 0;
        this.sendStatus = `${formatBytes(event.bytesSent)} / ${formatBytes(event.totalBytes)}`;
        break;
      }

      case 'completed':
        this.sendPhase = 'done';
        this.sendStatus = 'Sent ✓';
        this._sender = null;
        break;

      case 'declined':
        this.sendPhase = 'declined';
        this.sendStatus = event.reason
          ? `Declined: ${event.reason}`
          : 'Declined by the recipient.';
        this._sender = null;
        break;

      case 'cancelled':
        this.sendPhase = 'cancelled';
        this.sendStatus = 'Cancelled.';
        this._sender = null;
        break;

      case 'error':
        this.sendPhase = 'error';
        this.sendStatus = event.message;
        this._sender = null;
        break;

      default:
        console.warn('unknown send event', event);
    }
  },

  cancelSend() {
    if (this._sender) {
      this.sendStatus = 'Cancelling…';
      this._sender.cancel();
    }
  },

  // Return to the form to send again (keeps the code, clears text + files).
  resetSend() {
    this.sendPhase = 'idle';
    this.sendStatus = '';
    this.sendProgressPct = 0;
    this.sendForm.text = '';
    this.clearFiles();
    this._sender = null;
  },

  get sendBusy() {
    return (
      this.sendPhase === 'connecting' ||
      this.sendPhase === 'waiting' ||
      this.sendPhase === 'accepted'
    );
  },
});

// Kick off the wasm module load once; both halves await this before touching
// WebReceiver / WebSender.
const wasmReady = init();

window.Alpine = Alpine;
Alpine.start();

// Land on receive by default (the "receive in a browser" entry), or send when
// linked with ?mode=send — which, thanks to the lazy receiver, registers no
// pairing code for a send-only visit.
// Mirror the theme the inline <head> script already applied, so the toggle
// glyph starts correct.
Alpine.store('rx').theme =
  document.documentElement.getAttribute('data-theme') === 'light' ? 'light' : 'dark';

const initialMode = params.get('mode') === 'send' ? 'send' : 'receive';
Alpine.store('rx').setMode(initialMode);
