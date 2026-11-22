package com.runhelp.desktop

import android.app.Activity
import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.database.Cursor
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import android.provider.OpenableColumns
import android.util.Log
import androidx.core.app.NotificationCompat
import app.tauri.annotation.Command
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSArray
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import java.io.File
import java.io.FileOutputStream

private const val TAG = "RunHelpMobile"

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
        result.put(obj)
      }
      invoke.resolveObject(result)
    } catch (e: Exception) {
      Log.e(TAG, "resolveUris failed", e)
      invoke.reject(e.message ?: "读取所选文件失败")
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
