package dev.vigov5.wisp

import android.app.Activity
import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.VpnService
import android.hardware.usb.UsbAccessory
import android.hardware.usb.UsbConstants
import android.hardware.usb.UsbDevice
import android.hardware.usb.UsbDeviceConnection
import android.hardware.usb.UsbEndpoint
import android.hardware.usb.UsbInterface
import android.hardware.usb.UsbManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.ParcelFileDescriptor
import android.system.Os
import android.system.OsConstants
import android.system.StructPollfd
import io.flutter.plugin.common.BinaryMessenger
import io.flutter.plugin.common.EventChannel
import io.flutter.plugin.common.MethodChannel
import java.io.FileInputStream
import java.io.FileOutputStream
import java.nio.charset.StandardCharsets
import java.util.concurrent.atomic.AtomicBoolean

/**
 * Direct phone-to-phone transfer over a USB cable via the Android Open
 * Accessory (AOA) protocol — the only no-root way to move bytes between two
 * stock Android phones over a single cable (USB tethering can't: Android ships
 * only the RNDIS/NCM *gadget* side, not the host-side driver to consume another
 * phone's tether).
 *
 * Roles, decided by which phone is the USB host:
 *  - **Host** (the sending phone): enumerates the bus, runs the AOA handshake
 *    (control requests 51/52/53) to switch the peer into accessory mode,
 *    re-opens it as a `0x2D0x` accessory device, claims the bulk IN/OUT
 *    endpoints, and reads/writes via [UsbDeviceConnection.bulkTransfer].
 *  - **Accessory** (the receiving phone): Android matches our
 *    `res/xml/usb_accessory_filter.xml` against the host's identifying strings,
 *    launches us with `USB_ACCESSORY_ATTACHED`, and we [UsbManager.openAccessory]
 *    to get a [ParcelFileDescriptor] we read/write as plain streams.
 *
 * The established link is exposed two ways: a byte sink/source to Dart (for the
 * Phase-1 hardware spike — prove bytes round-trip) and, later, an in-process
 * [AoaLink] the VpnService pump copies TUN packets through (path A) without a
 * round trip through Dart/FFI.
 */
class UsbAoaChannel(private val context: Context) {

    companion object {
        const val METHOD_CHANNEL = "dev.vigov5.wisp/usb_aoa"
        const val EVENT_CHANNEL = "dev.vigov5.wisp/usb_aoa/events"

        // AOA identifying strings the host sends (request 52). These MUST match
        // res/xml/usb_accessory_filter.xml so the peer's Android launches Wisp
        // (the same APK) in accessory mode.
        private const val AOA_MANUFACTURER = "vigov5"
        private const val AOA_MODEL = "WispCable"
        private const val AOA_DESCRIPTION = "Wisp direct USB transfer"
        private const val AOA_VERSION = "1.0"
        private const val AOA_URI = "https://github.com/vigov5"
        private const val AOA_SERIAL = "wisp-aoa-0001"

        // Google's vendor id + the product ids a device reports once it has
        // entered accessory mode (with/without adb, with/without audio).
        private const val GOOGLE_VID = 0x18D1
        private val ACCESSORY_PIDS = setOf(0x2D00, 0x2D01, 0x2D04, 0x2D05)

        // AOA control requests (USB vendor requests on the default endpoint).
        private const val REQ_GET_PROTOCOL = 51
        private const val REQ_SEND_STRING = 52
        private const val REQ_START = 53

        // String indices for request 52.
        private const val IDX_MANUFACTURER = 0
        private const val IDX_MODEL = 1
        private const val IDX_DESCRIPTION = 2
        private const val IDX_VERSION = 3
        private const val IDX_URI = 4
        private const val IDX_SERIAL = 5

        private const val ACTION_USB_PERMISSION = "dev.vigov5.wisp.USB_PERMISSION"

        // bulkTransfer / stream read budget per call.
        private const val READ_BUF = 32 * 1024
        // Max bytes coalesced into a single TUN→AOA bulk write. Kept ≤16KB —
        // some controllers mishandle larger single bulk transfers.
        private const val BULK_BATCH_BYTES = 16 * 1024
        // Host bulk-WRITE timeout. Deliberately large: a write only stalls when
        // the accessory is briefly behind on draining the pipe (backpressure),
        // which resolves in well under this window. We must NOT use a short
        // timeout + retry — bulkTransfer can send some packets before timing
        // out, so resending the batch would duplicate bytes and desync the
        // length-framed stream permanently. So: wait it out; a return <0 then
        // means a real failure (cable pulled), which correctly ends the pump.
        private const val BULK_WRITE_TIMEOUT_MS = 8000
        // Control transfer timeout for the AOA handshake.
        private const val CONTROL_TIMEOUT_MS = 1000

        // Retry window for opening the accessory / re-enumerated host device:
        // after the AOA switch the other side needs a moment to be ready.
        private const val ACCESSORY_OPEN_ATTEMPTS = 15
        private const val ACCESSORY_OPEN_RETRY_MS = 200L
        private const val HOST_OPEN_ATTEMPTS = 15
        private const val HOST_OPEN_RETRY_MS = 200L

        // Point-to-point /30 over the cable. The USB host takes .1, the
        // accessory .2; iroh binds/dials these like any LAN address.
        const val TUNNEL_HOST_IP = "10.42.0.1"
        const val TUNNEL_ACCESSORY_IP = "10.42.0.2"
        const val TUNNEL_SUBNET = "10.42.0.0"
        const val TUNNEL_PREFIX = 30
        const val TUNNEL_MTU = 1280
        const val EXTRA_ROLE = "role"

        fun tunnelIpForRole(role: String?): String =
            if (role == "host") TUNNEL_HOST_IP else TUNNEL_ACCESSORY_IP

        const val REQUEST_CODE_VPN_CONSENT = 4810
    }

