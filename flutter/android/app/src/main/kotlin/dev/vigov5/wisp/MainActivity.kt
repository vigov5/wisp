package dev.vigov5.wisp

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ContentResolver
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageManager
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.ParcelFileDescriptor
import android.os.PowerManager
import android.os.SystemClock
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
import android.system.Os
import android.system.OsConstants
import android.util.Log
import android.view.WindowManager
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.documentfile.provider.DocumentFile
import androidx.lifecycle.lifecycleScope
import io.flutter.embedding.android.FlutterFragmentActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.FileOutputStream
import java.io.IOException
import java.util.concurrent.atomic.AtomicBoolean
import java.util.concurrent.atomic.AtomicLong
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.sync.withPermit
import kotlinx.coroutines.withContext

// Extends FlutterFragmentActivity (not FlutterActivity) so local_auth's
// BiometricPrompt can attach — it requires a FragmentActivity host.
class MainActivity : FlutterFragmentActivity() {

    companion object {
        private const val CHANNEL = "dev.vigov5.wisp/file_picker"
        private const val KEEPALIVE_CHANNEL = "dev.vigov5.wisp/transfer_keepalive"
        private const val SHARE_CHANNEL = "dev.vigov5.wisp/share_intent"
        private const val USB_TETHER_CHANNEL = "dev.vigov5.wisp/usb_tether"
        private const val MULTICAST_CHANNEL = "dev.vigov5.wisp/multicast_lock"
        private const val MULTICAST_TAG = "WispMdns"
        private const val REQUEST_CODE_PICK_FILES = 2001
        private const val REQUEST_CODE_PICK_FOLDER = 2002
        private const val REQUEST_CODE_PICK_SAVE_FOLDER = 2003
        private const val REQUEST_CODE_POST_NOTIF = 4801
        private const val OPEN_TAG = "WispOpenFolder"
        private const val SHARE_TAG = "WispShare"
        private const val PICK_TAG = "WispPick"
        private const val RECEIVE_TAG = "WispReceive"

        // Floor and ceiling for [openFdBudget], the number of descriptors
        // either direction of a transfer may hold open at once.
        //
        // The floor is what this used to be, fixed: 256.  That was chosen
        // against an assumed ~1024-descriptor table, and on a 1911-file folder
        // it left 1655 files copying into the cache — the exact cost the
        // descriptor path exists to remove.  The budget is now read from the
        // process's real limit, with 256 kept as the value we never go below
        // and 4096 as the point past which more descriptors buy nothing.
        private const val MIN_OPEN_FD_BUDGET = 256
        private const val MAX_OPEN_FD_BUDGET = 4096

        // How many MediaStore destinations to create, or publish, at once.
        //
        // Each one is a binder round trip into MediaProvider, and MediaProvider
        // serves concurrent transactions on its own threads — it sat at 87% of
        // a single core while this ran serially, so there was capacity on the
        // device that a serial loop could not reach.
        //
        // Not a measured optimum. The number to beat is 66.7 ms per file
        // (1911 files in 127.4 s), and both loops now log their elapsed time,
        // so the next run on a device says whether 8 was the right pick.
        private const val DESTINATION_CONCURRENCY = 8

        // MediaStore's on-disk name for a row whose IS_PENDING is still set.
        // Skipped by the folder walk; see [collectTreeFiles].
        private const val PENDING_MEDIA_PREFIX = ".pending-"

        // Depth limit for a transfer path built from a picked folder, matching
        // the core's own cap.  Guards against a provider reporting a cyclic or
        // absurdly deep tree.
        private const val MAX_TREE_DEPTH = 32

        // Minimum bytes copied between two "onPickProgress" events.  Throttles
        // the platform-channel chatter during a multi-GB copy to ~1 event per
        // 8 MB (≈375 events for a 3 GB file) while still animating smoothly.
        private const val PROGRESS_EMIT_BYTES = 8L * 1024 * 1024

        // Files resolved between two "onPickProgress" events, for the phase
        // that opens a descriptor per file rather than copying bytes. 16 puts
        // a 1911-file folder at ~120 events over ~27 s.
        private const val PROGRESS_EMIT_FILES = 16

        // Free space the fallback copy has to leave behind.  Filling /data
        // does not merely fail the copy: the platform starts killing
        // processes and the device stays wedged until something clears the
        // cache.  Sharing a 6.2 GB file into 6.3 GB of headroom did exactly
        // that — the copy ran to within 190 MB of the end of the disk and
        // took the app down with it.  Android's own low-storage threshold
        // sits near 500 MB, so stop short of it.
        private const val COPY_HEADROOM_BYTES = 512L * 1024 * 1024

        // How much a running copy may write between free-space checks.  The
        // pre-flight check covers providers that report a size; this catches
        // the ones that do not.
        private const val SPACE_RECHECK_BYTES = 32L * 1024 * 1024

        // Why a shared or picked source could not be prepared, as the Dart
        // side reads it.
        private const val REJECT_NO_SPACE = "no_space"
        private const val REJECT_UNREADABLE = "unreadable"
    }

    // The file_picker channel, kept so the copy coroutine can push
    // "onPickProgress" events back to Flutter while a pick is streaming.
    private var fileChannel: MethodChannel? = null

    private var pendingResult: MethodChannel.Result? = null
    private var pendingFolderResult: MethodChannel.Result? = null
    private var pendingSaveFolderResult: MethodChannel.Result? = null

    // Deferred holding the cached file paths produced from an ACTION_SEND /
    // ACTION_SEND_MULTIPLE intent.  The copy itself runs on Dispatchers.IO,
    // and Flutter awaits this when calling getInitialSharedFiles, so a
    // multi-hundred-megabyte share never blocks the main thread (or the
    // launch screen).
    private var initialSharedFilesJob: Deferred<Map<String, Any?>>? = null
    // Cold-start stash for an ACTION_SEND text/plain share (EXTRA_TEXT, no
    // EXTRA_STREAM).  Handed to Flutter once via getInitialSharedText.
    private var initialSharedText: String? = null
    private var shareChannel: MethodChannel? = null
    private var usbAoa: UsbAoaChannel? = null

    // Wi-Fi multicast lock, held while the app is foreground.
    //
    // Android's Wi-Fi driver filters inbound multicast when no app holds this
    // lock, to save power. Sending is unaffected, which is what made the bug
    // confusing: this device's mDNS announcements reached the desktop fine
    // while it never saw the desktop's queries or announcements, so LAN
    // discovery worked in neither direction. CHANGE_WIFI_MULTICAST_STATE was
    // already declared in the manifest for this; nothing had ever taken the
    // lock.
    //
    // Not reference counted: `setMulticastLockHeld` is idempotent so repeated
    // lifecycle callbacks cannot leak or over-release it.
    private var multicastLock: WifiManager.MulticastLock? = null

    // Monotonic suffix that keeps every cached copy in its own directory.
    // A millisecond timestamp alone is not unique: a large batch copies small
    // images far faster than 1 ms, so several land in the same directory, and
    // any two sharing a display name (or both falling back to "picked_file"
    // when DISPLAY_NAME is unavailable) resolve to one path — the second
    // silently overwrites the first and the draft carries a duplicate entry
    // instead of the file the user picked.
    private val pickedCopySeq = AtomicLong(0L)

    // Descriptors held open for sources the core reads straight from their
    // `content://` URI instead of a cache copy.  The core sees each one as
    // `/proc/self/fd/<n>` and the blob store references that path rather than
    // duplicating the bytes — but it reopens the path lazily every time it
    // serves, so the descriptor has to outlive the whole transfer, not just
    // the import.  Released together with the cache copies when the draft is
    // cleared (`releaseSendSources`).
    //
    // Set by the "cancelPick" channel call while a pick is still resolving.
    //
    // Checked between files rather than mid-file: a folder pick spends its time
    // in a loop of one binder round trip per file — 27-37 s for 1911 of them —
    // so a cancel lands within one of those, which is close enough to instant
    // and needs no way to interrupt a call already in flight.
    private val pickCancelled = AtomicBoolean(false)

    // Guarded by [sendFdLock]: resolves run on Dispatchers.IO (a share intent
    // can land while a pick is still resolving) while the release comes in on
    // the main thread.
    private val sendFdLock = Any()
    private val openSendFds = mutableListOf<ParcelFileDescriptor>()

    // How many descriptors one direction of a transfer may hold open.
    //
    // A quarter of the process's table, not half: send descriptors, receive
    // destinations, the Flutter engine, every socket and the blob store all
    // draw on the same limit, and exhausting it does not fail *here* — it
    // fails the next unrelated open anywhere in the app.  A quarter of the
    // common 1024-entry table is exactly the 256 this replaced, so no device
    // gets a smaller budget than before; a device with a larger table gets
    // proportionally more files sent without a copy.
    //
    // Logged once because it decides whether a large folder copies at all,
    // and the limit is not the same on every ROM.
    private val openFdBudget: Int by lazy {
        val limit = try {
            Os.sysconf(OsConstants._SC_OPEN_MAX)
        } catch (e: Exception) {
            Log.w(PICK_TAG, "cannot read the descriptor limit: ${e.message}")
            0L
        }
        val budget = (if (limit > 0L) limit / 4 else 0L)
            .coerceIn(MIN_OPEN_FD_BUDGET.toLong(), MAX_OPEN_FD_BUDGET.toLong())
            .toInt()
        Log.i(PICK_TAG, "descriptor limit $limit, budget $budget per direction")
        budget
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Before anything can reach the network: without the Android context
        // iroh's resolver cannot read the system DNS config and sends every
        // lookup to Google's public servers instead.
        WispNative.install(this)
        initialSharedFilesJob = extractSharedFilesAsync(intent)
        initialSharedText = extractSharedText(intent)
        // Instantiate before configureFlutterEngine so an accessory-attach
        // launch intent is recorded even though the channels wire up later.
        if (usbAoa == null) usbAoa = UsbAoaChannel(this)
        usbAoa?.notifyIntent(intent)
    }

