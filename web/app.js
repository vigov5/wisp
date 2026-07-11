import init, { WebReceiver } from './pkg/wisp_web_receiver.js';

// Rendezvous server the browser registers with. Override with ?rendezvous=... for
// local testing (e.g. ?rendezvous=http://localhost:8787). File bytes never touch
// this server — only the ~10 KB code/ticket handshake does.
const params = new URLSearchParams(location.search);
const RENDEZVOUS_URL =
  params.get('rendezvous') || 'https://rendezvous.wisp.mooo.com';

const $ = (id) => document.getElementById(id);
const show = (id) => $(id).classList.remove('hidden');

function setStatus(text) {
  $('status').textContent = text;
}

let totalBytes = 0;

function onEvent(event) {
  switch (event.type) {
    case 'registered':
      $('code').textContent = event.code;
      show('code-section');
      setStatus('Waiting for a sender…');
      break;

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
      if (event.inlineText != null) {
        setStatus('Received text message.');
      } else {
        setStatus(`Accepting ${event.files.length} file(s), ${formatBytes(totalBytes)}…`);
      }
      break;
    }

    case 'transferStarted':
      setStatus('Transfer started — downloading over relay…');
      break;

    case 'progress': {
      // The blob fetch reports total wire bytes (collection metadata + file
      // payload), which can exceed the manifest's file-content total, so clamp
      // for display.
      const received = Math.min(event.bytesReceived, totalBytes);
      const pct = totalBytes ? Math.min(100, (received / totalBytes) * 100) : 0;
      $('progress-bar').style.width = `${pct}%`;
      $('progress-text').textContent =
        `${formatBytes(received)} / ${formatBytes(totalBytes)}`;
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

    case 'completed':
      $('progress-bar').style.width = '100%';
      setStatus('Transfer complete ✓');
      break;

    case 'error':
      setStatus(`Error: ${event.message}`);
      break;

    default:
      console.warn('unknown event', event);
  }
}

function formatBytes(n) {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let v = n, u = 0;
  while (v >= 1024 && u < units.length - 1) { v /= 1024; u++; }
  return u === 0 ? `${n} B` : `${v.toFixed(1)} ${units[u]}`;
}

async function main() {
  setStatus('Loading…');
  await init();
  try {
    const receiver = await WebReceiver.start(RENDEZVOUS_URL, onEvent);
    console.log('receiver started, code:', receiver.code());
  } catch (err) {
    setStatus(`Failed to start: ${err}`);
    console.error(err);
  }
}

main();
