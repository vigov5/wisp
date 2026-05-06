package com.example.drift

import android.app.Activity
import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Environment
import android.provider.MediaStore
import android.provider.OpenableColumns
import androidx.documentfile.provider.DocumentFile
import io.flutter.embedding.android.FlutterActivity
import io.flutter.embedding.engine.FlutterEngine
import io.flutter.plugin.common.MethodCall
import io.flutter.plugin.common.MethodChannel
import java.io.File
import java.io.FileOutputStream
import java.io.IOException

class MainActivity : FlutterActivity() {

    companion object {
        private const val CHANNEL = "com.example.drift/file_picker"
        private const val REQUEST_CODE_PICK_FILES = 2001
        private const val REQUEST_CODE_PICK_FOLDER = 2002
        private const val REQUEST_CODE_PICK_SAVE_FOLDER = 2003
    }

    private var pendingResult: MethodChannel.Result? = null
    private var pendingFolderResult: MethodChannel.Result? = null
    private var pendingSaveFolderResult: MethodChannel.Result? = null

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
                else -> result.notImplemented()
            }
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
                File(cacheDir, "drift_picked"),
                "${System.currentTimeMillis()}_${rootDoc.name.ifBlank { "folder" }}",
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
            val fileName = resolveFileName(uri) ?: "picked_${System.currentTimeMillis()}"
            val dir = File(cacheDir, "drift_picked")
            dir.mkdirs()
            // Prefix with timestamp so repeated picks of the same name don't collide.
            val cacheFile = File(dir, "${System.currentTimeMillis()}_$fileName")
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
        return try {
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

    // Saves a file from [srcPath] into the public Downloads/Drift/ folder.
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
        val relativePath = "Download/Drift${if (subDir.isNotEmpty()) "/$subDir" else ""}"

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
        val destDir = if (subDir.isNotEmpty()) File(File(baseDir, "Drift"), subDir) else File(baseDir, "Drift")
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
