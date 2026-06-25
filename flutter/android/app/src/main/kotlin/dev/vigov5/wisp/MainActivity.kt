package dev.vigov5.wisp

import android.Manifest
import android.app.Activity
import android.content.ActivityNotFoundException
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.os.PowerManager
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.provider.Settings
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
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Deferred
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.async
import kotlinx.coroutines.launch

// Extends FlutterFragmentActivity (not FlutterActivity) so local_auth's
// BiometricPrompt can attach — it requires a FragmentActivity host.
class MainActivity : FlutterFragmentActivity() {

    companion object {
        private const val CHANNEL = "dev.vigov5.wisp/file_picker"
        private const val KEEPALIVE_CHANNEL = "dev.vigov5.wisp/transfer_keepalive"
        private const val SHARE_CHANNEL = "dev.vigov5.wisp/share_intent"
        private const val USB_TETHER_CHANNEL = "dev.vigov5.wisp/usb_tether"
        private const val REQUEST_CODE_PICK_FILES = 2001
        private const val REQUEST_CODE_PICK_FOLDER = 2002
        private const val REQUEST_CODE_PICK_SAVE_FOLDER = 2003
        private const val REQUEST_CODE_POST_NOTIF = 4801
    }

    private var pendingResult: MethodChannel.Result? = null
    private var pendingFolderResult: MethodChannel.Result? = null
    private var pendingSaveFolderResult: MethodChannel.Result? = null

