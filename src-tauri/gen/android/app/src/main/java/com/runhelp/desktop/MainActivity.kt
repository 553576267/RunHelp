package com.runhelp.desktop

import android.graphics.Color
import android.graphics.drawable.ColorDrawable
import android.os.Bundle
import android.view.View
import android.view.ViewGroup
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat

class MainActivity : TauriActivity() {
  /** 柔光浅主背景，与 WebView 页面背景一致；启动阶段也用它，避免先闪一下黑屏。 */
  private val windowBg = Color.parseColor("#E8F0E9")

  private fun barsInsets(insets: WindowInsetsCompat) = insets.getInsets(
    WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout()
  )

  override fun onCreate(savedInstanceState: Bundle?) {
    // Android 15（targetSdk 35+）强制边到边，adjustResize 不再自动收缩页面。
    // 这里只把「状态栏 + 导航栏 / 刘海」写成 decorView 内边距，让页面不压到系统条。
    // 软键盘**不参与**内边距：键盘应当盖在页面之上（底部页签原地不动），
    // 需要避让的只有居中的输入弹层——由 WebView 里的 --kb-height 单独上移。
    // 窗口背景必须早于 super.onCreate 设置，否则会先以默认黑底出现一帧。
    window.setBackgroundDrawable(ColorDrawable(windowBg))
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    val decor = window.decorView
    ViewCompat.setOnApplyWindowInsetsListener(decor) { view, insets ->
      val bars = barsInsets(insets)
      if (view.paddingLeft != bars.left || view.paddingTop != bars.top ||
        view.paddingRight != bars.right || view.paddingBottom != bars.bottom
      ) {
        view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      }
      insets
    }
    ViewCompat.requestApplyInsets(decor)

    // 返回手势 / 返回键：先让 WebView 回退（弹层打开时前端会 pushState，
    // 回退即触发 popstate 关掉弹层），没有可回退的历史才退出应用。
    // 不接管的话侧滑会直接把 Activity 结束掉，弹层还没关应用就没了。
    onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
      override fun handleOnBackPressed() {
        val web = findWebView(decor)
        if (web != null && web.canGoBack()) {
          web.goBack()
          return
        }
        // 交回默认处理（结束 Activity）
        isEnabled = false
        onBackPressedDispatcher.onBackPressed()
      }
    })

    // enableEdgeToEdge 会把系统栏设为透明，需再指定柔光浅背景，
    // 否则华为手势条区域会透出浅色 window 背景形成白边。
    window.statusBarColor = windowBg
    window.navigationBarColor = windowBg
    WindowCompat.getInsetsController(window, window.decorView).apply {
      isAppearanceLightStatusBars = true
      isAppearanceLightNavigationBars = true
    }

    // WebView 在首帧渲染前是黑底，先把它刷成页面背景色。
    // 布局完成与 WebView 创建可能不同步，这里按两个时间点各补一次，代价可忽略。
    paintWebViewBackground(decor, windowBg)
    decor.post { paintWebViewBackground(decor, windowBg) }
    decor.postDelayed({ paintWebViewBackground(decor, windowBg) }, 120)
  }

  override fun onRequestPermissionsResult(
    requestCode: Int,
    permissions: Array<out String>,
    grantResults: IntArray,
  ) {
    super.onRequestPermissionsResult(requestCode, permissions, grantResults)
    // 相册读取权限由原生插件申请，结果统一转交给插件继续查询媒体列表
    RunhelpMobilePlugin.dispatchPermissionResult(requestCode, grantResults)
  }

  /** Tauri 的 WebView 层级不固定，只能遍历视图树找。 */
  private fun findWebView(view: View): WebView? {
    if (view is WebView) return view
    if (view is ViewGroup) {
      for (i in 0 until view.childCount) {
        findWebView(view.getChildAt(i))?.let { return it }
      }
    }
    return null
  }

  /** 递归找到 WebView 并设置其背景色。 */
  private fun paintWebViewBackground(view: View, color: Int) {
    if (view is WebView) {
      view.setBackgroundColor(color)
      return
    }
    if (view is ViewGroup) {
      for (i in 0 until view.childCount) {
        paintWebViewBackground(view.getChildAt(i), color)
      }
    }
  }
}
