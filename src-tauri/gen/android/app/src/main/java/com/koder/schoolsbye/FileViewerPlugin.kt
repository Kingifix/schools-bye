package com.koder.schoolsbye

import android.app.Activity
import android.content.Intent
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import java.io.File

@InvokeArg
class OpenFileArgs {
  lateinit var path: String
  var mime: String? = null
}

/** Opens a downloaded (cached) file with the system "Open with…" chooser. */
@TauriPlugin
class FileViewerPlugin(private val activity: Activity) : Plugin(activity) {
  @Command
  fun open(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(OpenFileArgs::class.java)
      val file = File(args.path)
      val uri = FileProvider.getUriForFile(activity, activity.packageName + ".fileprovider", file)
      val fromExt = MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase())
      val mime = args.mime?.takeIf { it.isNotBlank() && it != "application/octet-stream" }
        ?: fromExt
        ?: "*/*"
      val view = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, mime)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }
      val chooser = Intent.createChooser(view, file.name).apply {
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }
      activity.startActivity(chooser)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message ?: "Cannot open file")
    }
  }

  /** Hands a downloaded APK to the system package installer (app update). */
  @Command
  fun install(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(OpenFileArgs::class.java)
      val uri = FileProvider.getUriForFile(activity, activity.packageName + ".fileprovider", File(args.path))
      val intent = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, "application/vnd.android.package-archive")
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_ACTIVITY_NEW_TASK)
      }
      activity.startActivity(intent)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message ?: "Cannot install update")
    }
  }
}
