package com.runhelp.desktop

import android.Manifest
import android.app.Activity
import android.content.ContentUris
import android.content.ContentValues
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.graphics.Bitmap
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.drawable.BitmapDrawable
import android.graphics.drawable.Drawable
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import androidx.activity.result.ActivityResult
import androidx.core.app.ActivityCompat
import androidx.core.content.ContextCompat
import androidx.core.content.FileProvider
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.provider.DocumentsContract
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Base64
import android.util.Log
import android.util.Size
import androidx.core.app.NotificationCompat
import androidx.core.view.WindowCompat
import app.tauri.annotation.ActivityCallback
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.ByteArrayOutputStream
import java.io.File
import java.io.FileOutputStream
import java.text.Collator
import java.util.Locale
import java.util.concurrent.Executors
import java.util.concurrent.TimeUnit

private const val TAG = "RunHelpMobile"

/** 应用内「接收目录」弹层一次最多列出的文件数（几百项足够，避免弹层自己变卡）。 */
private const val LIST_LIMIT = 300

/** 相册里的一条媒体（图片或视频） */
private class MediaRow(
  val uri: Uri,
  val name: String,
  val size: Long,
  val mime: String,
  val video: Boolean,
  /** 拍摄/添加时间（Unix 秒），前端用来按日期分组 */
  val date: Long,
)

/**
 * 移动端原生插件：为数据同步提供后台保活能力。
 * - startKeepAlive：启动前台服务（常驻通知）+ CPU/WiFi 锁，避免锁屏/切后台后
 *   UDP 发现广播与 TCP 47801 监听被系统冻结或杀掉。
 * - stopKeepAlive：释放锁并停止前台服务，与 sync_stop 联动。
 *
 * 注意：此类以 @TauriPlugin 标注，由 Rust 侧 register_android_plugin 按类名反射加载，
 * @Command 方法运行时反射发现，无需 KSP 代码生成。
 */
@TauriPlugin
class RunhelpMobilePlugin(private val activity: Activity) : Plugin(activity) {

  companion object {
    /** 相册读取权限请求码：MainActivity.onRequestPermissionsResult 会转交回来 */
    private const val REQ_MEDIA_PERMISSION = 47811
    private var permissionContinuation: ((Boolean) -> Unit)? = null

    /** 权限结果回调（由 MainActivity 调用，避免插件自己去 override Activity 方法） */
    @JvmStatic
    fun dispatchPermissionResult(requestCode: Int, grantResults: IntArray) {
      if (requestCode != REQ_MEDIA_PERMISSION) return
      val cb = permissionContinuation ?: return
      permissionContinuation = null
      cb(grantResults.isNotEmpty() && grantResults.all { it == PackageManager.PERMISSION_GRANTED })
    }
  }