    private val usbManager: UsbManager =
        context.getSystemService(Context.USB_SERVICE) as UsbManager
    private val mainHandler = Handler(Looper.getMainLooper())

    private var eventSink: EventChannel.EventSink? = null
    private var permissionReceiver: BroadcastReceiver? = null
    private var accessoryReceiver: BroadcastReceiver? = null
    // Guards against two concurrent accessory-open attempts (broadcast auto-open
    // racing the explicit connectAccessory call).
    private val accessoryConnecting = AtomicBoolean(false)
    // Set when the activity was launched/resumed by an accessory-attach intent
    // so the Dart side can route straight into the receive flow. Consumed once.
    @Volatile
    private var pendingAccessoryAttach = false

    @Volatile
    private var link: AoaLink? = null

    // IP-over-AOA tunnel (path A). When up, inbound AOA bytes are de-framed into
    // IP packets and written to the TUN, and TUN packets are framed out over the
    // link. iroh then runs over the TUN exactly as on Wi-Fi.
    @Volatile
    private var inboundConsumer: ((ByteArray) -> Unit)? = null
    @Volatile
    private var tunnelFd: ParcelFileDescriptor? = null
    private var tunnelReader: Thread? = null
    private var pendingTunnelResult: MethodChannel.Result? = null

    fun configure(messenger: BinaryMessenger) {
        MethodChannel(messenger, METHOD_CHANNEL).setMethodCallHandler { call, result ->
            when (call.method) {
                // Returns the connected role ("host"/"accessory") or null. Lets
                // the UI know a cable peer is live.
                "state" -> result.success(link?.role)
                // Host side: is there a USB device we could drive into accessory
                // mode? (Any device on the bus — phone-to-phone has exactly one.)
                "hasHostDevice" -> result.success(findFirstDevice() != null)
                // Host side: request permission (if needed) then run the AOA
                // handshake and connect. Async; resolves true once the bulk
                // endpoints are claimed.
                "connectHost" -> connectHost(result)
                // Accessory side: open the accessory delivered by the launch
                // intent (or the currently attached one).
                "connectAccessory" -> connectAccessory(result)
                // True once if this launch came from an accessory-attach intent
                // (the receiving phone was just plugged into a sending Wisp).
                "consumeAccessoryAttach" -> {
                    val pending = pendingAccessoryAttach
                    pendingAccessoryAttach = false
                    result.success(pending)
                }
                // Spike helper: write bytes over the live link.
                "send" -> {
                    val bytes = call.arguments as? ByteArray
                    val current = link
                    if (bytes == null || current == null) {
                        result.success(false)
                    } else {
                        current.write(bytes, 0, bytes.size)
                        result.success(true)
                    }
                }
                "disconnect" -> {
                    closeLink()
                    result.success(true)
                }
                // Bring up the IP-over-AOA tunnel (VpnService). Requires a live
                // link. May trigger the one-time VpnService consent dialog.
                "startTunnel" -> startTunnel(result)
                "stopTunnel" -> {
                    stopTunnelService()
                    result.success(true)
                }
                "tunnelLocalIp" -> result.success(tunnelLocalIp())
                else -> result.notImplemented()
            }
        }

        EventChannel(messenger, EVENT_CHANNEL).setStreamHandler(
            object : EventChannel.StreamHandler {
                override fun onListen(arguments: Any?, events: EventChannel.EventSink?) {
                    eventSink = events
                }

                override fun onCancel(arguments: Any?) {
                    eventSink = null
                }
            },
        )

        registerAccessoryReceiver()
        AoaBridge.channel = this
    }

