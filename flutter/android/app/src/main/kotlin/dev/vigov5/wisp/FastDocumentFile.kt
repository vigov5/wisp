package dev.vigov5.wisp

import android.content.ContentResolver
import android.content.Context
import android.database.Cursor
import android.net.Uri
import android.provider.DocumentsContract
import android.util.Log

private const val MIME_TYPE_DIR = "vnd.android.document/directory"
private const val TAG = "FastDocumentFile"

/**
 * Faster alternative to AndroidX DocumentFile.
 * Ported from LocalSend (https://github.com/localsend/localsend).
 *
 * The key difference: [listFiles] fetches all child metadata (MIME type,
 * document ID, display name, size, last-modified) in a **single**
 * ContentResolver query instead of one query per child, which is critical
 * for folders with many files.
 */
class FastDocumentFile(
    private val context: Context,
    val mime: String,
    val uri: Uri,
    val name: String,
    val size: Long,
) {
    val isDirectory: Boolean = mime == MIME_TYPE_DIR
    val isFile: Boolean = !isDirectory && mime.isNotBlank()

    fun listFiles(): List<FastDocumentFile> {
        val resolver: ContentResolver = context.contentResolver
        val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(
            uri,
            DocumentsContract.getDocumentId(uri),
        )

        val results = mutableListOf<FastDocumentFile>()
        var cursor: Cursor? = null
        try {
            cursor = resolver.query(
                childrenUri,
                arrayOf(
                    DocumentsContract.Document.COLUMN_MIME_TYPE,
                    DocumentsContract.Document.COLUMN_DOCUMENT_ID,
                    DocumentsContract.Document.COLUMN_DISPLAY_NAME,
                    DocumentsContract.Document.COLUMN_SIZE,
                ),
                null, null, null,
            )
            while (cursor!!.moveToNext()) {
                results.add(
                    FastDocumentFile(
                        context = context,
                        mime = cursor.getString(0),
                        uri = DocumentsContract.buildDocumentUriUsingTree(
                            uri,
                            cursor.getString(1),
                        ),
                        name = cursor.getString(2),
                        size = cursor.getLong(3),
                    ),
                )
            }
        } catch (e: Exception) {
            Log.w(TAG, "listFiles error: $e")
        } finally {
            try { cursor?.close() } catch (_: Exception) {}
        }
        return results
    }

    companion object {
        fun fromTreeUri(context: Context, treeUri: Uri): FastDocumentFile {
            val documentId = when {
                DocumentsContract.isDocumentUri(context, treeUri) ->
                    DocumentsContract.getDocumentId(treeUri)
                else ->
                    DocumentsContract.getTreeDocumentId(treeUri)
            }
            // Resolve the display name via a single query.
            val docUri = DocumentsContract.buildDocumentUriUsingTree(treeUri, documentId)
            var name = ""
            try {
                context.contentResolver.query(
                    docUri,
                    arrayOf(DocumentsContract.Document.COLUMN_DISPLAY_NAME),
                    null, null, null,
                )?.use { cursor ->
                    if (cursor.moveToFirst()) name = cursor.getString(0)
                }
            } catch (_: Exception) {}

            return FastDocumentFile(
                context = context,
                mime = MIME_TYPE_DIR,
                uri = docUri,
                name = name,
                size = 0,
            )
        }
    }
}