  private fun mediaPermissions(): Array<String> = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
    arrayOf(Manifest.permission.READ_MEDIA_IMAGES, Manifest.permission.READ_MEDIA_VIDEO)
  } else {
    arrayOf(Manifest.permission.READ_EXTERNAL_STORAGE)
  }

  private fun hasMediaPermission(): Boolean = mediaPermissions().all {
    ContextCompat.checkSelfPermission(activity, it) == PackageManager.PERMISSION_GRANTED
  }

  /**
   * 列出相册最近的照片与视频（图片 + 视频混排），供应用内九宫格选择。
   *
   * 为什么不用系统选择器：ACTION_PICK / ACTION_PICK_IMAGES 一次只接受单一 MIME，
   * ACTION_GET_CONTENT 的 EXTRA_MIME_TYPES 在部分 ROM（含鸿蒙兼容层）里不生效，
   * 结果只能选图片。这里自己查 MediaStore，图片与视频天然混选。
   * 返回 { items: [{ uri, name, size, mime, video, thumb }] }，thumb 是 base64 缩略图。
   */
  @Command
  fun listMedia(invoke: Invoke) {
    val limit = try {
      invoke.getArgs().optInt("limit", 200).coerceIn(1, 600)
    } catch (_: Exception) {
      200
    }
    if (hasMediaPermission()) {
      Thread { resolveMediaList(invoke, limit) }.start()
      return
    }
    permissionContinuation = { granted ->
      if (granted) {
        Thread { resolveMediaList(invoke, limit) }.start()
      } else {
        activity.runOnUiThread { invoke.reject("permission_denied") }
      }
    }
    try {
      activity.runOnUiThread {
        ActivityCompat.requestPermissions(activity, mediaPermissions(), REQ_MEDIA_PERMISSION)
      }
    } catch (e: Exception) {
      permissionContinuation = null
      Log.e(TAG, "request media permission failed", e)
      invoke.reject(e.message ?: "无法申请相册权限")
    }
  }

  private fun resolveMediaList(invoke: Invoke, limit: Int) {
    try {
      val resolver = activity.contentResolver
      val collection = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
      } else {
        MediaStore.Files.getContentUri("external")
      }
      val projection = arrayOf(
        MediaStore.MediaColumns._ID,
        MediaStore.MediaColumns.DISPLAY_NAME,
        MediaStore.MediaColumns.SIZE,
        MediaStore.MediaColumns.MIME_TYPE,
        MediaStore.Files.FileColumns.MEDIA_TYPE,
        MediaStore.MediaColumns.DATE_ADDED,
      )
      val selection = "${MediaStore.Files.FileColumns.MEDIA_TYPE}=? OR ${MediaStore.Files.FileColumns.MEDIA_TYPE}=?"
      val args = arrayOf(
        MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE.toString(),
        MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO.toString(),
      )
      val order = "${MediaStore.MediaColumns.DATE_ADDED} DESC"

      val rows = ArrayList<MediaRow>()
      resolver.query(collection, projection, selection, args, order)?.use { c ->
        val idIdx = c.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
        val nameIdx = c.getColumnIndex(MediaStore.MediaColumns.DISPLAY_NAME)
        val sizeIdx = c.getColumnIndex(MediaStore.MediaColumns.SIZE)
        val mimeIdx = c.getColumnIndex(MediaStore.MediaColumns.MIME_TYPE)
        val typeIdx = c.getColumnIndex(MediaStore.Files.FileColumns.MEDIA_TYPE)
        val dateIdx = c.getColumnIndex(MediaStore.MediaColumns.DATE_ADDED)
        while (c.moveToNext() && rows.size < limit) {
          val id = c.getLong(idIdx)
          val type = if (typeIdx >= 0) c.getInt(typeIdx) else MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE
          val isVideo = type == MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO
          val base = if (isVideo) MediaStore.Video.Media.EXTERNAL_CONTENT_URI else MediaStore.Images.Media.EXTERNAL_CONTENT_URI
          val mime = if (mimeIdx >= 0) c.getString(mimeIdx) ?: "" else ""
          rows.add(
            MediaRow(
              uri = ContentUris.withAppendedId(base, id),
              name = (if (nameIdx >= 0) c.getString(nameIdx) else null) ?: "media_$id",
              size = if (sizeIdx >= 0) c.getLong(sizeIdx) else 0L,
              mime = mime,
              video = isVideo,
              date = if (dateIdx >= 0) c.getLong(dateIdx) else 0L,
            ),
          )
        }
      }

      // 缩略图并行生成，避免上百条串行解码把首屏拖到几秒
      val thumbs = arrayOfNulls<String>(rows.size)
      if (rows.isNotEmpty()) {
        val pool = Executors.newFixedThreadPool(6)
        rows.forEachIndexed { i, row -> pool.execute { thumbs[i] = thumbDataUrl(row.uri, 300) } }
        pool.shutdown()
        pool.awaitTermination(8, TimeUnit.SECONDS)
      }

      val items = JSArray()
      rows.forEachIndexed { i, row ->
        items.put(JSObject().apply {
          put("uri", row.uri.toString())
          put("name", row.name)
          put("size", row.size)
          put("mime", row.mime)
          put("video", row.video)
          put("date", row.date)
          put("thumb", thumbs[i] ?: "")
        })
      }
      invoke.resolveObject(JSObject().apply { put("items", items) })
    } catch (e: Exception) {
      Log.e(TAG, "listMedia failed", e)
      invoke.reject(e.message ?: "读取相册失败")
    }
  }

  /**
   * 最近文件（不含相册里的图片/视频，那些走「媒体」）。
   * 点击「文件」时直接给出最近用过的文档/压缩包/安装包/音频，省去在目录里翻找。
   * 返回 { items: [{ uri, name, size, mime, date }] }。
   */
  @Command
  fun listRecentFiles(invoke: Invoke) {
    val limit = try {
      invoke.getArgs().optInt("limit", 120).coerceIn(1, 500)
    } catch (_: Exception) {
      120
    }
    Thread {
      try {
        val resolver = activity.contentResolver
        val collection = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
          MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
        } else {
          MediaStore.Files.getContentUri("external")
        }
        val projection = arrayOf(
          MediaStore.MediaColumns._ID,
          MediaStore.MediaColumns.DISPLAY_NAME,
          MediaStore.MediaColumns.SIZE,
          MediaStore.MediaColumns.MIME_TYPE,
          MediaStore.MediaColumns.DATE_ADDED,
        )
        // 只要 MediaStore 里的「非媒体」条目：文档、压缩包、安装包、音频等。
        val selection = "${MediaStore.Files.FileColumns.MEDIA_TYPE}!=? AND " +
          "${MediaStore.Files.FileColumns.MEDIA_TYPE}!=?"
        val args = arrayOf(
          MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE.toString(),
          MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO.toString(),
        )
        val order = "${MediaStore.MediaColumns.DATE_ADDED} DESC"
        val items = JSArray()
        resolver.query(collection, projection, selection, args, order)?.use { c ->
          val idIdx = c.getColumnIndexOrThrow(MediaStore.MediaColumns._ID)
          val nameIdx = c.getColumnIndex(MediaStore.MediaColumns.DISPLAY_NAME)
          val sizeIdx = c.getColumnIndex(MediaStore.MediaColumns.SIZE)
          val mimeIdx = c.getColumnIndex(MediaStore.MediaColumns.MIME_TYPE)
          val dateIdx = c.getColumnIndex(MediaStore.MediaColumns.DATE_ADDED)
          while (c.moveToNext() && items.length() < limit) {
            val name = (if (nameIdx >= 0) c.getString(nameIdx) else null) ?: continue
            if (name.startsWith(".")) continue
            val id = c.getLong(idIdx)
            items.put(JSObject().apply {
              put("uri", ContentUris.withAppendedId(collection, id).toString())
              put("name", name)
              put("size", if (sizeIdx >= 0) c.getLong(sizeIdx) else 0L)
              put("mime", if (mimeIdx >= 0) c.getString(mimeIdx) ?: "" else "")
              put("date", if (dateIdx >= 0) c.getLong(dateIdx) else 0L)
            })
          }
        }
        invoke.resolveObject(JSObject().apply { put("items", items) })
      } catch (e: Exception) {
        Log.e(TAG, "listRecentFiles failed", e)
        invoke.reject(e.message ?: "读取最近文件失败")
      }
    }.start()
  }

  /**
   * 已安装应用列表（发 APK 用）。
   * - excludeSystem：排除系统应用（默认开，和 LocalSend 一致）
   * - excludeNonLaunchable：排除没有启动入口的应用（默认开）
   * 返回 { items: [{ package, label, version, size, icon, system }] }，icon 是 base64 缩略图。
   */
  @Command
  fun listApps(invoke: Invoke) {
    val args = invoke.getArgs()
    val excludeSystem = try { args.optBoolean("excludeSystem", true) } catch (_: Exception) { true }
    val excludeNonLaunchable = try { args.optBoolean("excludeNonLaunchable", true) } catch (_: Exception) { true }
    Thread {
      try {
        val pm = activity.packageManager
        val launcherIntent = Intent(Intent.ACTION_MAIN).addCategory(Intent.CATEGORY_LAUNCHER)
        val collected = LinkedHashMap<String, ApplicationInfo>()
        if (excludeNonLaunchable) {
          val resolved = pm.queryIntentActivities(launcherIntent, 0)
          for (ri in resolved) {
            val ai = ri.activityInfo?.applicationInfo ?: continue
            collected[ai.packageName] = ai
          }
        } else {
          for (ai in pm.getInstalledApplications(0)) {
            collected[ai.packageName] = ai
          }
        }
        val entries = ArrayList<Pair<String, ApplicationInfo>>()
        for ((pkg, ai) in collected) {
          if (pkg == activity.packageName) continue
          val isSystem = ai.flags and ApplicationInfo.FLAG_SYSTEM != 0
          if (excludeSystem && isSystem) continue
          entries.add(pkg to ai)
        }
        // 按名称排序（Collator 支持中文按拼音）
        val collator = Collator.getInstance(Locale.CHINA)
        val labeled = entries.map { (pkg, ai) ->
          Triple(pkg, ai, pm.getApplicationLabel(ai).toString())
        }.sortedWith { a, b -> collator.compare(a.third, b.third) }

        val items = JSArray()
        val pool = Executors.newFixedThreadPool(4)
        val icons = arrayOfNulls<String>(labeled.size)
        labeled.forEachIndexed { i, entry ->
          pool.execute { icons[i] = iconDataUrl(pm.getApplicationIcon(entry.second)) }
        }
        pool.shutdown()
        pool.awaitTermination(6, TimeUnit.SECONDS)
        labeled.forEachIndexed { i, (pkg, ai, label) ->
          items.put(JSObject().apply {
            put("package", pkg)
            put("label", label)
            put("version", try { pm.getPackageInfo(pkg, 0).versionName ?: "" } catch (_: Exception) { "" })
            put("size", File(ai.sourceDir).length())
            put("system", ai.flags and ApplicationInfo.FLAG_SYSTEM != 0)
            put("icon", icons[i] ?: "")
          })
        }
        invoke.resolveObject(JSObject().apply { put("items", items) })
      } catch (e: Exception) {
        Log.e(TAG, "listApps failed", e)
        invoke.reject(e.message ?: "读取应用列表失败")
      }
    }.start()
  }

  /** 把选中应用的 APK 复制到缓存目录，返回可发送的真实路径。 */
  @Command
  fun prepareApks(invoke: Invoke) {
    Thread {
      try {
        val args = invoke.getArgs()
        val pkgs = if (args.has("packages")) JSArray(args.getJSONArray("packages").toString()) else JSArray()
        val outDir = File(activity.cacheDir, "sync_apks")
        if (!outDir.exists()) outDir.mkdirs()
        val pm = activity.packageManager
        val items = JSArray()
        for (i in 0 until pkgs.length()) {
          val pkg = pkgs.getString(i) ?: continue
          val ai = try { pm.getApplicationInfo(pkg, 0) } catch (e: Exception) { continue }
          val label = pm.getApplicationLabel(ai).toString().replace(Regex("[\\\\/:*?\"<>|]"), "_")
          val src = File(ai.sourceDir)
          if (!src.isFile) continue
          val dest = File(outDir, "$label.apk")
          src.inputStream().use { input -> FileOutputStream(dest).use { out -> input.copyTo(out) } }
          items.put(JSObject().apply {
            put("path", dest.absolutePath)
            put("name", "$label.apk")
          })
        }
        invoke.resolveObject(JSObject().apply { put("items", items) })
      } catch (e: Exception) {
        Log.e(TAG, "prepareApks failed", e)
        invoke.reject(e.message ?: "导出安装包失败")
      }
    }.start()
  }

  /** 生成缩略图 data URL（WebView 读不了 file://，只能内联 base64）。 */
  private fun thumbDataUrl(uri: Uri, size: Int = 180): String {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return ""
    return try {
      val bmp = activity.contentResolver.loadThumbnail(uri, Size(size, size), null)
      val out = ByteArrayOutputStream()
      bmp.compress(Bitmap.CompressFormat.JPEG, 70, out)
      bmp.recycle()
      "data:image/jpeg;base64," + Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
    } catch (e: Exception) {
      Log.w(TAG, "thumb failed: ${e.message}")
      ""
    }
  }

  /** 应用图标转 data URL（48dp PNG）。 */
  private fun iconDataUrl(drawable: Drawable): String {
    return try {
      val px = (48 * activity.resources.displayMetrics.density).toInt().coerceAtLeast(48)
      val bmp = if (drawable is BitmapDrawable && drawable.bitmap != null) {
        Bitmap.createScaledBitmap(drawable.bitmap, px, px, true)
      } else {
        Bitmap.createBitmap(px, px, Bitmap.Config.ARGB_8888).also { b ->
          val canvas = Canvas(b)
          drawable.setBounds(0, 0, px, px)
          drawable.draw(canvas)
        }
      }
      val out = ByteArrayOutputStream()
      bmp.compress(Bitmap.CompressFormat.PNG, 80, out)
      "data:image/png;base64," + Base64.encodeToString(out.toByteArray(), Base64.NO_WRAP)
    } catch (e: Exception) {
      Log.w(TAG, "icon failed: ${e.message}")
      ""
    }
  }

  @Command
  fun startKeepAlive(invoke: Invoke) {
    try {
      val intent = Intent(activity, SyncKeepAliveService::class.java)
      intent.action = SyncKeepAliveService.ACTION_START
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
        activity.startForegroundService(intent)
      } else {
        activity.startService(intent)
      }
      Log.i(TAG, "keep-alive service started")
      invoke.resolve()
    } catch (e: Exception) {
      Log.e(TAG, "startKeepAlive failed", e)
      invoke.reject(e.message ?: "无法启动后台保活服务")
    }
  }

  @Command
  fun stopKeepAlive(invoke: Invoke) {
    try {
      val intent = Intent(activity, SyncKeepAliveService::class.java)
      intent.action = SyncKeepAliveService.ACTION_STOP
      activity.startService(intent)
      Log.i(TAG, "keep-alive service stop requested")
      invoke.resolve()
    } catch (e: Exception) {
      Log.e(TAG, "stopKeepAlive failed", e)
      invoke.reject(e.message ?: "无法停止后台保活服务")
    }
  }

  /** 切换主题时同步设置 Android 状态栏/导航栏颜色，避免华为手势条出现白边。 */
  @Command
  fun setTheme(invoke: Invoke) {
    try {
      val theme = invoke.getArgs().optString("theme", "soft-light")
      val light = theme != "soft-cyan" && theme != "dark"
      val color = Color.parseColor(if (light) "#E8F0E9" else "#101B1C")
      activity.window.statusBarColor = color
      activity.window.navigationBarColor = color
      WindowCompat.getInsetsController(activity.window, activity.window.decorView).apply {
        isAppearanceLightStatusBars = light
        isAppearanceLightNavigationBars = light
      }
      invoke.resolve()
    } catch (e: Exception) {
      Log.e(TAG, "setTheme failed", e)
      invoke.reject(e.message ?: "切换主题失败")
    }
  }

  /** 用系统文件管理器打开应用私有接收目录（通过 FileProvider 授权）。 */
  @Command
  fun openReceiveDir(invoke: Invoke) {
    try {
      openPrivateReceiveDir()
      invoke.resolve()
    } catch (e: Exception) {
      Log.e(TAG, "openReceiveDir failed", e)
      invoke.reject(e.message ?: "无法打开接收目录")
    }
  }

  /**
   * 接收目录里的一条文件（供应用内「接收目录」弹层展示）。
   */
  private class SavedRow(
    val name: String,
    val size: Long,
    val date: Long,
    val uri: Uri,
    val mime: String,
  )

  private fun SavedRow.toJson(): JSObject = JSObject().apply {
    put("name", name)
    put("size", size)
    put("date", date)
    put("uri", uri.toString())
    put("mime", mime)
  }

  /**
   * 列出接收目录里的文件，供应用内弹层直接查看。
   *
   * 为什么不再打开系统文件管理器：恢复默认接收位置后接收目录是「下载/RunHelp」，
   * 它只能靠拼 document URI（`document/primary:Download/RunHelp`）去开——这个 URI
   * 既没授权给我们，也不是 DocumentsUI 认可的 tree URI。鸿蒙/EMUI 的文件管理器
   * 拿到它会开出一个空白页然后无响应，就是「恢复默认后点打开目录白屏卡死」。
   * 手动选过一次 SAF 目录后走的是 buildDocumentUriUsingTree + 持久授权，所以又正常。
   *
   * 现在改为自己列文件：SAF 目录查子项、默认目录查 MediaStore（Download/RunHelp），
   * 都是可靠的查询，不再依赖任何拼出来的目录 URI。
   * 返回 { dir: 目录名, items: [{ name, size, date, uri, mime }] }。
   */
  @Command
  fun listSavedFiles(invoke: Invoke) {
    val tree = try {
      invoke.getArgs().optString("tree", "")
    } catch (_: Exception) {
      ""
    }
    Thread {
      try {
        val (label, rows) = resolveSavedFiles(tree)
        val out = JSObject()
        out.put("dir", label)
        val arr = JSArray()
        for (row in rows) arr.put(row.toJson())
        out.put("items", arr)
        invoke.resolveObject(out)
      } catch (e: Exception) {
        Log.e(TAG, "listSavedFiles failed", e)
        invoke.reject(e.message ?: "无法读取接收目录")
      }
    }.start()
  }

  /** 打开接收目录里的某个文件（uri 来自 listSavedFiles，本身带授权或来自 MediaStore）。 */
  @Command
  fun openSavedFile(invoke: Invoke) {
    try {
      val args = invoke.getArgs()
      val uri = Uri.parse(args.optString("uri", ""))
      val mime = args.optString("mime", "").ifBlank { guessMime(uri.lastPathSegment ?: "") }
      val intent = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, mime)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      }
      if (intent.resolveActivity(activity.packageManager) == null) {
        invoke.reject("系统里没有能打开该文件的应用")
        return
      }
      activity.startActivity(intent)
      invoke.resolve()
    } catch (e: Exception) {
      Log.e(TAG, "openSavedFile failed", e)
      invoke.reject(e.message ?: "无法打开该文件")
    }
  }

  /**
   * 用系统文件管理器打开接收目录（次要入口）。
   *
   * 有 SAF 授权时直接开 tree URI；没有授权时只开恒存在的「内部存储」根，
   * **绝不拼子目录 URI**——那是白屏的根源。打不开就明确报错，让用户留在应用内查看。
   */
  @Command
  fun openFmRoot(invoke: Invoke) {
    try {
      val tree = try {
        invoke.getArgs().optString("tree", "")
      } catch (_: Exception) {
        ""
      }
      if (tree.isNotBlank()) {
        try {
          val treeUri = Uri.parse(tree)
          val doc = DocumentsContract.buildDocumentUriUsingTree(
            treeUri,
            DocumentsContract.getTreeDocumentId(treeUri),
          )
          if (documentExists(doc) && openDocumentDir(doc)) {
            invoke.resolve()
            return
          }
        } catch (e: Exception) {
          Log.w(TAG, "open tree dir failed: ${e.message}")
        }
      }
      val root = Uri.parse("content://com.android.externalstorage.documents/root/primary")
      if (openDocumentDir(root)) {
        invoke.resolve()
        return
      }
      invoke.reject("系统文件管理器打不开该目录，请直接在应用内查看")
    } catch (e: Exception) {
      Log.e(TAG, "openFmRoot failed", e)
      invoke.reject(e.message ?: "无法打开文件管理器")
    }
  }

  /** 解析接收目录：优先 SAF 目录（有授权），否则 MediaStore 的 下载/RunHelp，再退私有目录。 */
  private fun resolveSavedFiles(tree: String): Pair<String, List<SavedRow>> {
    if (tree.isNotBlank()) {
      try {
        val treeUri = Uri.parse(tree)
        val treeId = DocumentsContract.getTreeDocumentId(treeUri)
        val parent = DocumentsContract.buildDocumentUriUsingTree(treeUri, treeId)
        if (documentExists(parent)) {
          val label = queryDisplayName(parent) ?: "已选文件夹"
          return Pair(label, listTreeChildren(treeUri, treeId))
        }
      } catch (e: Exception) {
        Log.w(TAG, "resolve tree for list failed: ${e.message}")
      }
    }
    val media = listDownloadFiles()
    if (media.isNotEmpty()) return Pair("下载/RunHelp", media)
    return Pair("应用私有目录", listPrivateFiles())
  }

  /** 查 SAF 目录下的文件（只列文件，不递归子目录）。 */
  private fun listTreeChildren(treeUri: Uri, treeId: String): List<SavedRow> {
    val rows = ArrayList<SavedRow>()
    val children = DocumentsContract.buildChildDocumentsUriUsingTree(treeUri, treeId)
    activity.contentResolver.query(
      children,
      arrayOf(
        DocumentsContract.Document.COLUMN_DOCUMENT_ID,
        DocumentsContract.Document.COLUMN_DISPLAY_NAME,
        DocumentsContract.Document.COLUMN_SIZE,
        DocumentsContract.Document.COLUMN_LAST_MODIFIED,
        DocumentsContract.Document.COLUMN_MIME_TYPE,
      ),
      null, null, null,
    )?.use { c ->
      while (c.moveToNext() && rows.size < LIST_LIMIT) {
        val mime = c.getString(4) ?: ""
        if (mime == DocumentsContract.Document.MIME_TYPE_DIR) continue
        val id = c.getString(0) ?: continue
        rows.add(
          SavedRow(
            name = c.getString(1) ?: "未命名",
            size = if (c.isNull(2)) 0L else c.getLong(2),
            date = if (c.isNull(3)) 0L else c.getLong(3),
            uri = DocumentsContract.buildDocumentUriUsingTree(treeUri, id),
            mime = mime,
          ),
        )
      }
    }
    return rows
  }

  /** 查 MediaStore 里「下载/RunHelp」的文件（Android 10+ 无需存储权限）。 */
  private fun listDownloadFiles(): List<SavedRow> {
    val rows = ArrayList<SavedRow>()
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
      val resolver = activity.contentResolver
      val projection = arrayOf(
        MediaStore.MediaColumns._ID,
        MediaStore.MediaColumns.DISPLAY_NAME,
        MediaStore.MediaColumns.SIZE,
        MediaStore.MediaColumns.DATE_ADDED,
        MediaStore.MediaColumns.MIME_TYPE,
      )
      val selection = "${MediaStore.MediaColumns.RELATIVE_PATH} LIKE ?"
      val args = arrayOf("Download/RunHelp%")
      val order = "${MediaStore.MediaColumns.DATE_ADDED} DESC"
      try {
        resolver.query(MediaStore.Downloads.EXTERNAL_CONTENT_URI, projection, selection, args, order)?.use { c ->
          while (c.moveToNext() && rows.size < LIST_LIMIT) {
            val id = c.getLong(0)
            rows.add(
              SavedRow(
                name = c.getString(1) ?: "未命名",
                size = if (c.isNull(2)) 0L else c.getLong(2),
                date = if (c.isNull(3)) 0L else c.getLong(3) * 1000L,
                uri = ContentUris.withAppendedId(MediaStore.Downloads.EXTERNAL_CONTENT_URI, id),
                mime = c.getString(4) ?: guessMime(c.getString(1) ?: ""),
              ),
            )
          }
        }
      } catch (e: Exception) {
        Log.w(TAG, "query downloads failed: ${e.message}")
      }
    }
    if (rows.isEmpty()) {
      // 低版本或 MediaStore 尚未收录（刚写入时可能滞后）：直接扫文件系统
      val dir = File(
        android.os.Environment.getExternalStoragePublicDirectory(android.os.Environment.DIRECTORY_DOWNLOADS),
        "RunHelp",
      )
      rows.addAll(listFileDir(dir, "下载/RunHelp"))
    }
    return rows
  }

  /** 应用私有接收目录（SAF/MediaStore 都拿不到内容时的兜底）。 */
  private fun listPrivateFiles(): List<SavedRow> =
    listFileDir(File(activity.filesDir, "sync_receive"), "应用私有目录")

  private fun listFileDir(dir: File, label: String): List<SavedRow> {
    val rows = ArrayList<SavedRow>()
    if (!dir.isDirectory) return rows
    val stack = ArrayDeque<File>()
    stack.addLast(dir)
    while (stack.isNotEmpty() && rows.size < LIST_LIMIT) {
      val cur = stack.removeFirst()
      val children = cur.listFiles() ?: continue
      children.sortByDescending { it.lastModified() }
      for (f in children) {
        if (rows.size >= LIST_LIMIT) break
        if (f.isDirectory) {
          stack.addLast(f)
          continue
        }
        val uri = try {
          FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", f)
        } catch (e: Exception) {
          Log.w(TAG, "file provider uri failed for ${f.name}: ${e.message}")
          continue
        }
        rows.add(
          SavedRow(
            name = f.name,
            size = f.length(),
            date = f.lastModified(),
            uri = uri,
            mime = guessMime(f.name),
          ),
        )
      }
    }
    if (rows.isEmpty()) Log.i(TAG, "no files listed in $label")
    return rows
  }

  /** 该 document URI 在系统里是否真的存在（不存在时 DocumentsUI 会开出白屏）。 */
  private fun documentExists(uri: Uri): Boolean = try {
    activity.contentResolver.query(
      uri, arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID), null, null, null,
    )?.use { it.count > 0 } ?: false
  } catch (e: Exception) {
    Log.w(TAG, "documentExists failed: ${e.message}")
    false
  }

  /**
   * 用 DocumentsUI 打开一个目录 URI；系统里没有能处理它的应用时返回 false 交给上层兜底。
   * 不再「startActivity 没抛异常就算成功」——那正是白屏的来源：应用被拉起来了但显示不出来。
   */
  private fun openDocumentDir(uri: Uri): Boolean {
    val intent = Intent(Intent.ACTION_VIEW).apply {
      setDataAndType(uri, DocumentsContract.Document.MIME_TYPE_DIR)
      addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
      addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
    }
    if (intent.resolveActivity(activity.packageManager) == null) {
      Log.w(TAG, "no activity for $uri")
      return false
    }
    return try {
      activity.startActivity(intent)
      true
    } catch (e: Exception) {
      Log.w(TAG, "openDocumentDir failed: ${e.message}")
      false
    }
  }

  /**
   * 通过 FileProvider 打开应用私有接收目录。
   *
   * 注意：`resource/folder` 这个 MIME 只有少数文件管理器认，HarmonyOS 的安卓兼容层
   * （卓易通等）会直接提示「暂不支持此功能」。所以这里按「文档目录 → resource/folder」
   * 依次尝试，都不行时抛出可读错误，让上层提示改用「保存到下载目录」。
   */
  private fun openPrivateReceiveDir() {
    val dir = File(activity.filesDir, "sync_receive")
    if (!dir.exists()) dir.mkdirs()
    val uri = FileProvider.getUriForFile(activity, "${activity.packageName}.fileprovider", dir)
    for (mime in listOf(DocumentsContract.Document.MIME_TYPE_DIR, "resource/folder")) {
      val intent = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(uri, mime)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      }
      if (intent.resolveActivity(activity.packageManager) == null) continue
      try {
        activity.startActivity(intent)
        return
      } catch (e: Exception) {
        Log.w(TAG, "open private dir with $mime failed: ${e.message}")
      }
    }
    throw IllegalStateException("系统文件管理器无法打开应用私有目录，请开启「自动保存到下载目录」后从「下载/RunHelp」查看")
  }

  /**
   * 打开系统相册选择图片/视频（可混选）。
   *
   * 说明：ACTION_PICK 配 image MIME 只能选图片，ACTION_PICK_IMAGES（系统相册选择器）
   * 的 MIME 也只接受单一类型，二者都无法图片＋视频混选。官方支持混选的只有
   * ACTION_GET_CONTENT 配通配 MIME + EXTRA_MIME_TYPES 限定 image 与 video，
   * 在华为/HarmonyOS 上进入的就是带「图片/视频」分类的图库选择界面。
   * 返回 { uris: [content://...] }，由 resolveUris 复制为真实文件后发送。
   */
  @Command
  fun pickMedia(invoke: Invoke) {
    try {
      // EXTRA_MIME_TYPES 里放具体 MIME 而不是通配符：部分 ROM 只认具体类型，
      // 通配符会被忽略导致只能选图片。仍作为 listMedia 不可用时的兜底入口。
      val intent = Intent(Intent.ACTION_GET_CONTENT).apply {
        addCategory(Intent.CATEGORY_OPENABLE)
        type = "*/*"
        putExtra(
          Intent.EXTRA_MIME_TYPES,
          arrayOf(
            "image/jpeg", "image/png", "image/gif", "image/webp", "image/heic", "image/heif",
            "video/mp4", "video/3gpp", "video/quicktime", "video/webm", "video/x-matroska",
          ),
        )
        putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
      }
      startActivityForResult(invoke, intent, "pickMediaResult")
    } catch (e: Exception) {
      Log.e(TAG, "pickMedia failed", e)
      invoke.reject(e.message ?: "无法打开相册")
    }
  }

  @ActivityCallback
  fun pickMediaResult(invoke: Invoke, result: ActivityResult) {
    handleUriResult(invoke, result)
  }

  /**
   * 系统文件选择器（「文件」入口）。
   *
   * 注意：**不要**传 `EXTRA_INITIAL_URI`。系统的文件选择器（DocumentsUI）在没有
   * initial URI 时会停在「最近」页签，这正是用户想要的入口；之前为了「别停在上次的
   * 下载目录」硬塞了存储根目录，结果每次都要从根目录一层层翻。需要挑目录的场景走
   * [`pickFolder`]（那里才用 initial URI 定位到当前接收目录）。
   */
  @Command
  fun pickFiles(invoke: Invoke) {
    // 起始位置按「最近 → 系统默认」降级，不再强制定位到存储根目录：
    // 用户点「文件」想看的是刚用过的文档，不是一层层翻目录。
    val attempts = listOfNotNull(
      openDocumentIntent(recentDocumentsUri()),
      openDocumentIntent(null),
    )
    var lastErr: Exception? = null
    for (intent in attempts) {
      try {
        startActivityForResult(invoke, intent, "pickFilesResult")
        return
      } catch (e: Exception) {
        lastErr = e
        Log.w(TAG, "pickFiles attempt failed: ${e.message}")
      }
    }
    invoke.reject(lastErr?.message ?: "无法打开文件选择器")
  }

  private fun openDocumentIntent(initial: Uri?): Intent =
    Intent(Intent.ACTION_OPEN_DOCUMENT).apply {
      addCategory(Intent.CATEGORY_OPENABLE)
      type = "*/*"
      putExtra(Intent.EXTRA_ALLOW_MULTIPLE, true)
      if (initial != null) putExtra(DocumentsContract.EXTRA_INITIAL_URI, initial)
    }

  /**
   * 「最近」视图的 URI：content://<externalstorage>/root/primary/recent。
   * 部分 ROM（含鸿蒙兼容层）不认识它，会在 pickFiles 里向上抛出并降级到系统默认位置。
   */
  private fun recentDocumentsUri(): Uri? = try {
    DocumentsContract.buildRecentDocumentsUri("com.android.externalstorage.documents", "primary")
  } catch (_: Exception) {
    null
  }

  @ActivityCallback
  fun pickFilesResult(invoke: Invoke, result: ActivityResult) {
    handleUriResult(invoke, result)
  }

  /** 主存储根目录的 document URI（primary:），常用于把 SAF 选择器定位到根目录。 */
  private fun storageRootDocumentUri(): Uri =
    DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", "primary:")

  /**
   * 选择文件夹（SAF 目录树）。系统 dialog 插件在 Android 上会忽略 directory 选项，
   * 因此文件夹发送必须走这个原生命令。
   * 起点同样是存储根目录，方便一次性选到 DCIM / Download 等任意目录。
   */
  @Command
  fun pickFolder(invoke: Invoke) {
    try {
      // 传了当前接收目录就把选择器定位到那里，省得每次从根目录翻回去
      val initial = try {
        invoke.getArgs().optString("initial", "")
      } catch (_: Exception) {
        ""
      }
      val startUri = if (initial.startsWith("content://")) {
        try {
          val parsed = Uri.parse(initial)
          // EXTRA_INITIAL_URI 更认 document URI；目录树 URI 先换成对应的 document URI
          if (DocumentsContract.isTreeUri(parsed)) {
            val docId = DocumentsContract.getTreeDocumentId(parsed)
            DocumentsContract.buildDocumentUriUsingTree(parsed, docId)
          } else {
            parsed
          }
        } catch (_: Exception) {
          storageRootDocumentUri()
        }
      } else {
        storageRootDocumentUri()
      }
      val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
        addFlags(
          Intent.FLAG_GRANT_READ_URI_PERMISSION or
            Intent.FLAG_GRANT_WRITE_URI_PERMISSION or
            Intent.FLAG_GRANT_PERSISTABLE_URI_PERMISSION or
            Intent.FLAG_GRANT_PREFIX_URI_PERMISSION,
        )
        putExtra(DocumentsContract.EXTRA_INITIAL_URI, startUri)
      }
      startActivityForResult(invoke, intent, "pickFolderResult")
    } catch (e: Exception) {
      Log.e(TAG, "pickFolder failed", e)
      invoke.reject(e.message ?: "无法打开文件夹选择器")
    }
  }

  @ActivityCallback
  fun pickFolderResult(invoke: Invoke, result: ActivityResult) {
    if (result.resultCode != Activity.RESULT_OK) {
      invoke.reject("cancelled")
      return
    }
    val uri = result.data?.data
    if (uri == null) {
      invoke.reject("cancelled")
      return
    }
    try {
      activity.contentResolver.takePersistableUriPermission(
        uri,
        Intent.FLAG_GRANT_READ_URI_PERMISSION or Intent.FLAG_GRANT_WRITE_URI_PERMISSION,
      )
    } catch (e: Exception) {
      Log.w(TAG, "takePersistableUriPermission failed: ${e.message}")
    }
    val out = JSObject()
    out.put("uri", uri.toString())
    // 目录树 URI 本身查不到 DISPLAY_NAME，转成 document URI 再查
    val displayName = try {
      val docId = DocumentsContract.getTreeDocumentId(uri)
      queryDisplayName(DocumentsContract.buildDocumentUriUsingTree(uri, docId))
    } catch (e: Exception) {
      Log.w(TAG, "query tree display name failed: ${e.message}")
      null
    }
    out.put("name", displayName ?: uri.lastPathSegment ?: "已选文件夹")
    invoke.resolveObject(out)
  }

  /**
   * 把应用私有目录里的文件导出到用户选定的 SAF 文件夹（接收目录可修改）。
   * 扁平复制：同名冲突时自动加时间戳后缀，避免覆盖用户已有文件。
   */
  @Command
  fun exportToDir(invoke: Invoke) {
    try {
      val args = invoke.getArgs()
      val uriStr = args.getString("uri")
      if (uriStr.isNullOrBlank()) {
        invoke.reject("未设置导出目录")
        return
      }
      val treeUri = Uri.parse(uriStr)
      val paths = if (args.has("paths")) JSArray(args.getJSONArray("paths").toString()) else JSArray()
      val treeId = DocumentsContract.getTreeDocumentId(treeUri)
      val parent = DocumentsContract.buildDocumentUriUsingTree(treeUri, treeId)
      val resolver = activity.contentResolver
      var count = 0
      for (i in 0 until paths.length()) {
        val p = paths.getString(i) ?: continue
        val src = File(p)
        if (!src.isFile) continue
        val mime = guessMime(src.name)
        var doc: Uri? = try {
          DocumentsContract.createDocument(resolver, parent, mime, src.name)
        } catch (e: Exception) {
          Log.w(TAG, "createDocument failed: ${e.message}")
          null
        }
        if (doc == null) {
          val alt = "${src.nameWithoutExtension}_${System.currentTimeMillis()}${if (src.extension.isNotBlank()) "." + src.extension else ""}"
          doc = try {
            DocumentsContract.createDocument(resolver, parent, mime, alt)
          } catch (e: Exception) {
            Log.w(TAG, "createDocument retry failed: ${e.message}")
            null
          }
        }
        if (doc == null) continue
        val out = resolver.openOutputStream(doc) ?: continue
        out.use { target -> src.inputStream().use { input -> input.copyTo(target) } }
        count++
      }
      val res = JSObject()
      res.put("count", count)
      invoke.resolveObject(res)
    } catch (e: Exception) {
      Log.e(TAG, "exportToDir failed", e)
      invoke.reject(e.message ?: "导出到所选文件夹失败")
    }
  }

  /**
   * 把文件保存到系统「下载/RunHelp」目录（MediaStore，Android 10+ 无需权限）。
   * 用于接收目录不可见（如卓易通等兼容层）时让用户能在文件管理器里找到文件。
   */
  @Command
  fun saveToDownloads(invoke: Invoke) {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
      invoke.reject("系统版本过低，无法写入公共下载目录")
      return
    }
    try {
      val args = invoke.getArgs()
      val paths = if (args.has("paths")) JSArray(args.getJSONArray("paths").toString()) else JSArray()
      val resolver = activity.contentResolver
      var count = 0
      for (i in 0 until paths.length()) {
        val p = paths.getString(i) ?: continue
        val src = File(p)
        if (!src.isFile) continue
        val values = ContentValues().apply {
          put(MediaStore.Downloads.DISPLAY_NAME, src.name)
          put(MediaStore.Downloads.MIME_TYPE, guessMime(src.name))
          put(MediaStore.Downloads.RELATIVE_PATH, "Download/RunHelp")
          put(MediaStore.Downloads.IS_PENDING, 1)
        }
        val uri = resolver.insert(MediaStore.Downloads.EXTERNAL_CONTENT_URI, values) ?: continue
        val out = resolver.openOutputStream(uri) ?: continue
        out.use { target -> src.inputStream().use { input -> input.copyTo(target) } }
        resolver.update(uri, ContentValues().apply {
          put(MediaStore.Downloads.IS_PENDING, 0)
        }, null, null)
        count++
      }
      val res = JSObject()
      res.put("count", count)
      res.put("dir", "Download/RunHelp")
      invoke.resolveObject(res)
    } catch (e: Exception) {
      Log.e(TAG, "saveToDownloads failed", e)
      invoke.reject(e.message ?: "保存到下载目录失败")
    }
  }

  private fun handleUriResult(invoke: Invoke, result: ActivityResult) {
    if (result.resultCode != Activity.RESULT_OK) {
      invoke.reject("cancelled")
      return
    }
    val data = result.data
    val uris = JSArray()
    if (data != null) {
      val clip = data.clipData
      if (clip != null) {
        for (i in 0 until clip.itemCount) uris.put(clip.getItemAt(i).uri.toString())
      } else {
        data.data?.let { uris.put(it.toString()) }
      }
    }
    if (uris.length() == 0) {
      invoke.reject("cancelled")
      return
    }
    val out = JSObject()
    out.put("uris", uris)
    invoke.resolveObject(out)
  }

  private fun guessMime(name: String): String = when (name.substringAfterLast('.', "").lowercase()) {
    "jpg", "jpeg" -> "image/jpeg"
    "png" -> "image/png"
    "gif" -> "image/gif"
    "webp" -> "image/webp"
    "heic" -> "image/heic"
    "mp4" -> "video/mp4"
    "mov" -> "video/quicktime"
    "mkv" -> "video/x-matroska"
    "mp3" -> "audio/mpeg"
    "m4a" -> "audio/mp4"
    "wav" -> "audio/wav"
    "pdf" -> "application/pdf"
    "apk" -> "application/vnd.android.package-archive"
    "zip" -> "application/zip"
    "txt" -> "text/plain"
    "json" -> "application/json"
    "md" -> "text/markdown"
    else -> "application/octet-stream"
  }

  /**
   * 把 Android 文件选择器返回的 content:// URI 复制为应用私有目录下的真实文件，
   * 返回 [{ path, name }] 数组供 Rust 端按普通文件路径读取并发送。
   * 桌面端不会调用此方法；仅移动端 pickFiles 使用。
   */
  @Command
  fun resolveUris(invoke: Invoke) {
    try {
      val args = invoke.getArgs()
      val uris = if (args.has("uris")) JSArray(args.getJSONArray("uris").toString()) else JSArray()
      val outDir = File(activity.cacheDir, "sync_send")
      if (!outDir.exists()) outDir.mkdirs()
      val result = JSArray()
      for (i in 0 until uris.length()) {
        val uriStr = uris.getString(i) ?: continue
        val uri = Uri.parse(uriStr)
        val name = queryDisplayName(uri) ?: "file_${System.currentTimeMillis()}"
        if (isDirectoryUri(uri)) {
          collectDirectory(uri, name, outDir, result)
        } else {
          val dest = uniqueFile(outDir, name)
          activity.contentResolver.openInputStream(uri).use { input ->
            FileOutputStream(dest).use { output ->
              if (input == null) throw Exception("无法打开: $uriStr")
              input.copyTo(output)
            }
          }
          val obj = JSObject()
          obj.put("path", dest.absolutePath)
          obj.put("name", name)
          obj.put("rel", name)
          result.put(obj)
        }
      }
      invoke.resolveObject(result)
    } catch (e: Exception) {
      Log.e(TAG, "resolveUris failed", e)
      invoke.reject(e.message ?: "读取所选文件失败")
    }
  }

  private fun isDirectoryUri(uri: Uri): Boolean {
    if (DocumentsContract.isTreeUri(uri)) return true
    val cursor = activity.contentResolver.query(uri, null, null, null, null) ?: return false
    cursor.use {
      if (!it.moveToFirst()) return false
      val idx = it.getColumnIndex(DocumentsContract.Document.COLUMN_MIME_TYPE)
      if (idx < 0) return false
      return it.getString(idx) == DocumentsContract.Document.MIME_TYPE_DIR
    }
  }

  /** 递归遍历 Android 目录树 URI，复制文件到 cacheDir 并保留相对路径。 */
  private fun collectDirectory(uri: Uri, dirName: String, outDir: File, result: JSArray) {
    val resolver = activity.contentResolver
    val treeId = try {
      DocumentsContract.getTreeDocumentId(uri)
    } catch (_: Exception) {
      DocumentsContract.getDocumentId(uri)
    }
    val childrenUri = DocumentsContract.buildChildDocumentsUriUsingTree(uri, treeId)
    val projection = arrayOf(
      DocumentsContract.Document.COLUMN_DOCUMENT_ID,
      DocumentsContract.Document.COLUMN_DISPLAY_NAME,
      DocumentsContract.Document.COLUMN_MIME_TYPE,
    )
    val cursor = resolver.query(childrenUri, projection, null, null, null) ?: return
    val rows = mutableListOf<Triple<String, String, String>>()
    cursor.use {
      while (it.moveToNext()) {
        val id = it.getString(it.getColumnIndex(DocumentsContract.Document.COLUMN_DOCUMENT_ID)) ?: continue
        val name = it.getString(it.getColumnIndex(DocumentsContract.Document.COLUMN_DISPLAY_NAME)) ?: continue
        val mime = it.getString(it.getColumnIndex(DocumentsContract.Document.COLUMN_MIME_TYPE)) ?: continue
        rows.add(Triple(id, name, mime))
      }
    }
    for ((id, name, mime) in rows) {
      val rel = "$dirName/$name"
      if (mime == DocumentsContract.Document.MIME_TYPE_DIR) {
        collectDirectory(DocumentsContract.buildDocumentUriUsingTree(uri, id), rel, outDir, result)
      } else {
        val dest = File(outDir, rel.replace('/', File.separatorChar))
        dest.parentFile?.mkdirs()
        val docUri = DocumentsContract.buildDocumentUriUsingTree(uri, id)
        resolver.openInputStream(docUri).use { input ->
          FileOutputStream(dest).use { output ->
            if (input == null) throw Exception("无法读取: $rel")
            input.copyTo(output)
          }
        }
        val obj = JSObject()
        obj.put("path", dest.absolutePath)
        obj.put("name", name)
        obj.put("rel", rel)
        result.put(obj)
      }
    }
  }

  private fun queryDisplayName(uri: Uri): String? {
    var cursor: Cursor? = null
    try {
      cursor = activity.contentResolver.query(uri, null, null, null, null)
      if (cursor != null && cursor.moveToFirst()) {
        val idx = cursor.getColumnIndex(OpenableColumns.DISPLAY_NAME)
        if (idx >= 0) {
          val name = cursor.getString(idx)
          if (!name.isNullOrBlank()) return name
        }
      }
    } catch (_: Exception) {
    } finally {
      cursor?.close()
    }
    // 回退：从 URI 末段取名字
    return uri.lastPathSegment?.substringAfterLast('/')?.takeIf { it.isNotBlank() }
  }

  private fun uniqueFile(dir: File, name: String): File {
    var candidate = File(dir, name)
    var n = 1
    val ext = name.substringAfterLast('.', "")
    val base = if (ext.isNotBlank() && name.contains('.')) name.substringBeforeLast('.') else name
    while (candidate.exists()) {
      val suffix = if (ext.isNotBlank()) ".$ext" else ""
      candidate = File(dir, "${base}_${n++}$suffix")
    }
    return candidate
  }
}