    fun dispose() {
        stopTunnelService()
        closeLink()
        permissionReceiver?.let { runCatching { context.unregisterReceiver(it) } }
        permissionReceiver = null
        accessoryReceiver?.let { runCatching { context.unregisterReceiver(it) } }
        accessoryReceiver = null
        eventSink = null
        if (AoaBridge.channel === this) AoaBridge.channel = null
    }

    // Auto-connect as accessory the moment the cable switches us into accessory
    // mode — even when the page is already open (the launch intent only fires on
    // a cold start). This removes the manual "Connect as accessory" tap on the
    // receiving phone; the attach broadcast also carries the permission grant.
    private fun registerAccessoryReceiver() {
        if (accessoryReceiver != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(ctx: Context, intent: Intent) {
                when (intent.action) {
                    UsbManager.ACTION_USB_ACCESSORY_ATTACHED -> tryOpenAttachedAccessory()
                    UsbManager.ACTION_USB_ACCESSORY_DETACHED -> {
                        if (link?.role == "accessory") closeLink()
                    }
                    // The host's bulkTransfer read can't distinguish a timeout
                    // from a yanked cable, so rely on the detach broadcast to
                    // close the link + tear down the tunnel cleanly (otherwise
                    // the link hangs and the status lies about being connected).
                    //
                    // BUT: during the AOA switch the *pre-switch* device detaches
                    // and re-enumerates as a 0x2D0x accessory. That transient
                    // detach must NOT kill the link we then build over the
                    // accessory-mode device — so only treat a detach of an
                    // accessory-mode device as a real cable pull. (When the OEM
                    // doesn't populate the device extra, the Dart liveness poll
                    // backstops unplug detection.)
                    UsbManager.ACTION_USB_DEVICE_DETACHED -> {
                        val detached = deviceFromIntent(intent)
                        val accessoryMode = detached != null &&
                            detached.vendorId == GOOGLE_VID &&
                            ACCESSORY_PIDS.contains(detached.productId)
                        if (link?.role == "host" && accessoryMode) closeLink()
                    }
                }
            }
        }
        accessoryReceiver = receiver
        val filter = IntentFilter().apply {
            addAction(UsbManager.ACTION_USB_ACCESSORY_ATTACHED)
            addAction(UsbManager.ACTION_USB_ACCESSORY_DETACHED)
            addAction(UsbManager.ACTION_USB_DEVICE_DETACHED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            context.registerReceiver(receiver, filter)
        }
    }

    private fun tryOpenAttachedAccessory() {
        openAccessoryWithRetry(null)
    }

    @Suppress("DEPRECATION")
    private fun deviceFromIntent(intent: Intent): UsbDevice? =
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE, UsbDevice::class.java)
        } else {
            intent.getParcelableExtra(UsbManager.EXTRA_DEVICE)
        }

    /**
     * Open the attached accessory, retrying for a short window. After the host
     * drives the AOA switch, the accessory side often isn't ready for the first
     * ~hundreds of ms — `openAccessory` returns null or permission hasn't landed
     * yet — so a single attempt almost always loses the race. Retries on a
     * worker thread (never blocks the caller / main thread).
     */
    private fun openAccessoryWithRetry(onResult: ((Boolean, String?) -> Unit)?) {
        if (link != null) {
            onResult?.invoke(true, null)
            return
        }
        if (!accessoryConnecting.compareAndSet(false, true)) {
            // Another attempt is already in flight; let it report.
            onResult?.invoke(link != null, "accessory connect already in progress")
            return
        }
        Thread {
            var lastError: String? = "no accessory attached"
            try {
                repeat(ACCESSORY_OPEN_ATTEMPTS) {
                    if (link != null) {
                        onResult?.invoke(true, null)
                        return@Thread
                    }
                    val accessory = usbManager.accessoryList?.firstOrNull()
                    when {
                        accessory == null -> lastError = "no accessory attached"
                        !usbManager.hasPermission(accessory) -> lastError = "no accessory permission yet"
                        else -> {
                            val fd = try {
                                usbManager.openAccessory(accessory)
                            } catch (t: Throwable) {
                                lastError = t.message ?: "openAccessory threw"
                                null
                            }
                            if (fd != null) {
                                installLink(AccessoryLink(fd))
                                onResult?.invoke(true, null)
                                return@Thread
                            }
                            if (lastError == null) lastError = "openAccessory returned null"
                        }
                    }
                    Thread.sleep(ACCESSORY_OPEN_RETRY_MS)
                }
                onResult?.invoke(false, lastError)
            } finally {
                accessoryConnecting.set(false)
            }
        }.apply { isDaemon = true; name = "aoa-accessory-open" }.start()
    }

    // --- IP-over-AOA tunnel (path A) ---------------------------------------

    private fun startTunnel(result: MethodChannel.Result) {
        if (link == null) {
            result.error("NO_LINK", "No USB link to tunnel over", null)
            return
        }
        val consent = VpnService.prepare(context)
        if (consent == null) {
            launchTunnelService()
            result.success(true)
            return
        }
        val activity = context as? Activity
        if (activity == null) {
            result.error("NO_ACTIVITY", "Cannot request VPN consent without an activity", null)
            return
        }
        pendingTunnelResult = result
        activity.startActivityForResult(consent, REQUEST_CODE_VPN_CONSENT)
    }

    /** Called by MainActivity.onActivityResult for the VPN consent dialog. */
    fun onVpnConsentResult(granted: Boolean) {
        val result = pendingTunnelResult
        pendingTunnelResult = null
        if (granted) {
            launchTunnelService()
            result?.success(true)
        } else {
            result?.error("VPN_CONSENT_DENIED", "User denied VPN permission", null)
        }
    }

    private fun launchTunnelService() {
        // Plain startService (not startForegroundService): we are foreground
        // here (user just consented), and VpnService.establish() raises the
        // system VPN notification itself, so no app foreground notification is
        // needed for the link's lifetime while the app is alive.
        val intent = Intent(context, UsbTunnelVpnService::class.java)
            .putExtra(EXTRA_ROLE, link?.role)
        context.startService(intent)
    }

    private fun stopTunnelService() {
        runCatching { context.stopService(Intent(context, UsbTunnelVpnService::class.java)) }
        endTunnel()
    }

    /** Called by [UsbTunnelVpnService.onDestroy] so a system-killed service
     *  still tears the pump down. Idempotent with [endTunnel]. */
    fun onTunnelServiceDestroyed() = endTunnel()

    private fun tunnelLocalIp(): String? =
        if (tunnelFd != null) tunnelIpForRole(link?.role) else null

    /**
     * Wire the established TUN to the live AOA link. Called by
     * [UsbTunnelVpnService] once it has a TUN fd. Inbound AOA bytes are
     * de-framed into IP packets and written to the TUN; TUN packets are
     * length-framed out over the link. Takes ownership of [pfd].
     */
    fun beginTunnel(pfd: ParcelFileDescriptor) {
        val current = link ?: run { runCatching { pfd.close() }; return }
        endTunnel()
        tunnelFd = pfd
        val tunOut = FileOutputStream(pfd.fileDescriptor)
        val reassembler = FrameReassembler { packet ->
            runCatching { tunOut.write(packet) }
        }
        // AOA→TUN: de-frame inbound bulk bytes into packets; flush once per
        // bulk chunk (not per packet) to cut syscalls.
        inboundConsumer = { bytes ->
            reassembler.feed(bytes)
            runCatching { tunOut.flush() }
        }

        val reader = Thread {
            val fd = pfd.fileDescriptor
            val tunIn = FileInputStream(fd)
            // poll()-driven drain: read every currently-queued packet into one
            // bulk write to amortize USB per-transfer overhead (the main
            // throughput lever). Reads stay BLOCKING — we never set O_NONBLOCK,
            // because that flag is shared with the write side (tunOut) and would
            // make TUN injection fail with EAGAIN under load. poll(0) only tells
            // us whether another packet is already queued.
            val pollFd = StructPollfd().apply {
                this.fd = fd
                events = OsConstants.POLLIN.toShort()
            }
            val pollFds = arrayOf(pollFd)
            val packet = ByteArray(TUNNEL_MTU + 64)
            val batch = java.io.ByteArrayOutputStream(BULK_BATCH_BYTES + TUNNEL_MTU)
            try {
                while (!Thread.currentThread().isInterrupted && tunnelFd === pfd) {
                    // Block up to 1s until readable; timeout re-checks the flag.
                    pollFd.revents = 0
                    Os.poll(pollFds, 1000)
                    if ((pollFd.revents.toInt() and OsConstants.POLLIN) == 0) continue
                    batch.reset()
                    // First read is ready (poll said so), so it won't block.
                    var n = tunIn.read(packet)
                    if (n < 0) break
                    appendFrame(batch, packet, n)
                    // Drain further queued packets, each gated by a non-blocking
                    // poll(0) so the blocking read never actually blocks.
                    while (batch.size() < BULK_BATCH_BYTES) {
                        pollFd.revents = 0
                        Os.poll(pollFds, 0)
                        if ((pollFd.revents.toInt() and OsConstants.POLLIN) == 0) break
                        n = tunIn.read(packet)
                        if (n <= 0) break
                        appendFrame(batch, packet, n)
                    }
                    val out = batch.toByteArray()
                    current.write(out, 0, out.size)
                }
            } catch (_: Throwable) {
                // link/tun closed (fd closed in endTunnel), or a transient bulk
                // error. A bulk hiccup is NOT a disconnect, so don't tear the
                // whole link down here (doing so killed the VPN the instant a
                // single write timed out). A real cable pull is caught by the
                // DEVICE_DETACHED broadcast (accessory-mode device) and the Dart
                // liveness poll.
            } finally {
                mainHandler.post { eventSink?.success(mapOf("event" to "tunnelClosed")) }
            }
        }.apply { isDaemon = true; name = "aoa-tun-reader" }
        tunnelReader = reader
        reader.start()
        startKeepalive()
        mainHandler.post {
            eventSink?.success(mapOf("event" to "tunnelUp", "ip" to tunnelIpForRole(current.role)))
        }
    }

    private fun appendFrame(out: java.io.ByteArrayOutputStream, packet: ByteArray, len: Int) {
        out.write((len shr 8) and 0xFF)
        out.write(len and 0xFF)
        out.write(packet, 0, len)
    }

    private fun endTunnel() {
        stopKeepalive()
        inboundConsumer = null
        tunnelReader?.interrupt()
        tunnelReader = null
        tunnelFd?.let { runCatching { it.close() } }
        tunnelFd = null
    }

    // Hold a wake lock (reusing the transfer keepalive FGS) while the cable
    // tunnel is up, so the pump keeps running with the screen off.
    private fun startKeepalive() {
        val intent = Intent(context, TransferKeepaliveService::class.java)
            .putExtra(TransferKeepaliveService.EXTRA_TITLE, "Wisp USB cable")
            .putExtra(TransferKeepaliveService.EXTRA_BODY, "Direct transfer over USB is active")
        runCatching {
            if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
                context.startForegroundService(intent)
            } else {
                context.startService(intent)
            }
        }
    }

    private fun stopKeepalive() {
        runCatching {
            context.startService(
                Intent(context, TransferKeepaliveService::class.java)
                    .setAction(TransferKeepaliveService.ACTION_STOP),
            )
        }
    }

    /**
     * Reassembles length-prefixed frames ([u16 len][payload]) from an arbitrary
     * byte stream and emits each complete payload (one IP packet).
     */
    private class FrameReassembler(private val onPacket: (ByteArray) -> Unit) {
        private var buffer = ByteArray(0)

        fun feed(chunk: ByteArray) {
            buffer = if (buffer.isEmpty()) chunk else buffer + chunk
            var offset = 0
            while (buffer.size - offset >= 2) {
                val len = ((buffer[offset].toInt() and 0xFF) shl 8) or
                    (buffer[offset + 1].toInt() and 0xFF)
                if (buffer.size - offset - 2 < len) break
                onPacket(buffer.copyOfRange(offset + 2, offset + 2 + len))
                offset += 2 + len
            }
            buffer = if (offset == 0) buffer else buffer.copyOfRange(offset, buffer.size)
        }
    }

    /** True when the launch/new intent is an AOA accessory attach for us. */
    fun isAccessoryAttachIntent(intent: Intent?): Boolean =
        intent?.action == UsbManager.ACTION_USB_ACCESSORY_ATTACHED

    /** Record an accessory-attach launch so Dart can consume it once attached. */
    fun notifyIntent(intent: Intent?) {
        if (isAccessoryAttachIntent(intent)) pendingAccessoryAttach = true
    }

    // --- Host role ---------------------------------------------------------

    private fun findFirstDevice(): UsbDevice? = usbManager.deviceList.values.firstOrNull()

    private fun connectHost(result: MethodChannel.Result) {
        val device = findFirstDevice()
        if (device == null) {
            result.error("NO_DEVICE", "No USB device attached", null)
            return
        }
        if (usbManager.hasPermission(device)) {
            startHostHandshake(device, result)
            return
        }
        requestPermission(device, result)
    }

    private fun requestPermission(device: UsbDevice, result: MethodChannel.Result) {
        val flags = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            PendingIntent.FLAG_MUTABLE
        } else {
            0
        }
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(ctx: Context, intent: Intent) {
                if (intent.action != ACTION_USB_PERMISSION) return
                runCatching { context.unregisterReceiver(this) }
                if (permissionReceiver === this) permissionReceiver = null
                val granted = intent.getBooleanExtra(UsbManager.EXTRA_PERMISSION_GRANTED, false)
                if (granted) {
                    startHostHandshake(device, result)
                } else {
                    result.error("PERMISSION_DENIED", "USB permission denied", null)
                }
            }
        }
        permissionReceiver = receiver
        val filter = IntentFilter(ACTION_USB_PERMISSION)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            context.registerReceiver(receiver, filter)
        }
        val pending = PendingIntent.getBroadcast(
            context,
            0,
            Intent(ACTION_USB_PERMISSION).setPackage(context.packageName),
            flags,
        )
        usbManager.requestPermission(device, pending)
    }

    /**
     * Drive [device] into accessory mode then connect. If the device is already
     * an accessory (Google VID + 0x2D0x PID) we connect directly; otherwise we
     * send the AOA handshake and the device re-enumerates — the subsequent
     * attach is handled by [onHostDeviceReattached].
     */
    private fun startHostHandshake(device: UsbDevice, result: MethodChannel.Result) {
        Thread {
            try {
                if (device.vendorId == GOOGLE_VID && ACCESSORY_PIDS.contains(device.productId)) {
                    openHostLink(device)
                    mainHandler.post { result.success(true) }
                    return@Thread
                }
                val connection = usbManager.openDevice(device)
                    ?: throw IllegalStateException("openDevice returned null")
                try {
                    val protocol = getAoaProtocol(connection)
                    if (protocol < 1) {
                        throw IllegalStateException("peer does not support AOA (protocol=$protocol)")
                    }
                    sendAoaStrings(connection)
                    startAccessoryMode(connection)
                } finally {
                    connection.close()
                }
                // The device now detaches and re-attaches as a 0x2D0x
                // accessory. Wait for it, then claim the bulk endpoints.
                val accessoryDevice = awaitReattach()
                    ?: throw IllegalStateException("device did not re-enumerate as accessory")
                openHostLink(accessoryDevice)
                mainHandler.post { result.success(true) }
            } catch (t: Throwable) {
                mainHandler.post {
                    result.error("HOST_CONNECT_FAILED", t.message, null)
                }
            }
        }.start()
    }

    private fun getAoaProtocol(connection: UsbDeviceConnection): Int {
        val buffer = ByteArray(2)
        val len = connection.controlTransfer(
            UsbConstants.USB_DIR_IN or UsbConstants.USB_TYPE_VENDOR,
            REQ_GET_PROTOCOL,
            0,
            0,
            buffer,
            buffer.size,
            CONTROL_TIMEOUT_MS,
        )
        if (len < 2) return 0
        // Little-endian u16.
        return (buffer[1].toInt() and 0xFF shl 8) or (buffer[0].toInt() and 0xFF)
    }

    private fun sendAoaStrings(connection: UsbDeviceConnection) {
        sendString(connection, IDX_MANUFACTURER, AOA_MANUFACTURER)
        sendString(connection, IDX_MODEL, AOA_MODEL)
        sendString(connection, IDX_DESCRIPTION, AOA_DESCRIPTION)
        sendString(connection, IDX_VERSION, AOA_VERSION)
        sendString(connection, IDX_URI, AOA_URI)
        sendString(connection, IDX_SERIAL, AOA_SERIAL)
    }

    private fun sendString(connection: UsbDeviceConnection, index: Int, value: String) {
        val bytes = (value + " ").toByteArray(StandardCharsets.US_ASCII)
        val sent = connection.controlTransfer(
            UsbConstants.USB_DIR_OUT or UsbConstants.USB_TYPE_VENDOR,
            REQ_SEND_STRING,
            0,
            index,
            bytes,
            bytes.size,
            CONTROL_TIMEOUT_MS,
        )
        if (sent < 0) throw IllegalStateException("AOA send string $index failed")
    }

    private fun startAccessoryMode(connection: UsbDeviceConnection) {
        val sent = connection.controlTransfer(
            UsbConstants.USB_DIR_OUT or UsbConstants.USB_TYPE_VENDOR,
            REQ_START,
            0,
            0,
            null,
            0,
            CONTROL_TIMEOUT_MS,
        )
        if (sent < 0) throw IllegalStateException("AOA start accessory failed")
    }

    /** Poll the bus (the device re-enumerates with a new PID) for up to ~3s. */
    private fun awaitReattach(): UsbDevice? {
        val deadline = System.nanoTime() + 3_000_000_000L
        while (System.nanoTime() < deadline) {
            val accessory = usbManager.deviceList.values.firstOrNull {
                it.vendorId == GOOGLE_VID && ACCESSORY_PIDS.contains(it.productId)
            }
            if (accessory != null) {
                if (usbManager.hasPermission(accessory)) return accessory
                // Permission for the pre-switch device does not carry over.
                // A real flow re-requests here; surfaced as a failure for now.
                return accessory
            }
            Thread.sleep(100)
        }
        return null
    }

    private fun openHostLink(device: UsbDevice) {
        // The re-enumerated accessory device needs a moment to settle after the
        // AOA switch; openDevice returns null until then. Retry instead of
        // failing the whole connect (which forced the user to retry manually).
        var connection: UsbDeviceConnection? = null
        var attempts = 0
        while (connection == null && attempts < HOST_OPEN_ATTEMPTS) {
            connection = usbManager.openDevice(device)
            if (connection == null) Thread.sleep(HOST_OPEN_RETRY_MS)
            attempts++
        }
        val conn = connection
            ?: throw IllegalStateException("openDevice (accessory) returned null after retries")
        openHostLinkOn(conn, device)
    }

    private fun openHostLinkOn(connection: UsbDeviceConnection, device: UsbDevice) {
        val iface: UsbInterface = device.getInterface(0)
        if (!connection.claimInterface(iface, true)) {
            connection.close()
            throw IllegalStateException("claimInterface failed")
        }
        var bulkIn: UsbEndpoint? = null
        var bulkOut: UsbEndpoint? = null
        for (i in 0 until iface.endpointCount) {
            val ep = iface.getEndpoint(i)
            if (ep.type != UsbConstants.USB_ENDPOINT_XFER_BULK) continue
            if (ep.direction == UsbConstants.USB_DIR_IN) bulkIn = ep else bulkOut = ep
        }
        if (bulkIn == null || bulkOut == null) {
            connection.releaseInterface(iface)
            connection.close()
            throw IllegalStateException("missing bulk endpoints")
        }
        installLink(HostLink(connection, iface, bulkIn, bulkOut))
    }

    // --- Accessory role ----------------------------------------------------

    private fun connectAccessory(result: MethodChannel.Result) {
        openAccessoryWithRetry { ok, err ->
            mainHandler.post {
                if (ok) {
                    result.success(true)
                } else {
                    result.error("ACCESSORY_CONNECT_FAILED", err, null)
                }
            }
        }
    }

    // --- Shared link plumbing ----------------------------------------------

    private fun installLink(newLink: AoaLink) {
        closeLink()
        link = newLink
        newLink.startReadLoop(
            onData = { data -> dispatchInbound(data) },
            onClosed = {
                stopTunnelService()
                mainHandler.post {
                    eventSink?.success(mapOf("event" to "closed"))
                }
                if (link === newLink) link = null
            },
        )
        mainHandler.post { eventSink?.success(mapOf("event" to "connected", "role" to newLink.role)) }
    }

    // Inbound AOA bytes go to the tunnel reassembler when the IP tunnel is up,
    // otherwise to the Dart event sink (spike / raw byte surface).
    private fun dispatchInbound(data: ByteArray) {
        val consumer = inboundConsumer
        if (consumer != null) {
            consumer(data)
        } else {
            mainHandler.post { eventSink?.success(data) }
        }
    }

    private fun closeLink() {
        endTunnel()
        link?.close()
        link = null
    }

    /**
     * A live bidirectional AOA byte channel. Two implementations back the two
     * USB roles; both present the same read-loop + write surface so the spike
     * (Dart sink) and the future VpnService pump can share them.
     */
    private abstract class AoaLink(val role: String) {
        private val closed = AtomicBoolean(false)
        protected fun markClosed(): Boolean = closed.compareAndSet(false, true)
        fun isClosed(): Boolean = closed.get()

        abstract fun write(buf: ByteArray, off: Int, len: Int)
        abstract fun readInto(buf: ByteArray): Int
        abstract fun close()

        fun startReadLoop(onData: (ByteArray) -> Unit, onClosed: () -> Unit) {
            Thread {
                val buf = ByteArray(READ_BUF)
                try {
                    while (!isClosed()) {
                        val n = readInto(buf)
                        if (n < 0) break
                        if (n > 0) onData(buf.copyOf(n))
                    }
                } catch (_: Throwable) {
                    // fall through to onClosed
                } finally {
                    close()
                    onClosed()
                }
            }.apply { isDaemon = true }.start()
        }
    }

    private inner class HostLink(
        private val connection: UsbDeviceConnection,
        private val iface: UsbInterface,
        private val bulkIn: UsbEndpoint,
        private val bulkOut: UsbEndpoint,
    ) : AoaLink("host") {
        override fun write(buf: ByteArray, off: Int, len: Int) {
            var sent = 0
            // bulkTransfer wants an offset-0 buffer on older APIs; copy when needed.
            val payload = if (off == 0 && len == buf.size) buf else buf.copyOfRange(off, off + len)
            while (sent < len) {
                val n = connection.bulkTransfer(
                    bulkOut,
                    if (sent == 0) payload else payload.copyOfRange(sent, len),
                    len - sent,
                    BULK_WRITE_TIMEOUT_MS,
                )
                if (n <= 0) throw IllegalStateException("bulk write failed ($n)")
                sent += n
            }
        }

        override fun readInto(buf: ByteArray): Int {
            // Long timeout; returns -1 on transient timeout so the loop keeps
            // polling until the link is closed.
            val n = connection.bulkTransfer(bulkIn, buf, buf.size, 250)
            return if (n < 0) 0 else n
        }

        override fun close() {
            if (!markClosed()) return
            runCatching { connection.releaseInterface(iface) }
            runCatching { connection.close() }
        }
    }

    private inner class AccessoryLink(
        private val fd: ParcelFileDescriptor,
    ) : AoaLink("accessory") {
        private val input = FileInputStream(fd.fileDescriptor)
        private val output = FileOutputStream(fd.fileDescriptor)

        override fun write(buf: ByteArray, off: Int, len: Int) {
            output.write(buf, off, len)
            output.flush()
        }

        override fun readInto(buf: ByteArray): Int = input.read(buf)

        override fun close() {
            if (!markClosed()) return
            runCatching { input.close() }
            runCatching { output.close() }
            runCatching { fd.close() }
        }
    }
}

/**
 * Process-wide handoff between [UsbTunnelVpnService] (which owns the TUN
 * lifecycle + consent + notification) and the [UsbAoaChannel] that owns the AOA
 * link and runs the packet pump. Same process, so a plain reference suffices.
 */
object AoaBridge {
    @Volatile
    var channel: UsbAoaChannel? = null
}