    override fun onDestroy() {
        usbAoa?.dispose()
        usbAoa = null
        // The draft dies with the engine, so nothing will send through these
        // descriptors again; leaving them open would just hold the underlying
        // files (and the fd slots) for the life of the process.
        releaseSendSources()
        // A transfer cannot outlive the engine either, so any destination still
        // pending here belongs to one that will never finish.
        releaseReceiveDestinations(publish = false)
        super.onDestroy()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // Persist the new intent so any later getIntent() lookups see it
        // (otherwise we'd keep re-reading the original launch intent).
        setIntent(intent)
        usbAoa?.notifyIntent(intent)

        // Text shares carry EXTRA_TEXT and no EXTRA_STREAM — route them to the
        // Share-text flow instead of the (empty) file pipeline.
        val sharedText = extractSharedText(intent)
        if (sharedText != null) {
            val channel = shareChannel
            if (channel != null) {
                channel.invokeMethod("onSharedText", sharedText)
            } else {
                initialSharedText = sharedText
            }
            return
        }

        val deferred = extractSharedFilesAsync(intent) ?: return
        lifecycleScope.launch {
            val files = try {
                deferred.await()
            } catch (_: Exception) {
                return@launch
            }
            // A share that resolved nothing at all still has to travel: the
            // rejections are the only thing that explains the empty draft.
            if (files.values.all { (it as? List<*>).isNullOrEmpty() }) return@launch
            val channel = shareChannel
            if (channel != null) {
                channel.invokeMethod("onSharedFiles", files)
            } else {
                // Flutter side hasn't attached yet — fall back to the
                // cold-start stash so getInitialSharedFiles still picks
                // them up when it eventually wires up.
                initialSharedFilesJob = CompletableDeferred(files)
            }
        }
    }

    // Returns null when the intent isn't a share intent so the caller can
    // skip it entirely; otherwise kicks off the URI resolve on
    // Dispatchers.IO and returns the in-flight Deferred.  Tied to
    // lifecycleScope so the work is cancelled if the activity dies.
    private fun extractSharedFilesAsync(intent: Intent?): Deferred<Map<String, Any?>>? {
        if (intent == null) return null
        if (intent.action != Intent.ACTION_SEND &&
            intent.action != Intent.ACTION_SEND_MULTIPLE
        ) return null
        return lifecycleScope.async(Dispatchers.IO) {
            extractSharedFilesFromIntent(intent) ?: emptyShare()
        }
    }

    private fun emptyShare(): Map<String, Any?> = mapOf(
        "sources" to emptyList<Map<String, Any?>>(),
        "rejected" to emptyList<Map<String, Any?>>(),
    )