/**
 * 前台服务：持有常驻通知 + PARTIAL_WAKE_LOCK + WIFI_MODE_FULL_LOW_LATENCY 锁。
 * 只要同步在跑，这个服务就让进程保持在前台优先级，系统不会冻结其网络线程，
 * UDP 广播与 TCP 监听得以持续工作。
 */
class SyncKeepAliveService : Service() {

  companion object {
    const val ACTION_START = "com.runhelp.desktop.keepalive.START"
    const val ACTION_STOP = "com.runhelp.desktop.keepalive.STOP"
    private const val CHANNEL_ID = "runhelp_sync_keepalive"
    private const val NOTIFICATION_ID = 47800
  }

  private var wakeLock: PowerManager.WakeLock? = null
  private var wifiLock: WifiManager.WifiLock? = null

  override fun onBind(intent: Intent?): IBinder? = null

  override fun onCreate() {
    super.onCreate()
    createChannel()
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    when (intent?.action) {
      ACTION_STOP -> {
        releaseLocks()
        stopForeground(STOP_FOREGROUND_REMOVE)
        stopSelf()
        return START_NOT_STICKY
      }
      else -> {
        startForeground(NOTIFICATION_ID, buildNotification())
        acquireLocks()
        Log.i(TAG, "foreground service running with locks")
      }
    }
    // 被系统回收后自动重建，保证同步期间保活不被打断。
    return START_STICKY
  }

