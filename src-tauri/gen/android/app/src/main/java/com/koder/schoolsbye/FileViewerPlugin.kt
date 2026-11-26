package com.koder.schoolsbye

import android.app.Activity
import android.content.Intent
import android.content.pm.PackageInfo
import android.content.pm.PackageManager
import android.os.Build
import android.webkit.MimeTypeMap
import androidx.core.content.FileProvider
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.Plugin
import java.io.File
import java.security.MessageDigest

@InvokeArg
class OpenFileArgs {
  lateinit var path: String
  var mime: String? = null
}

/** Opens a downloaded (cached) file with the system "Open with…" chooser. */
@TauriPlugin
class FileViewerPlugin(private val activity: Activity) : Plugin(activity) {
  private fun mimeOf(file: File, hint: String?): String =
    hint?.takeIf { it.isNotBlank() && it != "application/octet-stream" }
      ?: MimeTypeMap.getSingleton().getMimeTypeFromExtension(file.extension.lowercase())
      ?: "*/*"

  @Command
  fun open(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(OpenFileArgs::class.java)
      val file = File(args.path)
      val uri = FileProvider.getUriForFile(activity, activity.packageName + ".fileprovider", file)
      val view = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, mimeOf(file, args.mime))
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

  /** Sends a downloaded (cached) file to the system share sheet. */
  @Command
  fun share(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(OpenFileArgs::class.java)
      val file = File(args.path)
      val uri = FileProvider.getUriForFile(activity, activity.packageName + ".fileprovider", file)
      val send = Intent(Intent.ACTION_SEND).apply {
        type = mimeOf(file, args.mime)
        putExtra(Intent.EXTRA_STREAM, uri)
        clipData = android.content.ClipData.newRawUri(file.name, uri)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }
      val chooser = Intent.createChooser(send, file.name).apply {
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      }
      activity.startActivity(chooser)
      invoke.resolve()
    } catch (ex: Exception) {
      invoke.reject(ex.message ?: "Cannot share file")
    }
  }

  /** SHA-256 of the certificates the package is currently signed with. */
  @Suppress("DEPRECATION")
  private fun signers(info: PackageInfo?): Set<String> {
    val sigs = if (Build.VERSION.SDK_INT >= 28) info?.signingInfo?.apkContentsSigners else info?.signatures
    return sigs.orEmpty().map { sig ->
      MessageDigest.getInstance("SHA-256").digest(sig.toByteArray()).joinToString("") { "%02x".format(it) }
    }.toSet()
  }

  /**
   * Hands a downloaded APK to the system package installer (app update) — only if it is
   * this same app signed with the same key. Independent of GitHub: a swapped release
   * without our keystore is refused here.
   */
  @Command
  fun install(invoke: Invoke) {
    try {
      val args = invoke.parseArgs(OpenFileArgs::class.java)
      val pm = activity.packageManager
      @Suppress("DEPRECATION")
      val flags = if (Build.VERSION.SDK_INT >= 28) PackageManager.GET_SIGNING_CERTIFICATES else PackageManager.GET_SIGNATURES
      val apk = pm.getPackageArchiveInfo(args.path, flags)?.apply {
        applicationInfo?.sourceDir = args.path
        applicationInfo?.publicSourceDir = args.path
      }
      val mine = pm.getPackageInfo(activity.packageName, flags)
      val theirs = signers(apk)
      if (apk == null || apk.packageName != activity.packageName || theirs.isEmpty() || theirs != signers(mine)) {
        File(args.path).delete()
        invoke.reject("UPDATE_VERIFY: signature mismatch")
        return
      }
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