    // Deferred holding the cached file paths produced from an ACTION_SEND /
    // ACTION_SEND_MULTIPLE intent.  The copy itself runs on Dispatchers.IO,
    // and Flutter awaits this when calling getInitialSharedFiles, so a
    // multi-hundred-megabyte share never blocks the main thread (or the
    // launch screen).
    private var initialSharedFilesJob: Deferred<List<String>>? = null
    // Cold-start stash for an ACTION_SEND text/plain share (EXTRA_TEXT, no
    // EXTRA_STREAM).  Handed to Flutter once via getInitialSharedText.
    private var initialSharedText: String? = null
    private var shareChannel: MethodChannel? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        initialSharedFilesJob = extractSharedFilesAsync(intent)
        initialSharedText = extractSharedText(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        // Persist the new intent so any later getIntent() lookups see it
        // (otherwise we'd keep re-reading the original launch intent).
        setIntent(intent)

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
            if (files.isEmpty()) return@launch
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
    // skip it entirely; otherwise kicks off the URI → cache copy on
    // Dispatchers.IO and returns the in-flight Deferred.  Tied to
    // lifecycleScope so the work is cancelled if the activity dies.
    private fun extractSharedFilesAsync(intent: Intent?): Deferred<List<String>>? {
        if (intent == null) return null
        if (intent.action != Intent.ACTION_SEND &&
            intent.action != Intent.ACTION_SEND_MULTIPLE
        ) return null
        return lifecycleScope.async(Dispatchers.IO) {
            extractSharedFilesFromIntent(intent) ?: emptyList()
        }
    }

    // Returns the plain text of an ACTION_SEND text/plain share, or null when
    // the intent isn't a text share.  A file share (EXTRA_STREAM present) is
    // left to the file pipeline even if it also carries a text caption.
    private fun extractSharedText(intent: Intent?): String? {
        if (intent == null) return null
        if (intent.action != Intent.ACTION_SEND) return null
        val hasStream = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java) != null
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM) != null
        }
        if (hasStream) return null
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString()
        return text?.takeIf { it.isNotEmpty() }
    }

    override fun configureFlutterEngine(flutterEngine: FlutterEngine) {
        super.configureFlutterEngine(flutterEngine)
        MethodChannel(
            flutterEngine.dartExecutor.binaryMessenger,
            CHANNEL,
        ).setMethodCallHandler { call, result ->
            when (call.method) {
                "pickFiles" -> {
                    if (pendingResult != null) {
                        result.error("ALREADY_PICKING", "A file pick is already in progress", null)
                        return@setMethodCallHandler
                    }
                    pendingResult = result
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
                "saveToSafUri" -> saveToSafUri(call, result)
                "openSavedFolder" -> openSavedFolder(call, result)
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
                else -> result.notImplemented()
            }
        }

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
                                // Activity destroyed mid-copy (job cancelled)
                                // or copy threw — surface as empty rather
                                // than failing the channel call.
                                result.success(emptyList<String>())
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

    // Pulls file URIs out of an ACTION_SEND / ACTION_SEND_MULTIPLE intent
    // and copies each one into the app cache via the same `wisp_picked`
    // tree the file picker uses.  Returns the resulting local paths.
    // Returns null when the intent isn't a share intent at all so the
    // caller can distinguish "no share" from "empty share".
    private fun extractSharedFilesFromIntent(intent: Intent?): List<String>? {
        if (intent == null) return null
        val uris: List<Uri> = when (intent.action) {
            Intent.ACTION_SEND -> {
                val uri = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
                } else {
                    @Suppress("DEPRECATION")
                    intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
                }
                if (uri != null) listOf(uri) else emptyList()
            }
            Intent.ACTION_SEND_MULTIPLE -> {
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
            else -> return null
        }
        return uris.mapNotNull { copyUriToCache(it) }
    }

    private fun handleKeepaliveCall(call: MethodCall, result: MethodChannel.Result) {
        when (call.method) {
            "start", "update" -> {
                val title = call.argument<String>("title").orEmpty()
                val body = call.argument<String>("body").orEmpty()
                if (call.method == "start") {
                    ensureNotificationPermission()
                }
                val intent = Intent(this, TransferKeepaliveService::class.java)
                    .putExtra(TransferKeepaliveService.EXTRA_TITLE, title)
                    .putExtra(TransferKeepaliveService.EXTRA_BODY, body)
                ContextCompat.startForegroundService(this, intent)
                result.success(null)
            }
            "stop" -> {
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
        if (requestCode == REQUEST_CODE_PICK_FILES) {
            val result = pendingResult
            pendingResult = null
            if (result == null) {
                super.onActivityResult(requestCode, resultCode, data)
                return
            }
            if (resultCode != Activity.RESULT_OK || data == null) {
                result.success(emptyList<String>())
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
            val paths = uris.mapNotNull { uri -> copyUriToCache(uri) }
            result.success(paths)
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
            // Copy the entire folder tree into the app cache so that Rust can
            // read it via ordinary filesystem APIs. External storage paths are
            // blocked by Android scoped storage for native (non-SAF) callers.
            val destDir = File(
                File(File(cacheDir, "wisp_picked"), System.currentTimeMillis().toString()),
                rootDoc.name.ifBlank { "folder" },
            )
            val sizeBytes = copyDocumentTreeToCache(rootDoc, destDir)
            result.success(mapOf("path" to destDir.absolutePath, "sizeBytes" to sizeBytes))
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

    // Streams a content URI to the app cache directory to avoid encoding
    // large files as bytes through the Flutter platform channel.
    private fun copyUriToCache(uri: Uri): String? {
        return try {
            val fileName = resolveFileName(uri) ?: "picked_file"
            val dir = File(File(cacheDir, "wisp_picked"), System.currentTimeMillis().toString())
            dir.mkdirs()
            // Use a timestamped subdirectory so repeated picks of the same name don't collide.
            val cacheFile = File(dir, fileName)
            contentResolver.openInputStream(uri)?.use { input ->
                FileOutputStream(cacheFile).use { output ->
                    input.copyTo(output, bufferSize = 65_536)
                }
            }
            cacheFile.absolutePath
        } catch (_: Exception) {
            null
        }
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

    // Recursively copies a FastDocumentFile tree into [destDir] using 64 KB
    // streaming chunks via SAF.  Uses a single ContentResolver query per
    // directory level (ported from LocalSend's FastDocumentFile approach) so
    // it scales well to folders with many files.  Returns the total bytes copied.
    private fun copyDocumentTreeToCache(src: FastDocumentFile, destDir: File): Long {
        destDir.mkdirs()
        var totalBytes = 0L
        for (child in src.listFiles()) {
            if (child.name.isBlank()) continue
            if (child.isDirectory) {
                totalBytes += copyDocumentTreeToCache(child, File(destDir, child.name))
            } else if (child.isFile) {
                val destFile = File(destDir, child.name)
                try {
                    contentResolver.openInputStream(child.uri)?.use { input ->
                        FileOutputStream(destFile).use { output ->
                            totalBytes += input.copyTo(output, bufferSize = 65_536)
                        }
                    }
                } catch (_: Exception) {
                    // Skip unreadable files; the transfer will surface the gap.
                }
            }
        }
        return totalBytes
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

        try {
            val savedPath = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
                saveToDownloadsQ(srcPath, relativeFilePath, mimeType)
            } else {
                saveToDownloadsLegacy(srcPath, relativeFilePath)
            }
            result.success(savedPath)
        } catch (e: Exception) {
            result.error("SAVE_FAILED", e.message, null)
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

        try {
            val treeUri = Uri.parse(treeUriStr)
            var dir = DocumentFile.fromTreeUri(this, treeUri)
                ?: throw IOException("Cannot open folder URI")

            val parts = relativeFilePath.replace('\\', '/').split('/')
            val fileName = parts.last()
            val dirParts = if (parts.size > 1) parts.dropLast(1) else emptyList()

            // Navigate / create subdirectories
            for (segment in dirParts) {
                val existing = dir.findFile(segment)
                dir = if (existing != null && existing.isDirectory) {
                    existing
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

            result.success(docFile.uri.toString())
        } catch (e: Exception) {
            result.error("SAVE_FAILED", e.message, null)
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
            val intent: Intent = if (path.startsWith("content://")) {
                val treeUri = Uri.parse(path)
                val docId = android.provider.DocumentsContract.getTreeDocumentId(treeUri)
                val docUri = android.provider.DocumentsContract
                    .buildDocumentUriUsingTree(treeUri, docId)
                Intent(Intent.ACTION_VIEW).apply {
                    setDataAndType(
                        docUri,
                        android.provider.DocumentsContract.Document.MIME_TYPE_DIR,
                    )
                    addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                }
            } else {
                Intent(android.app.DownloadManager.ACTION_VIEW_DOWNLOADS)
            }

            try {
                startActivity(intent)
            } catch (_: ActivityNotFoundException) {
                // Some Files apps reject the directory MIME type; fall back
                // to the generic Downloads view so the button never silently
                // fails on the user.
                startActivity(Intent(android.app.DownloadManager.ACTION_VIEW_DOWNLOADS))
            }
            result.success(null)
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