    // Returns the plain text of an ACTION_SEND text/plain share, or null when
    // the intent isn't a text share.  A file share (EXTRA_STREAM present) is
    // left to the file pipeline even if it also carries a text caption.
    private fun extractSharedText(intent: Intent?): String? {
        if (intent == null) return null
        if (intent.action != Intent.ACTION_SEND) return null
        // Same URI source as the file pipeline, so a ClipData-only file share
        // is never mistaken for a text share (and vice versa).
        if (sharedUris(intent).isNotEmpty()) return null
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
        return text?.takeIf { it.isNotEmpty() }
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        fileChannel = MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            CHANNEL,
        )
        fileChannel?.setMethodCallHandler { call, result ->
            when (call.method) {
                "pickFiles" -> {
                    if (pendingResult != null) {
                        result.error("ALREADY_PICKING", "A file pick is already in progress", null)
                        return@setMethodCallHandler
                    }
                    pendingResult = result
                    // A cancel from the previous pick must not kill this one.
                    pickCancelled.set(false)
                    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
                        addCategory(Intent.CATEGORY_OPENABLE)
                        type = "*/*"
                        putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
                    }
                    @Suppress("DEPRECATION")
                    startActivityForResult(intent, REQUEST_CODE_PICK_FILES)
                }
                "pickFolder" -> {
                    if (pendingFolderResult != null) {
                        result.error("ALREADY_PICKING", "A folder pick is already in progress", null)
                        return@setMethodCallHandler
                    }
                    pendingFolderResult = result
                    pickCancelled.set(false)
                    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                    @Suppress("DEPRECATION")
                    startActivityForResult(intent, REQUEST_CODE_PICK_FOLDER)
                }
                "saveToDownloads" -> saveToDownloads(call, result)
                "pickSaveFolder" -> {
                    if (pendingSaveFolderResult != null) {
                        result.error("ALREADY_PICKING", "A folder pick is already in progress", null)
                        return@setMethodCallHandler
                    }
                    pendingSaveFolderResult = result
                    val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
                    @Suppress("DEPRECATION")
                    startActivityForResult(intent, REQUEST_CODE_PICK_SAVE_FOLDER)
                }
                "releaseSendSources" -> {
                    releaseSendSources()
                    result.success(null)
                }
                "cancelPick" -> {
                    // Only a flag. The resolve loop owns the unwinding, so this
                    // returns at once rather than making the UI wait on cleanup
                    // it cannot see.
                    pickCancelled.set(true)
                    result.success(null)
                }
                // Both of these are one MediaStore insert plus one
                // openFileDescriptor per file, and both are binder round
                // trips into MediaProvider.  A MethodChannel handler runs on
                // the platform thread by default, so a 1911-file folder spent
                // tens of seconds there: the Accept tap that started it looked
                // like it had done nothing, and the app ANR'd.  lifecycleScope
                // resumes on the main thread, which is where result.success
                // has to be called from.
                //
                // [receiveDestMutex] is what the platform thread used to
                // provide for free: while both ran on the looper they could
                // not interleave, and the code relies on that.  Off it they
                // can, and a release landing between two of a create's inserts
                // clears the list it is still filling — observed on a device
                // as `created 1911` followed by `discarded 348`, which leaves
                // Dart holding descriptor paths whose entries are already
                // closed and deleted.  The lock restores the exclusion the
                // move took away.
                "createReceiveDestinations" -> {
                    val paths = call.argument<List<String>>("paths") ?: emptyList()
                    lifecycleScope.launch {
                        val created = receiveDestMutex.withLock {
                            withContext(Dispatchers.IO) {
                                createReceiveDestinations(paths)
                            }
                        }
                        result.success(created)
                    }
                }
                "finishReceiveDestinations" -> {
                    val publish = call.argument<Boolean>("publish") ?: false
                    lifecycleScope.launch {
                        val published = receiveDestMutex.withLock {
                            releaseReceiveDestinationsConcurrently(publish)
                        }
                        result.success(published)
                    }
                }
                "saveToSafUri" -> saveToSafUri(call, result)
                "openSavedFolder" -> openSavedFolder(call, result)
                "openFileUri" -> openFileUri(call, result)
                else -> result.notImplemented()
            }
        }

        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            KEEPALIVE_CHANNEL,
        ).setMethodCallHandler { call, result -> handleKeepaliveCall(call, result) }

        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            USB_TETHER_CHANNEL,
        ).setMethodCallHandler { call, result ->
            when (call.method) {
                "openTetherSettings" -> openTetherSettings(result)
                "isCableConnected" -> result.success(isUsbCableConnected())
                else -> result.notImplemented()
            }
        }

        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            MULTICAST_CHANNEL,
        ).setMethodCallHandler { call, result ->
            when (call.method) {
                "setHeld" -> {
                    val held = call.argument<Boolean>("held") ?: false
                    result.success(setMulticastLockHeld(held))
                }
                else -> result.notImplemented()
            }
        }

        // Direct phone-to-phone USB (AOA) host/accessory link.
        if (usbAoa == null) usbAoa = UsbAoaChannel(this)
        usbAoa?.configure(flutterEngine.dartExecutor.binaryMessenger)

        shareChannel = MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            SHARE_CHANNEL,
        ).apply {
            setMethodCallHandler { call, result ->
                when (call.method) {
                    "getInitialSharedFiles" -> {
                        val job = initialSharedFilesJob
                        initialSharedFilesJob = null
                        if (job == null) {
                            result.success(null)
                            return@setMethodCallHandler
                        }
                        // job.await() suspends until the IO copy finishes.
                        // lifecycleScope dispatches the resumed continuation
                        // back to the main thread, which is what Flutter
                        // requires for result.success().
                        lifecycleScope.launch {
                            try {
                                result.success(job.await())
                            } catch (_: Exception) {
                                // Activity destroyed mid-resolve (job
                                // cancelled) or it threw — surface as empty
                                // rather than failing the channel call.
                                result.success(emptyShare())
                            }
                        }
                    }
                    "getInitialSharedText" -> {
                        val text = initialSharedText
                        initialSharedText = null
                        result.success(text)
                    }
                    else -> result.notImplemented()
                }
            }
        }
    }

    // Pulls file URIs out of an ACTION_SEND / ACTION_SEND_MULTIPLE intent and
    // resolves each one through the same descriptor-first path the file picker
    // uses.  Returns null when the intent isn't a share intent at all so the
    // caller can distinguish "no share" from "empty share".
    private fun extractSharedFilesFromIntent(intent: Intent?): Map<String, Any?>? {
        if (intent == null) return null
        if (intent.action != Intent.ACTION_SEND &&
            intent.action != Intent.ACTION_SEND_MULTIPLE
        ) return null
        val uris = sharedUris(intent)
        Log.i(SHARE_TAG, "share intent ${intent.action}: ${uris.size} uri(s)")
        // A share has no cancel affordance of its own, so this only fires if a
        // cancel flag outlived the pick that set it. An empty share is the
        // right answer either way: it is what the caller already renders when
        // nothing could be read.
        val resolved = resolveSendSources(uris) ?: return emptyShare()
        if (resolved.sources.size < uris.size) {
            Log.w(
                SHARE_TAG,
                "share intent: ${uris.size - resolved.sources.size} of ${uris.size} " +
                    "uri(s) could not be read",
            )
        }
        return mapOf("sources" to resolved.sources, "rejected" to resolved.rejected)
    }

    // The URIs a share intent carries, preferring ClipData over EXTRA_STREAM.
    //
    // ClipData is the authoritative list: the platform mirrors EXTRA_STREAM
    // into it (Intent.migrateExtraStreamToClipData) precisely because that is
    // what carries the read grants, and a sender may populate only ClipData.
    // Google Photos does exactly that once a selection gets large — sharing 3
    // photos arrived with both, sharing 91 arrived with a 91-item ClipData and
    // no usable EXTRA_STREAM, so an EXTRA_STREAM-only reader silently saw an
    // empty share and dropped the whole batch on the floor.  EXTRA_STREAM
    // remains the fallback for senders that skip ClipData.
    private fun sharedUris(intent: Intent): List<Uri> {
        val clip = intent.clipData
        if (clip != null && clip.itemCount > 0) {
            // Only readable stream URIs.  A shared link arrives as a ClipData
            // item whose URI is the http(s) address itself; treating that as a
            // file would swallow the text share and produce nothing.
            val fromClip = (0 until clip.itemCount)
                .mapNotNull { clip.getItemAt(it).uri }
                .filter {
                    it.scheme == ContentResolver.SCHEME_CONTENT ||
                        it.scheme == ContentResolver.SCHEME_FILE
                }
            if (fromClip.isNotEmpty()) return fromClip
        }
        return when (intent.action) {
            Intent.ACTION_SEND -> {
                val uri = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
                }
                if (uri != null) listOf(uri) else emptyList()
            }
            else -> {
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableArrayListExtra(
                        Intent.EXTRA_STREAM,
                        Uri::class.java,
                    )?.toList().orEmpty()
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
                        ?.toList()
                        .orEmpty()
                }
            }
        }
    }

    // Holds the screen awake for the duration of a transfer.
    //
    // FLAG_KEEP_SCREEN_ON only applies while this activity is visible, which is
    // exactly the case worth covering: a transfer the user is watching, where a
    // screen timeout would otherwise drop Wi-Fi into power-save and cost ~4x
    // throughput. Once Wisp is backgrounded the platform sleeps the screen no
    // matter what an app asks for; from there the foreground service's wake lock
    // is what keeps the transfer itself running.
    private fun applyKeepScreenOn(enabled: Boolean) {
        if (enabled) {
            window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        } else {
            window.clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)
        }
    }

    private fun handleKeepaliveCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "start", "update" -> {
                val title = call.argument<String>("title").orEmpty()
                val body = call.argument<String>("body").orEmpty()
                if (call.method == "start") {
                    ensureNotificationPermission()
                    // Only "start" carries the flag. "update" fires ~1/s for the
                    // whole transfer and must not touch the window.
                    applyKeepScreenOn(
                        call.argument<Boolean>("keepScreenOn") == true,
                    )
                }
                // Progress ticks ("update", ~1/s for the whole transfer) only
                // change the notification text — re-post it directly on the
                // running service.  Routing them through
                // startForegroundService() instead armed a fresh ~5s
                // "must call startForeground()" deadline every second, and a
                // large send (a ~91-photo share) is precisely when the main
                // thread is least likely to meet one: missing it is a hard
                // process kill with
                // ForegroundServiceDidNotStartInTimeException.
                val updated = call.method == "update" &&
                    TransferKeepaliveService.postUpdate(this, title, body)
                if (!updated) {
                    val intent = Intent(this, TransferKeepaliveService::class.java)
                        .putExtra(TransferKeepaliveService.EXTRA_TITLE, title)
                        .putExtra(TransferKeepaliveService.EXTRA_BODY, body)
                    ContextCompat.startForegroundService(this, intent)
                }
                result.success(null)
            }
            "stop" -> {
                applyKeepScreenOn(false)
                stopService(Intent(this, TransferKeepaliveService::class.java))
                result.success(null)
            }
            "requestIgnoreBatteryOptimizations" -> {
                try {
                    val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                        .setData(Uri.parse("package:$packageName"))
                    startActivity(intent)
                    result.success(null)
                } catch (_: ActivityNotFoundException) {
                    try {
                        startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS))
                    } catch (_: ActivityNotFoundException) {
                        // System lacks the settings panel; nothing to surface.
                    }
                    result.success(null)
                }
            }
            "isIgnoringBatteryOptimizations" -> {
                val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
                result.success(pm.isIgnoringBatteryOptimizations(packageName))
            }
            else -> result.notImplemented()
        }
    }

    // Opens the system Tethering settings so the user can flip on USB
    // tethering (Android has no public API to toggle it programmatically).
    // The dedicated TetherSettings screen isn't a documented component, so we
    // fall back through progressively-broader settings panels per device.
    // Raw "is a USB cable physically attached" check, independent of whether
    // tethering is on yet. The platform keeps ACTION_USB_STATE as a sticky
    // broadcast, so a null-receiver register returns the last value without
    // actually subscribing; its `connected` extra is true whenever a cable is
    // plugged. Lets the tether checklist tick "Connect the cable" before the
    // user flips tethering on (which is the only thing that creates the
    // 192.168.42.x interface detect_usb_link keys off).
    private fun isUsbCableConnected(): Boolean {
        return try {
            val sticky = registerReceiver(
                null,
                IntentFilter("android.hardware.usb.action.USB_STATE"),
            )
            sticky?.getBooleanExtra("connected", false) ?: false
        } catch (_: Exception) {
            false
        }
    }

    /// Acquires or releases the Wi-Fi multicast lock. Idempotent: acquiring an
    /// already-held lock, or releasing one that is not held, does nothing.
    /// Returns whether the lock is held afterwards.
    private fun setMulticastLockHeld(held: Boolean): Boolean {
        if (held) {
            if (multicastLock == null) {
                val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
                multicastLock = wm.createMulticastLock(MULTICAST_TAG).apply {
                    setReferenceCounted(false)
                }
            }
            val lock = multicastLock ?: return false
            if (!lock.isHeld) {
                try {
                    lock.acquire()
                } catch (e: SecurityException) {
                    // CHANGE_WIFI_MULTICAST_STATE missing or denied by policy.
                    // LAN discovery degrades to short codes; nothing else breaks.
                    Log.w(OPEN_TAG, "multicast lock acquire failed: ${e.message}")
                    return false
                }
            }
            return lock.isHeld
        }
        multicastLock?.let { if (it.isHeld) it.release() }
        return false
    }

    private fun openTetherSettings(result: MethodChannel.Result) {
        val intents = listOf(
            Intent().setClassName(
                "com.android.settings",
                "com.android.settings.TetherSettings",
            ),
            Intent(Settings.ACTION_WIRELESS_SETTINGS),
            Intent(Settings.ACTION_SETTINGS),
        )
        for (intent in intents) {
            try {
                startActivity(intent)
                result.success(true)
                return
            } catch (_: ActivityNotFoundException) {
                // Try the next, broader fallback.
            } catch (_: SecurityException) {
                // Some OEMs guard the hidden component; fall through.
            }
        }
        result.success(false)
    }

    private fun ensureNotificationPermission() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return
        val granted = ContextCompat.checkSelfPermission(
            this,
            Manifest.permission.POST_NOTIFICATIONS,
        ) == PackageManager.PERMISSION_GRANTED
        if (!granted) {
            ActivityCompat.requestPermissions(
                this,
                arrayOf(Manifest.permission.POST_NOTIFICATIONS),
                REQUEST_CODE_POST_NOTIF,
            )
        }
    }

    @Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
    override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
        if (requestCode == UsbAoaChannel.REQUEST_CODE_VPN_CONSENT) {
            usbAoa?.onVpnConsentResult(resultCode == Activity.RESULT_OK)
            return
        }
        if (requestCode == REQUEST_CODE_PICK_FILES) {
            val result = pendingResult
            pendingResult = null
            if (result == null) {
                super.onActivityResult(requestCode, resultCode, data)
                return
            }
            if (resultCode != Activity.RESULT_OK || data == null) {
                result.success(
                    mapOf(
                        "paths" to emptyList<String>(),
                        "bytesCopied" to 0L,
                        "copyElapsedMicros" to 0L,
                    ),
                )
                return
            }
            val uris = mutableListOf<Uri>()
            val clipData = data.clipData
            if (clipData != null) {
                for (i in 0 until clipData.itemCount) {
                    uris.add(clipData.getItemAt(i).uri)
                }
            } else {
                data.data?.let { uris.add(it) }
            }
            // Copy on Dispatchers.IO so a multi-GB pick never blocks the main
            // Runs on Dispatchers.IO because the fallback copy still can, so a
            // pick that lands there never blocks the main thread (which froze
            // the UI / triggered an ANR).  Copy progress is streamed back via
            // "onPickProgress"; a pick that copies nothing — the common case
            // now — finishes without emitting any.
            lifecycleScope.launch {
                val pickResult = withContext(Dispatchers.IO) {
                    val startedNanos = SystemClock.elapsedRealtimeNanos()
                    var lastEmit = 0L
                    val resolved = resolveSendSources(uris) { copied, copyTotal, index ->
                        if (copied - lastEmit >= PROGRESS_EMIT_BYTES) {
                            lastEmit = copied
                            emitPickProgress(copied, copyTotal, index, uris.size)
                        }
                    } ?: return@withContext cancelledPick()
                    val bytesCopied = resolved.sources
                        .filter { it["copied"] == true }
                        .sumOf { it["size"] as Long }
                    if (bytesCopied > 0L) {
                        emitPickProgress(bytesCopied, bytesCopied, uris.size, uris.size)
                    }
                    mapOf(
                        "sources" to resolved.sources,
                        "rejected" to resolved.rejected,
                        "bytesCopied" to bytesCopied,
                        "copyElapsedMicros" to
                            (SystemClock.elapsedRealtimeNanos() - startedNanos) / 1_000L,
                    )
                }
                result.success(pickResult)
            }
            return
        }
        if (requestCode == REQUEST_CODE_PICK_FOLDER) {
            val result = pendingFolderResult
            pendingFolderResult = null
            if (result == null) {
                super.onActivityResult(requestCode, resultCode, data)
                return
            }
            val treeUri = data?.data
            if (resultCode != Activity.RESULT_OK || treeUri == null) {
                result.success(null)
                return
            }
            contentResolver.takePersistableUriPermission(
                treeUri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION,
            )
            val rootDoc = FastDocumentFile.fromTreeUri(this, treeUri)
            // Off the main thread, same as the file pick above.  Only the files
            // that could not be sent from a descriptor are copied, and how many
            // that is isn't known until the tree has been walked, so progress
            // stays indeterminate (totalBytes = 0).
            lifecycleScope.launch {
                val res = withContext(Dispatchers.IO) {
                    val startedNanos = SystemClock.elapsedRealtimeNanos()
                    var lastEmit = 0L
                    var lastFileEmit = -PROGRESS_EMIT_FILES
                    val tree = resolveTreeSources(
                        rootDoc,
                        onCopyProgress = { copied ->
                            if (copied - lastEmit >= PROGRESS_EMIT_BYTES) {
                                lastEmit = copied
                                emitPickProgress(copied, 0L, 0, 1)
                            }
                        },
                        onFileProgress = { resolved, total ->
                            // Throttled by file count for the same reason the
                            // copy is throttled by bytes: 1911 channel hops in
                            // 27 s would cost more than the work they describe.
                            // Every 16 gives ~4 updates a second, which a
                            // progress bar cannot use more of anyway.
                            if (resolved - lastFileEmit >= PROGRESS_EMIT_FILES ||
                                resolved == total
                            ) {
                                lastFileEmit = resolved
                                emitPickProgress(0L, 0L, resolved, total)
                            }
                        },
                    ) ?: return@withContext cancelledPick()
                    mapOf(
                        // A tree has no filesystem path, so the URI is what the
                        // draft keys this item by.
                        "identity" to treeUri.toString(),
                        "name" to tree.name,
                        "sources" to tree.sources,
                        "rejected" to tree.rejected,
                        "sizeBytes" to tree.totalBytes,
                        "bytesCopied" to tree.copiedBytes,
                        "copyElapsedMicros" to
                            (SystemClock.elapsedRealtimeNanos() - startedNanos) / 1_000L,
                    )
                }
                result.success(res)
            }
            return
        }
        if (requestCode == REQUEST_CODE_PICK_SAVE_FOLDER) {
            val result = pendingSaveFolderResult
            pendingSaveFolderResult = null
            if (result == null) {
                super.onActivityResult(requestCode, resultCode, data)
                return
            }
            val treeUri = data?.data
            if (resultCode != Activity.RESULT_OK || treeUri == null) {
                result.success(null)
                return
            }
            // Persist both read and write permissions so the app can save
            // files to this folder across sessions without re-prompting.
            contentResolver.takePersistableUriPermission(
                treeUri,
                Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
            )
            val docFile = DocumentFile.fromTreeUri(this, treeUri)
            val displayName = docFile?.name ?: treeUri.lastPathSegment ?: "Selected folder"
            result.success(mapOf("uri" to treeUri.toString(), "displayName" to displayName))
            return
        }
        super.onActivityResult(requestCode, resultCode, data)
    }

    // Resolves picked or shared URIs into sources the core can open, handing
    // back a live descriptor wherever the platform allows one and falling
    // back to the old cache copy only where it does not.  That is the whole
    // point of the exercise: a 6 GB pick used to need 6 GB of free space
    // before the transfer could even start.
    //
    // Descriptors are handed out largest-first, because the budget is what
    // limits how many sources can skip the copy and the big ones are the ones
    // that hurt.
    //
    // Each returned map carries `path` (what Rust opens), `name` (what the
    // receiver sees — a descriptor path ends in the fd number, so the real
    // name has to travel separately), `size` and `copied`.
    /// `null` when the user cancelled part-way; see [pickCancelled].
    private fun resolveSendSources(
        uris: List<Uri>,
        onCopyProgress: (copiedTotal: Long, copyTotal: Long, index: Int) -> Unit = { _, _, _ -> },
    ): SendSources? {
        val names = uris.map { sanitizeFileName(resolveFileName(it)) }
        val sizes = uris.map { resolveSize(it) ?: 0L }
        val resolved = arrayOfNulls<Map<String, Any?>>(uris.size)
        val rejected = mutableListOf<Map<String, Any?>>()

        for (index in uris.indices.sortedByDescending { sizes[it] }) {
            if (pickCancelled.get()) return null
            val fdPath = openForSend(uris[index]) ?: continue
            resolved[index] = mapOf(
                "path" to fdPath,
                "name" to names[index],
                // Not every provider fills in OpenableColumns.SIZE.  Fall back
                // to the descriptor rather than to `File(fdPath).length()`:
                // statting the path is the same walk that opening it fails.
                "size" to (sizes[index].takeIf { it > 0L } ?: descriptorLength(fdPath)),
                "copied" to false,
            )
        }

        val copyTotal = uris.indices.filter { resolved[it] == null }.sumOf { sizes[it] }
        var copiedBefore = 0L
        for (index in uris.indices) {
            if (pickCancelled.get()) return null
            if (resolved[index] != null) continue
            // Refuse up front what the disk cannot hold, rather than writing
            // gigabytes and discovering it at the far end.  A provider that
            // never reports a size falls through to the running check inside
            // the copy itself.
            val needed = sizes[index]
            val spare = copyBudgetBytes()
            if (needed > spare) {
                Log.w(
                    PICK_TAG,
                    "no room to copy ${names[index]}: needs $needed B, $spare B spare",
                )
                rejected.add(rejection(names[index], REJECT_NO_SPACE, needed))
                copiedBefore += needed
                continue
            }
            val path = copyUriToCache(uris[index]) { fileCopied ->
                onCopyProgress(copiedBefore + fileCopied, copyTotal, index)
            }
            copiedBefore += needed
            if (path == null) {
                val reason = if (copyBudgetBytes() <= 0L) REJECT_NO_SPACE else REJECT_UNREADABLE
                rejected.add(rejection(names[index], reason, needed))
                continue
            }
            resolved[index] = mapOf(
                "path" to path,
                "name" to names[index],
                "size" to File(path).length(),
                "copied" to true,
            )
        }

        val fdCount = resolved.count { it != null && it["copied"] == false }
        Log.i(
            PICK_TAG,
            "resolved ${resolved.count { it != null }}/${uris.size} source(s), " +
                "$fdCount without a copy",
        )
        return SendSources(resolved.filterNotNull(), rejected)
    }

    private data class SendSources(
        val sources: List<Map<String, Any?>>,
        val rejected: List<Map<String, Any?>>,
    )

    // Bytes the fallback copy may still write before it would put the device
    // into low storage.  Zero once the cache filesystem is down to the
    // headroom we refuse to spend.
    private fun copyBudgetBytes(): Long =
        (cacheDir.usableSpace - COPY_HEADROOM_BYTES).coerceAtLeast(0L)

    // One source the platform left us no way to prepare, in the shape the
    // Dart side turns into a message.
    private fun rejection(
        name: String,
        reason: String,
        requiredBytes: Long,
    ): Map<String, Any?> = mapOf(
        "name" to name,
        "reason" to reason,
        "requiredBytes" to requiredBytes,
        "availableBytes" to copyBudgetBytes(),
    )

    // Resolves a picked SAF tree into sources.  A tree offers no single handle
    // to open, so a folder travels as one descriptor per file, each carrying
    // its path within the folder for the receiver to rebuild the tree from.
    //
    // Whatever does not fit the descriptor budget (or cannot be opened) is
    // copied into one cache directory that mirrors the folder, and that
    // directory is handed over as a single ordinary source — the core walks it
    // and derives the same relative paths.  So the two kinds mix within one
    // folder without either needing to know about the other.
    // What a cancelled pick hands back.
    //
    // Null, which is the same shape a dismissed system picker produces, so no
    // caller needs a new case for it. The descriptors opened before the cancel
    // are closed here: nothing partly prepared may reach Dart, or the draft
    // would hold paths to files the core can no longer read.
    private fun cancelledPick(): Map<String, Any?>? {
        Log.i(PICK_TAG, "pick cancelled, releasing what was opened")
        releaseSendSources()
        return null
    }

    /// `null` when the user cancelled part-way; see [pickCancelled].
    private fun resolveTreeSources(
        root: FastDocumentFile,
        onCopyProgress: (copiedTotal: Long) -> Unit = {},
        onFileProgress: (resolved: Int, total: Int) -> Unit = { _, _ -> },
    ): TreeSources? {
        val rootName = sanitizeFileName(root.name.ifBlank { "folder" })
        val files = mutableListOf<TreeFile>()
        collectTreeFiles(root, rootName, 1, files)

        val sources = mutableListOf<Map<String, Any?>>()
        val claimed = BooleanArray(files.size)
        var opened = 0
        for (index in files.indices.sortedByDescending { files[it].doc.size }) {
            val file = files[index]
            // Reported per file, because this loop is the one the user waits
            // through now. Opening a descriptor is a binder round trip, ~14 ms
            // each, so a 1911-file folder sat here for 27-37 s. The only
            // progress signal was the fallback copy's byte count — and the
            // descriptor budget fix removed the copy, so the folder that most
            // needed a progress bar was the one that stopped emitting any.
            if (pickCancelled.get()) return null
            onFileProgress(opened, files.size)
            val fdPath = openForSend(file.doc.uri) ?: continue
            claimed[index] = true
            opened += 1
            sources.add(
                mapOf(
                    "path" to fdPath,
                    "name" to file.transferPath,
                    "size" to (file.doc.size.takeIf { it > 0L } ?: descriptorLength(fdPath)),
                    "copied" to false,
                ),
            )
        }
        onFileProgress(opened, files.size)
        val leftovers = files.filterIndexed { index, _ -> !claimed[index] }
        val rejected = mutableListOf<Map<String, Any?>>()

        var copiedBytes = 0L
        var mirrored = 0
        if (leftovers.isNotEmpty()) {
            // The mirror is rooted at the folder name so the core's walk
            // derives exactly the transfer paths the descriptors carry.
            val mirrorRoot = File(newPickedDir(), rootName)
            for (file in leftovers) {
                if (pickCancelled.get()) {
                    // Half-written copies are this loop's own mess to clear.
                    mirrorRoot.deleteRecursively()
                    return null
                }
                // Drop the folder name — it is already `mirrorRoot`.
                val relative = file.transferPath.substringAfter('/', "")
                if (relative.isEmpty()) continue
                val needed = file.doc.size
                val spare = copyBudgetBytes()
                if (needed > spare) {
                    Log.w(
                        PICK_TAG,
                        "no room to copy ${file.transferPath}: needs $needed B, $spare B spare",
                    )
                    rejected.add(rejection(file.transferPath, REJECT_NO_SPACE, needed))
                    continue
                }
                val dest = File(mirrorRoot, relative)
                dest.parentFile?.mkdirs()
                val before = copiedBytes
                copiedBytes += try {
                    val written = streamUriToFile(file.doc.uri, dest) { copied ->
                        onCopyProgress(before + copied)
                    }
                    mirrored += 1
                    written
                } catch (e: Exception) {
                    Log.w(PICK_TAG, "could not copy ${file.transferPath}: ${e.message}")
                    dest.delete()
                    val reason =
                        if (copyBudgetBytes() <= 0L) REJECT_NO_SPACE else REJECT_UNREADABLE
                    rejected.add(rejection(file.transferPath, reason, needed))
                    0L
                }
            }
            // Only when something actually landed there: an empty mirror is
            // not a folder the receiver should be offered.
            if (mirrored > 0) {
                sources.add(
                    mapOf(
                        "path" to mirrorRoot.absolutePath,
                        "name" to rootName,
                        "size" to copiedBytes,
                        "copied" to true,
                    ),
                )
            }
        }

        Log.i(
            PICK_TAG,
            "folder $rootName: ${files.size} file(s), " +
                "${files.size - leftovers.size} without a copy",
        )
        return TreeSources(
            name = rootName,
            sources = sources,
            // Summed from the resolved sources, not from the tree listing:
            // providers do not always fill in COLUMN_SIZE, and a descriptor
            // source has already had its size measured through the fd.
            totalBytes = sources.sumOf { it["size"] as Long },
            copiedBytes = copiedBytes,
            rejected = rejected,
        )
    }

    private data class TreeFile(val transferPath: String, val doc: FastDocumentFile)

    private data class TreeSources(
        val name: String,
        val sources: List<Map<String, Any?>>,
        val totalBytes: Long,
        val copiedBytes: Long,
        val rejected: List<Map<String, Any?>>,
    )

    private fun collectTreeFiles(
        dir: FastDocumentFile,
        prefix: String,
        depth: Int,
        out: MutableList<TreeFile>,
    ) {
        if (depth > MAX_TREE_DEPTH) {
            Log.w(PICK_TAG, "folder tree deeper than $MAX_TREE_DEPTH, stopping at $prefix")
            return
        }
        for (child in dir.listFiles()) {
            if (child.name.isBlank()) continue
            // `.pending-<epoch>-<name>` is the on-disk name MediaStore gives a
            // row while IS_PENDING is set — never a file the user put there.
            //
            // They show up because clearing IS_PENDING returns *before*
            // MediaProvider renames the file: after a 1911-file receive, 770 of
            // them still carried pending names 21 s after our publish loop had
            // finished and reported success on every one. Picking that folder
            // in the meantime walked names whose rows were no longer pending,
            // so every open failed — "can't read 770 files" on a folder Wisp
            // had itself just received.
            if (child.name.startsWith(PENDING_MEDIA_PREFIX)) {
                continue
            }
            val childPath = "$prefix/${sanitizeFileName(child.name)}"
            if (child.isDirectory) {
                collectTreeFiles(child, childPath, depth + 1, out)
            } else if (child.isFile) {
                out.add(TreeFile(childPath, child))
            }
        }
    }

    // Streams one content URI into [dest] and returns the bytes written,
    // stopping before the copy would fill the disk.  Throws on any failure —
    // including that one — so the caller can delete the partial file.
    private fun streamUriToFile(uri: Uri, dest: File, onBytes: (Long) -> Unit): Long {
        val input = contentResolver.openInputStream(uri)
            ?: throw IOException("provider returned no stream")
        var written = 0L
        input.use {
            FileOutputStream(dest).use { output ->
                val buffer = ByteArray(65_536)
                var nextCheck = 0L
                while (true) {
                    // Re-measured as we go: the pre-flight check only knows
                    // the sizes providers chose to report, and other apps are
                    // spending the same disk in the meantime.
                    if (written >= nextCheck) {
                        if (copyBudgetBytes() <= 0L) {
                            throw IOException("not enough free space")
                        }
                        nextCheck = written + SPACE_RECHECK_BYTES
                    }
                    val read = it.read(buffer)
                    if (read < 0) break
                    output.write(buffer, 0, read)
                    written += read
                    onBytes(written)
                }
            }
        }
        return written
    }

    // Opens [uri] for a copy-free send and returns the `/proc/self/fd/<n>`
    // path the core should use, or null when this source has to be copied.
    //
    // The descriptor must point at a plain regular file: cloud providers
    // (Drive, Dropbox) answer `openFileDescriptor` with a pipe, which has no
    // length and cannot be read at an offset, so those still need a copy.
    //
    // What is deliberately *not* required is that the path reopen.  It mostly
    // does not: opening `/proc/self/fd/<n>` is a fresh path walk that lands in
    // MediaProvider's FUSE daemon, which re-checks permission against our uid,
    // and the grant we hold covers the descriptor rather than the name.  On
    // Android 17 that refused every provider tried, which used to send every
    // external-storage file down the copy path.  The core reads the descriptor
    // by duplicating it instead of reopening the path — see the core's
    // `blobs::descriptor` — so the refusal no longer matters.
    private fun openForSend(uri: Uri): String? {
        // A soft budget: two concurrent resolves can overshoot it by the
        // handful they have in flight, which is well inside the headroom.
        if (synchronized(sendFdLock) { openSendFds.size } >= openFdBudget) return null
        val pfd = try {
            contentResolver.openFileDescriptor(uri, "r")
        } catch (e: Exception) {
            Log.i(PICK_TAG, "cannot open $uri as a descriptor: ${e.message}")
            null
        } ?: return null
        val regular = try {
            OsConstants.S_ISREG(Os.fstat(pfd.fileDescriptor).st_mode)
        } catch (e: Exception) {
            Log.i(PICK_TAG, "cannot stat descriptor for $uri, copying instead: ${e.message}")
            false
        }
        if (!regular) {
            try {
                pfd.close()
            } catch (_: IOException) {
            }
            return null
        }
        synchronized(sendFdLock) { openSendFds.add(pfd) }
        return "/proc/self/fd/${pfd.fd}"
    }

    // Size of the file behind a `/proc/self/fd/<n>` path, measured through the
    // descriptor.  `File(path).length()` would stat the path, which is the walk
    // the platform refuses for a granted file, and would quietly report 0.
    private fun descriptorLength(fdPath: String): Long {
        val fd = fdPath.substringAfterLast('/').toIntOrNull() ?: return 0L
        val pfd = synchronized(sendFdLock) { openSendFds.lastOrNull { it.fd == fd } } ?: return 0L
        return try {
            Os.fstat(pfd.fileDescriptor).st_size
        } catch (e: Exception) {
            Log.w(PICK_TAG, "cannot measure descriptor $fdPath: ${e.message}")
            0L
        }
    }

    // Releases every descriptor held for a copy-free send.  Dart calls this
    // when it clears the draft, alongside deleting the `wisp_picked` cache.
    private fun releaseSendSources() {
        val held = synchronized(sendFdLock) {
            if (openSendFds.isEmpty()) return
            openSendFds.toList().also { openSendFds.clear() }
        }
        Log.i(PICK_TAG, "releasing ${held.size} send descriptor(s)")
        for (pfd in held) {
            try {
                pfd.close()
            } catch (_: IOException) {
            }
        }
    }

    // One incoming file's destination, created before the transfer starts and
    // held open while it runs.
    //
    // MediaStore keeps a `IS_PENDING` entry invisible to other apps, which is
    // exactly the guarantee a partially written file needs — so the receiver
    // can write straight into the user's Downloads rather than filling the app
    // cache and copying afterwards, which is what made a receive cost two
    // copies of the transfer.
    private data class ReceiveDestination(
        val transferPath: String,
        val uri: Uri,
        val pfd: ParcelFileDescriptor,
    )

    // Guards one mutation of the list below.
    private val receiveDestLock = Any()

    // Guards a whole create-or-release *operation*, which is a different
    // question: each is a loop of thousands of binder calls, and each assumes
    // the other is not running.  Held across the suspension in the channel
    // handlers, so `createReceiveDestinations`'s own internal release calls
    // (its first line, and `abortPartialDestinations`) must stay unlocked —
    // they already run inside it.
    //
    // `onDestroy` also releases without taking it, deliberately: it runs on
    // the main thread, and blocking there for a create that is 34 s into 1911
    // MediaStore inserts would re-create the ANR this whole change removed.
    // At teardown nothing will write through those descriptors anyway.
    private val receiveDestMutex = Mutex()
    private val openReceiveDestinations = mutableListOf<ReceiveDestination>()

    // Creates a pending MediaStore entry per incoming file and returns the
    // descriptor path the core should write to.
    //
    // Returns an empty list if anything goes wrong, which the Dart side reads
    // as "resolve destinations yourself" — the receiver then writes into the
    // app cache and the files are copied over once the transfer completes, as
    // they always were.  Only the default Downloads/Wisp target is handled
    // here: a user-chosen SAF folder has no equivalent of `IS_PENDING`, so a
    // partial file would be visible there, and it keeps the copy path.
    private suspend fun createReceiveDestinations(paths: List<String>): List<Map<String, Any?>> {
        // Concurrently, like everything else on this path: a retry after a
        // failed transfer arrives here holding the previous attempt's 1911
        // destinations, and discarding those serially would put the cost right
        // back on the critical path it was just taken off.
        releaseReceiveDestinationsConcurrently(publish = false)
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return emptyList()
        // Every destination is a descriptor held for the whole transfer, and
        // this loop had no ceiling at all — a 1911-file folder asked for 1911
        // of them at once, against a table the rest of the app shares.  Past
        // the budget the whole transfer takes the cache route instead: two
        // writes per byte, but nothing else in the app loses a descriptor.
        if (paths.size > openFdBudget) {
            Log.i(
                RECEIVE_TAG,
                "${paths.size} file(s) exceeds the $openFdBudget descriptor " +
                    "budget; receiving into the cache and copying afterwards",
            )
            return emptyList()
        }
        // Concurrent because this sits on the critical path of the *sender's*
        // decision timeout, and serially it did not fit inside it.  Measured
        // on a 1911-file folder: 127.4 s of inserts against the sender's 120 s
        // budget for a decision, which it abandoned 7 s before the receiver
        // was ready — the user saw "unable to send" and had tapped Accept
        // within 4 seconds of the offer appearing.  All of that was ours.
        val started = SystemClock.elapsedRealtime()
        val created = arrayOfNulls<Map<String, Any?>>(paths.size)
        val slots = Semaphore(DESTINATION_CONCURRENCY)
        val giveUp = AtomicBoolean(false)
        try {
            coroutineScope {
                paths.forEachIndexed { index, path ->
                    launch(Dispatchers.IO) {
                        slots.withPermit {
                            // One failure loses the whole batch — the caller's
                            // contract is all-or-nothing — so there is no
                            // point opening another 1900 descriptors for a
                            // batch already lost.
                            if (giveUp.get()) return@withPermit
                            val entry = createOneDestination(path)
                            if (entry == null) giveUp.set(true) else created[index] = entry
                        }
                    }
                }
            }
        } catch (e: Exception) {
            Log.w(RECEIVE_TAG, "could not pre-create destinations: ${e.message}")
            return abortPartialDestinations()
        }
        if (giveUp.get() || created.any { it == null }) return abortPartialDestinations()
        // Elapsed, because the number that matters is whether this now fits
        // inside the sender's budget, and it depends on how many files already
        // sit in Download/Wisp: a colliding display name makes MediaProvider
        // uniquify it, which took the per-file cost from 18 ms to 66.7 ms.
        Log.i(
            RECEIVE_TAG,
            "created ${paths.size} pending destination(s) in " +
                "${SystemClock.elapsedRealtime() - started} ms",
        )
        return created.map { it!! }
    }

    // One pending entry, or null when it could not be created.
    //
    // The descriptor lands in [openReceiveDestinations] before this returns,
    // so an abort or a publish covers it even when the batch around it fails.
    private fun createOneDestination(path: String): Map<String, Any?>? {
        val parts = path.replace('\\', '/').split('/').filter { it.isNotBlank() }
        if (parts.isEmpty()) return null
        val fileName = sanitizeFileName(parts.last())
        val subDir = parts.dropLast(1).joinToString("/")
        val values = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, fileName)
            put(MediaStore.Downloads.MIME_TYPE, "application/octet-stream")
            put(
                MediaStore.Downloads.RELATIVE_PATH,
                "Download/Wisp${if (subDir.isNotEmpty()) "/$subDir" else ""}",
            )
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val uri = contentResolver.insert(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI, values,
        ) ?: return null
        // "rw" rather than "w": the core seeks when it resumes, and it reads
        // the current length to know where to resume from.
        val pfd = contentResolver.openFileDescriptor(uri, "rw")
        if (pfd == null) {
            contentResolver.delete(uri, null, null)
            return null
        }
        synchronized(receiveDestLock) {
            openReceiveDestinations.add(ReceiveDestination(path, uri, pfd))
        }
        return mapOf("transferPath" to path, "fdPath" to "/proc/self/fd/${pfd.fd}")
    }

    private suspend fun abortPartialDestinations(): List<Map<String, Any?>> {
        releaseReceiveDestinationsConcurrently(publish = false)
        return emptyList()
    }

    // Closes every held descriptor.  On success the pending flag is cleared and
    // the files become visible; otherwise the entries are deleted, so a failed
    // transfer leaves nothing behind.  Returns transfer path -> final URI for
    // the published files.
    private fun releaseReceiveDestinations(publish: Boolean): Map<String, String> {
        val held = takeHeldDestinations() ?: return emptyMap()
        val started = SystemClock.elapsedRealtime()
        val published = mutableMapOf<String, String>()
        for (dest in held) {
            releaseOneDestination(dest, publish)?.let { published[dest.transferPath] = it }
        }
        logReleased(publish, held.size, started)
        return published
    }

    // The same work, spread across [DESTINATION_CONCURRENCY] threads.
    //
    // Publishing is the mirror of creating and cost the same 34 s on a
    // 1911-file transfer, except it lands *after* the last byte: the files stay
    // invisible and the receive looks stuck long after it finished. Nothing is
    // waiting on a timeout here, which is why the serial version above is
    // still what `onDestroy` calls — it runs on the main thread, where it
    // cannot suspend.
    private suspend fun releaseReceiveDestinationsConcurrently(
        publish: Boolean,
    ): Map<String, String> {
        val held = takeHeldDestinations() ?: return emptyMap()
        val started = SystemClock.elapsedRealtime()
        val published = java.util.concurrent.ConcurrentHashMap<String, String>()
        val slots = Semaphore(DESTINATION_CONCURRENCY)
        coroutineScope {
            for (dest in held) {
                launch(Dispatchers.IO) {
                    slots.withPermit {
                        releaseOneDestination(dest, publish)?.let {
                            published[dest.transferPath] = it
                        }
                    }
                }
            }
        }
        logReleased(publish, held.size, started)
        return published
    }

    // Claims the whole held set, or null when there is nothing to release.
    private fun takeHeldDestinations(): List<ReceiveDestination>? =
        synchronized(receiveDestLock) {
            if (openReceiveDestinations.isEmpty()) {
                null
            } else {
                openReceiveDestinations.toList().also { openReceiveDestinations.clear() }
            }
        }

    // Closes one descriptor and either clears its pending flag or deletes the
    // entry. Returns the final URI when it was published.
    private fun releaseOneDestination(dest: ReceiveDestination, publish: Boolean): String? {
        try {
            dest.pfd.close()
        } catch (_: IOException) {
        }
        return try {
            if (publish) {
                contentResolver.update(
                    dest.uri,
                    ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) },
                    null, null,
                )
                dest.uri.toString()
            } else {
                contentResolver.delete(dest.uri, null, null)
                null
            }
        } catch (e: Exception) {
            Log.w(
                RECEIVE_TAG,
                "could not ${if (publish) "publish" else "discard"} ${dest.uri}: ${e.message}",
            )
            null
        }
    }

    private fun logReleased(publish: Boolean, count: Int, startedAt: Long) {
        Log.i(
            RECEIVE_TAG,
            "${if (publish) "published" else "discarded"} $count destination(s) in " +
                "${SystemClock.elapsedRealtime() - startedAt} ms",
        )
    }

    // A fresh, never-reused directory under the shared `wisp_picked` cache
    // root (which Dart clears wholesale via clearPickedCache).
    private fun newPickedDir(): File = File(
        File(cacheDir, "wisp_picked"),
        "${System.currentTimeMillis()}-${pickedCopySeq.getAndIncrement()}",
    )

    // Streams a content URI to the app cache directory to avoid encoding
    // large files as bytes through the Flutter platform channel.  [onBytes]
    // is invoked with the running byte count for this file after every chunk
    // so the caller can report copy progress.
    private fun copyUriToCache(uri: Uri, onBytes: (Long) -> Unit = {}): String? {
        val dir = newPickedDir()
        dir.mkdirs()
        val cacheFile = File(dir, sanitizeFileName(resolveFileName(uri)))
        return try {
            streamUriToFile(uri, cacheFile, onBytes)
            cacheFile.absolutePath
        } catch (e: Exception) {
            // Leave nothing behind.  A half-written copy is useless to the
            // send, and the copy that ran out of room is precisely the one
            // whose leftovers keep the device full.
            Log.w(PICK_TAG, "could not copy $uri: ${e.message}")
            cacheFile.delete()
            dir.delete()
            null
        }
    }

    // Posts a copy-progress event to Flutter on the main thread.  A
    // [totalBytes] of 0 means "unknown" (folder picks), so the Dart side
    // renders an indeterminate bar.
    private fun emitPickProgress(bytesCopied: Long, totalBytes: Long, index: Int, count: Int) {
        runOnUiThread {
            fileChannel?.invokeMethod(
                "onPickProgress",
                mapOf(
                    "bytesCopied" to bytesCopied,
                    "totalBytes" to totalBytes,
                    "index" to index,
                    "count" to count,
                ),
            )
        }
    }

    // Total size of a content URI via OpenableColumns.SIZE, or null when the
    // provider doesn't report it (progress then treats the file as 0 B).
    private fun resolveSize(uri: Uri): Long? {
        return try {
            contentResolver.query(
                uri,
                arrayOf(OpenableColumns.SIZE),
                null, null, null,
            )?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val idx = cursor.getColumnIndex(OpenableColumns.SIZE)
                    if (idx >= 0 && !cursor.isNull(idx)) cursor.getLong(idx) else null
                } else null
            }
        } catch (_: Exception) {
            null
        }
    }

    // Providers occasionally hand back a display name carrying path
    // separators (or nothing at all).  Flatten it so the copy always lands
    // directly inside the per-item directory created above rather than
    // failing on a missing parent.
    private fun sanitizeFileName(name: String?): String {
        val flattened = name
            ?.replace('\\', '_')
            ?.replace('/', '_')
            ?.trim()
            ?.trimStart('.')
        return if (flattened.isNullOrBlank()) "picked_file" else flattened
    }

    private fun resolveFileName(uri: Uri): String? {
        // content:// URIs from DocumentsUI / DocumentProviders expose the
        // friendly name via OpenableColumns.DISPLAY_NAME.
        val displayName = try {
            contentResolver.query(
                uri,
                arrayOf(OpenableColumns.DISPLAY_NAME),
                null, null, null,
            )?.use { cursor ->
                if (cursor.moveToFirst()) {
                    val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
                    if (idx >= 0) cursor.getString(idx) else null
                } else null
            }
        } catch (_: Exception) {
            null
        }
        if (!displayName.isNullOrBlank()) return displayName
        // file:// URIs (older Files apps, some legacy share targets) carry
        // the name as the last path segment.
        return uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() }
    }

    // Saves a file from [srcPath] into the public Downloads/Wisp/ folder.
    // [relativeFilePath] is the path relative to the transfer root (e.g. "photos/cat.jpg").
    // On API 29+: uses MediaStore.Downloads so no extra permission is needed.
    // On API < 29:  writes to app-specific external downloads (no permission needed).
    private fun saveToDownloads(call: MethodCall, result: MethodChannel.Result) {
        val srcPath = call.argument<String>("srcPath")
            ?: return result.error("INVALID", "srcPath required", null)
        val relativeFilePath = call.argument<String>("relativeFilePath")
            ?: return result.error("INVALID", "relativeFilePath required", null)
        val mimeType = call.argument<String>("mimeType") ?: "application/octet-stream"

        // Copies the file's bytes.  The receiver calls this once per file when
        // it wrote into its cache instead of straight to the destination, so a
        // 1911-file folder is 1911 whole-file copies — never on the platform
        // thread.
        lifecycleScope.launch {
            try {
                val savedPath = withContext(Dispatchers.IO) {
                    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                        saveToDownloadsQ(srcPath, relativeFilePath, mimeType)
                    } else {
                        saveToDownloadsLegacy(srcPath, relativeFilePath)
                    }
                }
                result.success(savedPath)
            } catch (e: Exception) {
                result.error("SAVE_FAILED", e.message, null)
            }
        }
    }

    // MediaStore.Downloads path (Android 10+).
    private fun saveToDownloadsQ(srcPath: String, relativeFilePath: String, mimeType: String): String {
        val parts = relativeFilePath.replace('\\', '/').split('/')
        val fileName = parts.last()
        val subDir = if (parts.size > 1) parts.dropLast(1).joinToString("/") else ""
        val relativePath = "Download/Wisp${if (subDir.isNotEmpty()) "/$subDir" else ""}"

        val contentValues = ContentValues().apply {
            put(MediaStore.Downloads.DISPLAY_NAME, fileName)
            put(MediaStore.Downloads.MIME_TYPE, mimeType)
            put(MediaStore.Downloads.RELATIVE_PATH, relativePath)
            put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val uri = contentResolver.insert(
            MediaStore.Downloads.EXTERNAL_CONTENT_URI, contentValues,
        ) ?: throw java.io.IOException("Could not create MediaStore entry for $fileName")

        try {
            contentResolver.openOutputStream(uri)?.use { output ->
                File(srcPath).inputStream().use { input ->
                    input.copyTo(output, bufferSize = 65_536)
                }
            }
            contentResolver.update(
                uri,
                ContentValues().apply { put(MediaStore.Downloads.IS_PENDING, 0) },
                null, null,
            )
        } catch (e: Exception) {
            contentResolver.delete(uri, null, null)
            throw e
        }
        return uri.toString()
    }

    // Fallback for Android 9 and below: write to app-specific external downloads.
    // No storage permission required for the app's own external directory.
    private fun saveToDownloadsLegacy(srcPath: String, relativeFilePath: String): String {
        val parts = relativeFilePath.replace('\\', '/').split('/')
        val fileName = parts.last()
        val subDir = if (parts.size > 1) parts.dropLast(1).joinToString(File.separator) else ""
        val baseDir = getExternalFilesDir(Environment.DIRECTORY_DOWNLOADS)
            ?: cacheDir  // last resort fallback
        val destDir = if (subDir.isNotEmpty()) File(File(baseDir, "Wisp"), subDir) else File(baseDir, "Wisp")
        destDir.mkdirs()
        val destFile = File(destDir, fileName)
        File(srcPath).copyTo(destFile, overwrite = true)
        return destFile.absolutePath
    }

    // Saves [srcPath] into a user-chosen folder identified by a SAF tree URI.
    // Intermediate sub-directories from [relativeFilePath] are created as needed.
    // Returns the final DocumentFile URI string on success.
    private fun saveToSafUri(call: MethodCall, result: MethodChannel.Result) {
        val srcPath = call.argument<String>("srcPath")
            ?: return result.error("INVALID", "srcPath required", null)
        val relativeFilePath = call.argument<String>("relativeFilePath")
            ?: return result.error("INVALID", "relativeFilePath required", null)
        val treeUriStr = call.argument<String>("treeUri")
            ?: return result.error("INVALID", "treeUri required", null)

        // Same reason as saveToDownloads: one call per received file, each a
        // whole-file copy plus several SAF queries, so it cannot run on the
        // platform thread.
        //
        // One cost deliberately left in place: DocumentFile.findFile
        // enumerates the directory, so saving N files into one folder is
        // O(N^2) queries.  Off the main thread that is slow rather than fatal,
        // and only the user-chosen-folder path pays it — the default Downloads
        // target writes straight into pending MediaStore entries and never
        // reaches here.
        lifecycleScope.launch {
            try {
                val savedUri = withContext(Dispatchers.IO) {
                    val treeUri = Uri.parse(treeUriStr)
                    var dir = DocumentFile.fromTreeUri(this@MainActivity, treeUri)
                        ?: throw IOException("Cannot open folder URI")

                    val parts = relativeFilePath.replace('\\', '/').split('/')
                    val fileName = parts.last()
                    val dirParts = if (parts.size > 1) parts.dropLast(1) else emptyList()

                    // Navigate / create subdirectories
                    for (segment in dirParts) {
                        val existingDir = dir.findFile(segment)
                        dir = if (existingDir != null && existingDir.isDirectory) {
                            existingDir
                        } else {
                            dir.createDirectory(segment)
                                ?: throw IOException("Cannot create directory: $segment")
                        }
                    }

                    // Create or overwrite the target file
                    val mimeType = _guessMimeType(fileName)
                    val existing = dir.findFile(fileName)
                    val docFile = if (existing != null && existing.isFile) {
                        existing  // overwrite by writing to the existing URI
                    } else {
                        dir.createFile(mimeType, fileName)
                            ?: throw IOException("Cannot create file: $fileName")
                    }

                    contentResolver.openOutputStream(docFile.uri, "wt")?.use { out ->
                        File(srcPath).inputStream().use { input ->
                            input.copyTo(out, bufferSize = 65_536)
                        }
                    } ?: throw IOException("Cannot open output stream for $fileName")

                    docFile.uri.toString()
                }
                result.success(savedUri)
            } catch (e: Exception) {
                result.error("SAVE_FAILED", e.message, null)
            }
        }
    }

    // Opens the system Files app at the receive destination.  When [path] is
    // a SAF tree URI (`content://…/tree/…`) we resolve it to a document URI
    // and ACTION_VIEW that — Files apps recognize the directory MIME type
    // and navigate into it.  Otherwise (legacy or default Downloads/Wisp
    // path) we fall back to DownloadManager.ACTION_VIEW_DOWNLOADS so the
    // user still ends up looking at where their files landed.
    private fun openSavedFolder(call: MethodCall, result: MethodChannel.Result) {
        val path = call.argument<String>("path").orEmpty()
        try {
            val opened: Boolean = if (path.startsWith("content://")) {
                // The user picked a SAF folder — open *that* folder. We hold a
                // persisted permission for it, so we can pass the URI grant.
                val treeUri = Uri.parse(path)
                val docId = android.provider.DocumentsContract.getTreeDocumentId(treeUri)
                val docUri = android.provider.DocumentsContract
                    .buildDocumentUriUsingTree(treeUri, docId)
                openFolderInDocumentsUi(docUri, treeUri, grant = true)
            } else {
                // No SAF folder chosen: the stored path is the app-private dir,
                // but files actually land in the PUBLIC Download/Wisp via
                // MediaStore. Open that exact folder (not the generic Downloads
                // root) by addressing it through the external-storage documents
                // provider.
                val publicDir = android.provider.DocumentsContract.buildDocumentUri(
                    "com.android.externalstorage.documents",
                    "primary:Download/Wisp",
                )
                // We don't hold a permission for this public document URI, so
                // we must NOT add the URI grant flags — doing so makes
                // startActivity throw SecurityException. DocumentsUI opens it
                // with its own storage access.
                openFolderInDocumentsUi(publicDir, null, grant = false)
            }
            if (!opened) {
                // No handler navigated to the folder — the generic Downloads
                // view is the least-wrong last resort.
                startActivity(Intent(android.app.DownloadManager.ACTION_VIEW_DOWNLOADS))
            }
            result.success(null)
        } catch (e: Exception) {
            Log.e(OPEN_TAG, "openSavedFolder failed", e)
            result.error("OPEN_FAILED", e.message, null)
        }
    }

    // Opens a folder ([docUri], optional [treeUri] for SAF) in the device's file
    // browser. Different Files apps register for different shapes of this intent,
    // so we try several. Order matters: pin DocumentsUI (the system file
    // browser) FIRST — the generic, un-pinned ACTION_VIEW dir resolves to the
    // *Downloads viewer* on some devices, which ignores our URI and just shows
    // Downloads. Returns true if an activity was launched.
    private fun openFolderInDocumentsUi(docUri: Uri, treeUri: Uri?, grant: Boolean): Boolean {
        val dirMime = android.provider.DocumentsContract.Document.MIME_TYPE_DIR
        // `docsUi` is whichever app handled the folder picker — i.e. the device's
        // real DocumentsUI, even on OEMs that rename it off the AOSP/Google
        // package names.
        val docsUi = resolveDocumentsUiPackage()
        return (docsUi != null && tryViewFolder(docUri, dirMime, docsUi, grant)) ||
            (treeUri != null && docsUi != null && tryViewFolder(treeUri, dirMime, docsUi, grant)) ||
            tryViewFolder(docUri, dirMime, "com.google.android.documentsui", grant) ||
            tryViewFolder(docUri, dirMime, "com.android.documentsui", grant) ||
            tryViewFolder(docUri, dirMime, null, grant) ||
            (treeUri != null && tryViewFolder(treeUri, dirMime, null, grant)) ||
            tryViewFolder(docUri, "resource/folder", null, grant) ||
            tryViewFolder(docUri, null, null, grant)
    }

    // The package of the device's DocumentsUI — i.e. whatever app handles the
    // folder picker (ACTION_OPEN_DOCUMENT_TREE). That same app's file browser is
    // the right target for re-opening a SAF folder, including on OEMs that don't
    // use the AOSP/Google `documentsui` package names. Null if none resolves.
    private fun resolveDocumentsUiPackage(): String? {
        return Intent(Intent.ACTION_OPEN_DOCUMENT_TREE)
            .resolveActivity(packageManager)
            ?.packageName
    }

    // Tries to open a SAF *folder* via ACTION_VIEW. [pkg] optionally pins a
    // specific Files/Documents app. Returns true only if an activity was
    // actually launched (false on ActivityNotFoundException so the caller falls
    // through to the next strategy). We do NOT gate on resolveActivity() — on
    // Android 11+ it can return null for a perfectly launchable intent due to
    // package-visibility rules; its result is logged for diagnostics only.
    private fun tryViewFolder(uri: Uri, mime: String?, pkg: String?, grant: Boolean): Boolean {
        val intent = Intent(Intent.ACTION_VIEW).apply {
            if (mime != null) setDataAndType(uri, mime) else data = uri
            if (pkg != null) setPackage(pkg)
            // Only grant when we actually hold a permission for [uri] (SAF). For
            // public document URIs we don't own, granting throws SecurityException.
            if (grant) {
                addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                        Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            }
        }
        return try {
            startActivity(intent)
            true
        } catch (e: Exception) {
            // ActivityNotFoundException (no handler) or SecurityException
            // (target activity not exported for a pinned package) — either way,
            // fall through to the next strategy rather than aborting the chain.
            false
        }
    }

    // Fires an ACTION_VIEW for a single file [uri] (with optional [mime]),
    // granting read access. Returns true if an activity was launched, false if
    // no app handles it — so callers can fall through to an alternative intent.
    private fun tryViewIntent(uri: Uri, mime: String?): Boolean {
        return try {
            val intent = Intent(Intent.ACTION_VIEW).apply {
                if (mime != null) setDataAndType(uri, mime) else data = uri
                addFlags(
                    Intent.FLAG_GRANT_READ_URI_PERMISSION or
                        Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
                )
            }
            startActivity(intent)
            true
        } catch (_: ActivityNotFoundException) {
            false
        }
    }

    // Opens a single received file at [uri] (a content:// document/MediaStore
    // URI) with the device's default app. Returns true if launched, false if
    // no installed app can handle the file's MIME type.
    private fun openFileUri(call: MethodCall, result: MethodChannel.Result) {
        val uriStr = call.argument<String>("uri")
            ?: return result.error("INVALID", "uri required", null)
        val mime = call.argument<String>("mime") ?: "*/*"
        try {
            val uri = Uri.parse(uriStr)
            // Try the precise MIME first, then a wildcard so a chooser can still
            // appear for types the guesser got wrong / didn't know.
            val opened = tryViewIntent(uri, mime) ||
                (mime != "*/*" && tryViewIntent(uri, "*/*"))
            result.success(opened)
        } catch (e: Exception) {
            result.error("OPEN_FAILED", e.message, null)
        }
    }

    private fun _guessMimeType(fileName: String): String {
        return when (fileName.substringAfterLast('.', "").lowercase()) {
            "jpg", "jpeg" -> "image/jpeg"
            "png" -> "image/png"
            "gif" -> "image/gif"
            "webp" -> "image/webp"
            "mp4" -> "video/mp4"
            "mov" -> "video/quicktime"
            "mp3" -> "audio/mpeg"
            "pdf" -> "application/pdf"
            "txt" -> "text/plain"
            "zip" -> "application/zip"
            else -> "application/octet-stream"
        }
    }
}
