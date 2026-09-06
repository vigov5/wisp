/* Wisp web — where received bytes go.
 *
 * The wasm receiver hands over verified chunks as they arrive off the wire and
 * never keeps them, so this module decides what "saving a file" means in a
 * browser. Two implementations, picked per download:
 *
 *   1. Streamed. The page opens a stream, hands the readable half to the service
 *      worker, and points a hidden iframe at a URL the worker answers with it.
 *      The browser then writes the response to disk as an ordinary download,
 *      the way it would any other — so nothing is resident, the download shows
 *      up in the browser's own UI while it runs, and the size a transfer can be
 *      is bounded by the disk rather than by the tab.
 *
 *   2. Buffered. No service worker (or no transferable streams): chunks pile up
 *      in an array and become a Blob at the end. That is one copy of the payload
 *      instead of the three the old wasm-side path kept, but it is still a copy,
 *      so the receiver keeps warning about large transfers whenever this is the
 *      path in use — see `streaming()`.
 *
 * Both shapes are the sink object described in crates/web-receiver/src/sink.rs.
 */

/** Path segment, under the service worker's scope, that means "a download". */
const ROUTE = '__wisp-dl';
/** Chunks the stream buffers before `write()` stops resolving immediately. */
const QUEUE_CHUNKS = 64;
/** How long to wait for the worker to acknowledge a download it was handed. */
const HANDSHAKE_MS = 3000;
/** How long to wait for the browser to actually come and fetch it. */
const CLAIM_MS = 10000;

let registration = null;
if ('serviceWorker' in navigator) {
  navigator.serviceWorker.ready.then(
    (reg) => {
      registration = reg;
    },
    () => {},
  );
}

/* Whether this browser can hand a ReadableStream to another thread. Without it
 * the page would have to relay every chunk through postMessage and re-implement
 * backpressure by hand, which is not worth it — the Blob path is the fallback.
 * Probed once, on a channel that is closed immediately. */
let transferable = null;
function canTransferStreams() {
  if (transferable === null) {
    try {
      const stream = new ReadableStream();
      const channel = new MessageChannel();
      channel.port1.postMessage(stream, [stream]);
      channel.port1.close();
      channel.port2.close();
      transferable = true;
    } catch (err) {
      transferable = false;
    }
  }
  return transferable;
}

/** The last path segment, so a file sent from inside a folder still lands under
 *  a plain name (`album/photo.jpg` → `photo.jpg`). */
function baseName(path) {
  const parts = String(path).split(/[/\\]/);
  return parts[parts.length - 1] || 'download';
}

function newId() {
  if (crypto.randomUUID) return crypto.randomUUID();
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2)}`;
}

/** Resolve on the worker's next message of `type`, or reject on timeout. Both
 *  handshake steps have to be armed before the message that triggers them is
 *  sent, so each returns a promise the caller awaits later. */
function signal(port, type, timeoutMs) {
  return new Promise((resolve, reject) => {
    const onMessage = (event) => {
      if (!event.data || event.data.type !== type) return;
      port.removeEventListener('message', onMessage);
      resolve();
    };
    port.addEventListener('message', onMessage);
    port.start();
    setTimeout(() => {
      port.removeEventListener('message', onMessage);
      reject(new Error(`the service worker never reported "${type}"`));
    }, timeoutMs);
  });
}

async function openStreamed(filename, size) {
  const worker = navigator.serviceWorker.controller;
  if (!worker) throw new Error('no service worker is controlling this page');

  const id = newId();
  const stream = new TransformStream({}, { highWaterMark: QUEUE_CHUNKS });
  const writer = stream.writable.getWriter();

  // Two acknowledgements, both needed before this sink can be trusted with
  // bytes. Navigating before the worker holds the stream would 404; returning
  // before the browser has actually asked for it would leave the writer pushing
  // into a stream nobody reads, which stalls the transfer with no error to show.
  const channel = new MessageChannel();
  const registered = signal(channel.port1, 'registered', HANDSHAKE_MS);
  worker.postMessage(
    { type: 'wisp-download', id, filename: baseName(filename), size, body: stream.readable },
    [stream.readable, channel.port2],
  );
  const claimed = signal(channel.port1, 'claimed', CLAIM_MS);
  let frame = null;
  try {
    await registered;

    // A hidden iframe rather than a top-level navigation: the response is an
    // attachment, so nothing renders, but a navigation the browser declined to
    // download would take the transfer UI off screen with it.
    frame = document.createElement('iframe');
    frame.hidden = true;
    frame.src = new URL(
      `${ROUTE}/${id}/${encodeURIComponent(baseName(filename))}`,
      registration.scope,
    ).href;
    document.body.appendChild(frame);

    await claimed;
  } catch (err) {
    if (frame) frame.remove();
    // Whichever step didn't fail is still armed and will time out on its own;
    // swallow it so it doesn't surface as an unhandled rejection.
    registered.catch(() => {});
    claimed.catch(() => {});
    channel.port1.close();
    writer.abort(err).catch(() => {});
    throw err;
  }
  const done = () => {
    channel.port1.close();
    frame.remove();
  };

  /* A download the browser gives up on — refused as one automatic download too
   * many, or cancelled from its own UI — cancels the response stream, and that
   * surfaces here as the writable erroring, often with no reason at all. Say
   * what happened rather than passing an empty rejection up into the receiver. */
  const refused = (err) => {
    const detail = err && err.message ? `: ${err.message}` : '';
    return new Error(`the browser stopped the download${detail}`);
  };

  return {
    async write(chunk) {
      try {
        // `ready` is the backpressure: it stays pending while the queue is full,
        // which is what stops the fetch behind it from running ahead.
        await writer.ready;
        await writer.write(chunk);
      } catch (err) {
        done();
        throw refused(err);
      }
    },
    async close() {
      try {
        await writer.close();
      } catch (err) {
        done();
        throw refused(err);
      }
      done();
      return null; // already on disk; there is nothing for the UI to link to
    },
    async abort() {
      // Erroring the stream truncates the response, which the browser reports
      // as a failed download rather than leaving a short file looking whole.
      try {
        await writer.abort(new Error('transfer cancelled'));
      } catch (err) {
        /* already closed */
      }
      done();
    },
  };
}

function openBuffered(filename) {
  const name = baseName(filename);
  let parts = [];
  return {
    async write(chunk) {
      parts.push(chunk);
    },
    async close() {
      const blob = new Blob(parts, { type: 'application/octet-stream' });
      parts = [];
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = name;
      anchor.rel = 'noopener';
      document.body.appendChild(anchor);
      anchor.click();
      anchor.remove();
      // Handed back so the UI can offer the file again without a re-transfer;
      // it pins the bytes until Clear revokes it.
      return url;
    },
    async abort() {
      parts = [];
    },
  };
}

export const downloads = {
  /** Whether a download opened right now would go straight to disk. */
  streaming() {
    return !!(
      registration &&
      navigator.serviceWorker &&
      navigator.serviceWorker.controller &&
      canTransferStreams()
    );
  },

  /** Open a download. `size` is the final length when known, else null. */
  async create(filename, size) {
    if (this.streaming()) {
      try {
        return await openStreamed(filename, size);
      } catch (err) {
        // A worker that was evicted mid-session shouldn't fail the transfer.
        console.warn('streaming download unavailable, buffering instead', err);
      }
    }
    return openBuffered(filename);
  },
};
