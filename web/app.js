import init, { WebReceiver } from './pkg/wisp_web_receiver.js';

// Rendezvous server the browser registers with. Override with ?rendezvous=... for
// local testing (e.g. ?rendezvous=http://localhost:8787). File bytes never touch
// this server — only the ~10 KB code/ticket handshake does.
const params = new URLSearchParams(location.search);
const RENDEZVOUS_URL =
  params.get('rendezvous') || 'https://rendezvous.wisp.mooo.com';

const $ = (id) => document.getElementById(id);
const show = (id) => $(id).classList.remove('hidden');
const hide = (id) => $(id).classList.add('hidden');

function setStatus(text) {
  $('status').textContent = text;
}

let receiver = null;
let totalBytes = 0;
let lastText = '';
let progressStart = 0;
let ttlTimer = null;

function startCountdown(iso) {
  const end = Date.parse(iso);
  if (ttlTimer) clearInterval(ttlTimer);
  if (!end) { $('ttl').textContent = ''; return; }
  const tick = () => {
    const remaining = Math.max(0, Math.floor((end - Date.now()) / 1000));
    const m = Math.floor(remaining / 60);
    const s = String(remaining % 60).padStart(2, '0');
    $('ttl').textContent =
      remaining > 0 ? `Code expires in ${m}:${s}` : 'Code expired — refreshing…';
    if (remaining <= 0) clearInterval(ttlTimer);
  };
  tick();
  ttlTimer = setInterval(tick, 1000);
}

const isUrl = (s) => /^https?:\/\/\S+$/i.test(s.trim());

function renderText(text) {
  lastText = text;
  show('text-section');
  $('text-content').textContent = text;
  const open = $('btn-open-link');
  if (isUrl(text)) {
    open.href = text.trim();
    open.classList.remove('hidden');
  } else {
    open.classList.add('hidden');
  }
}

// Deterministic colour + short label from a public key, mirroring the native
// identity badge (pubkey_visual.dart). Full token parity comes in Workstream A.
function pubkeyColor(pubkey) {
  let h = 0;
  for (let i = 0; i < pubkey.length; i++) h = (h * 31 + pubkey.charCodeAt(i)) >>> 0;
  return `hsl(${h % 360} 60% 45%)`;
}
function shortPubkey(pubkey) {
  return pubkey ? pubkey.slice(0, 6) : '';
}

function resetOffer() {
  hide('offer-section');
  hide('sender');
  hide('decision');
  hide('progress');
  $('file-list').innerHTML = '';
  $('progress-bar').style.width = '0%';
  $('progress-text').textContent = '';
}

function onEvent(event) {
  switch (event.type) {
    case 'registered':
      $('code').textContent = event.code;
      show('code-section');
      startCountdown(event.expiresAt);
      setStatus('Waiting for a sender…');
      break;

    case 'connecting': {
      resetOffer();
      show('offer-section');
      show('sender');
      const badge = $('sender-badge');
      badge.style.background = pubkeyColor(event.senderPubkey);
      badge.textContent = shortPubkey(event.senderPubkey);
      $('sender-name').textContent =
        `${event.senderName || 'Unknown device'} · ${event.senderDeviceType}`;
      setStatus('A sender is connecting…');
      break;
    }

    case 'offer': {
      show('offer-section');
      totalBytes = event.totalBytes;
      const list = $('file-list');
      list.innerHTML = '';
      for (const f of event.files) {
        const li = document.createElement('li');
        li.textContent = `${f.path} — ${formatBytes(f.size)}`;
        list.appendChild(li);
      }
      const warn = $('offer-warning');
      if (event.tooLarge) {
        warn.textContent =
          "This transfer is large and may exceed your browser tab's memory — it might fail.";
        warn.classList.remove('hidden');
      } else {
        warn.classList.add('hidden');
      }
      show('decision');
      setStatus(
        `Accept ${event.files.length} file(s), ${formatBytes(totalBytes)}?`,
      );
      break;
    }

    case 'transferStarted':
      hide('decision');
      show('progress');
      progressStart = Date.now();
      setStatus('Transfer started — downloading over relay…');
      break;

    case 'progress': {
      // Self-heal the UI: bytes are flowing, so the decision is settled even if
      // the transferStarted event was missed or arrived late.
      hide('decision');
      show('progress');
      // The blob fetch reports total wire bytes (collection metadata + file
      // payload), which can exceed the manifest's file-content total, so clamp
      // for display.
      const received = Math.min(event.bytesReceived, totalBytes);
      const pct = totalBytes ? Math.min(100, (received / totalBytes) * 100) : 0;
      $('progress-bar').style.width = `${pct}%`;
      const secs = Math.max(0.001, (Date.now() - progressStart) / 1000);
      const speed = received / secs;
      let line = `${formatBytes(received)} / ${formatBytes(totalBytes)}`;
      if (speed > 0) {
        line += ` · ${formatBytes(speed)}/s`;
        const remaining = Math.max(0, totalBytes - received);
        const eta = remaining / speed;
        if (received < totalBytes && isFinite(eta)) line += ` · ETA ${formatEta(eta)}`;
      }
      $('progress-text').textContent = line;
      break;
    }

    case 'fileReady': {
      show('downloads-section');
      const li = document.createElement('li');
      const a = document.createElement('a');
      a.href = event.url;
      a.download = event.path;
      a.textContent = `${event.path} (${formatBytes(event.size)})`;
      li.appendChild(a);
      $('download-list').appendChild(li);
      break;
    }

    case 'textReady':
      hide('decision');
      renderText(event.text);
      break;

    case 'completed':
      $('progress-bar').style.width = '100%';
      setStatus('Transfer complete ✓');
      break;

    case 'declined':
      resetOffer();
      setStatus('Declined. Waiting for a sender…');
      break;

    case 'cancelled':
      resetOffer();
      setStatus('Cancelled. Waiting for a sender…');
      break;

    case 'error':
      // The receiver stays live (accept loop keeps running), so clear any
      // in-flight offer/progress UI and fall back to the waiting state.
      resetOffer();
      setStatus(`${event.message} — waiting for a sender…`);
      break;

    default:
      console.warn('unknown event', event);
  }
}

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

function wireButtons() {
  $('btn-accept').addEventListener('click', () => {
    if (!receiver) return;
    // Hide the decision immediately; transferStarted/textReady take over next.
    hide('decision');
    setStatus('Accepted — starting…');
    receiver.accept();
  });
  $('btn-decline').addEventListener('click', () => {
    if (!receiver) return;
    hide('decision');
    receiver.decline();
  });
  $('btn-copy').addEventListener('click', async () => {
    try {
      await navigator.clipboard.writeText(lastText);
      setStatus('Copied to clipboard ✓');
    } catch {
      setStatus('Copy failed — select and copy manually.');
    }
  });
  $('btn-save-text').addEventListener('click', () => {
    const blob = new Blob([lastText], { type: 'text/plain' });
    const url = URL.createObjectURL(blob);
    const a = document.createElement('a');
    a.href = url;
    a.download = 'message.txt';
    a.click();
    URL.revokeObjectURL(url);
  });
  $('btn-cancel').addEventListener('click', () => {
    if (!receiver) return;
    setStatus('Cancelling…');
    receiver.cancel();
  });
}

async function main() {
  setStatus('Loading…');
  wireButtons();
  await init();
  try {
    receiver = await WebReceiver.start(RENDEZVOUS_URL, onEvent);
    console.log('receiver started, code:', receiver.code());
  } catch (err) {
    setStatus(`Failed to start: ${err}`);
    console.error(err);
  }
}

main();