  private fun createChannel() {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      val mgr = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
      if (mgr.getNotificationChannel(CHANNEL_ID) == null) {
        val channel = NotificationChannel(
          CHANNEL_ID,
          "数据同步后台服务",
          NotificationManager.IMPORTANCE_LOW,
        ).apply { description = "保持局域网同步在后台持续运行" }
        mgr.createNotificationChannel(channel)
      }
    }
  }

  private fun buildNotification(): Notification {
    val launchIntent = packageManager.getLaunchIntentForPackage(packageName)?.apply {
      action = Intent.ACTION_MAIN
      addCategory(Intent.CATEGORY_LAUNCHER)
      flags = Intent.FLAG_ACTIVITY_SINGLE_TOP or Intent.FLAG_ACTIVITY_CLEAR_TOP
    }
    val pending = PendingIntent.getActivity(
      this, 0, launchIntent,
      PendingIntent.FLAG_UPDATE_CURRENT or
        (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.M) PendingIntent.FLAG_IMMUTABLE else 0),
    )
    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setSmallIcon(android.R.drawable.stat_sys_upload_done)
      .setContentTitle("RunHelp 数据同步运行中")
      .setContentText("正在后台保持局域网设备发现与接收")
      .setContentIntent(pending)
      .setOngoing(true)
      .setPriority(NotificationCompat.PRIORITY_LOW)
      .build()
  }

  private fun acquireLocks() {
    if (wakeLock == null) {
      val pm = getSystemService(Context.POWER_SERVICE) as PowerManager
      wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "$packageName:sync").apply {
        setReferenceCounted(false)
        acquire()
      }
    }
    if (wifiLock == null && Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) {
      @Suppress("DEPRECATION")
      val wm = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      wifiLock = wm.createWifiLock(WifiManager.WIFI_MODE_FULL_HIGH_PERF, "RunHelpSync").apply {
        setReferenceCounted(false)
        acquire()
      }
    }
  }

  private fun releaseLocks() {
    wakeLock?.let { if (it.isHeld) it.release() }
    wakeLock = null
    wifiLock?.let { if (it.isHeld) it.release() }
    wifiLock = null
  }

  override fun onDestroy() {
    releaseLocks()
    super.onDestroy()
  }
}
