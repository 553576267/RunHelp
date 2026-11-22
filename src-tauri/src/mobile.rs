//! 移动端原生能力桥接。
//! 通过 Tauri 自定义插件调用 Kotlin 侧实现：后台保活（前台服务 + WakeLock）。
//! 目前只对 Android 生效；桌面/iOS 编译为空实现，command 签名保持一致，
//! 前端无需做平台判断。

use tauri::{AppHandle, Runtime};

use tauri::plugin::Builder;
#[cfg(target_os = "android")]
use tauri::{plugin::PluginHandle, Manager};

#[cfg(target_os = "android")]
const PLUGIN_IDENTIFIER: &str = "com.runhelp.desktop";

/// 原生插件句柄容器（仅 Android）。
#[cfg(target_os = "android")]
pub struct MobilePlugin<R: Runtime>(pub PluginHandle<R>);

/// 注册插件到 Builder。桌面/iOS 分支为空插件，保持调用点与 command 签名一致。
pub fn init<R: Runtime>() -> tauri::plugin::TauriPlugin<R> {
    #[allow(unused_mut)]
    let mut builder = Builder::new("runhelp-mobile");
    #[cfg(target_os = "android")]
    {
        builder = builder.setup(|app, api| {
            let handle = api.register_android_plugin(PLUGIN_IDENTIFIER, "RunhelpMobilePlugin")?;
            app.manage(MobilePlugin::<R>(handle));
            Ok(())
        });
    }
    builder.build()
}

/// 内部统一实现：sync_start/sync_stop 与 command 共用，签名与平台无关。
fn keep_alive(app: &AppHandle, start: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let cmd = if start { "startKeepAlive" } else { "stopKeepAlive" };
        plugin
            .0
            .run_mobile_plugin::<()>(cmd, ())
            .map_err(|e| format!("调用原生保活失败: {}", e))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, start);
        Ok(())
    }
}

// ---- Tauri commands：全平台签名一致，注册到 invoke_handler ----

#[tauri::command]
pub fn mobile_start_keepalive(app: AppHandle) -> Result<(), String> {
    keep_alive(&app, true)
}

#[tauri::command]
pub fn mobile_stop_keepalive(app: AppHandle) -> Result<(), String> {
    keep_alive(&app, false)
}

/// 解析 Android content:// URI 为可读的真实文件路径（复制到应用私有目录）。
/// 桌面端原样返回传入路径（本就是绝对路径），移动端调用 Kotlin 插件转换。
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct ResolvedUri {
    pub path: String,
    pub name: String,
}

#[tauri::command]
pub fn mobile_resolve_uris(app: AppHandle, uris: Vec<String>) -> Result<Vec<ResolvedUri>, String> {
    #[cfg(target_os = "android")]
    {
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        plugin
            .0
            .run_mobile_plugin::<Vec<ResolvedUri>>("resolveUris", serde_json::json!({ "uris": uris }))
            .map_err(|e| format!("读取所选文件失败: {}", e))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(uris
            .into_iter()
            .map(|p| {
                let name = std::path::Path::new(&p)
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "file".to_string());
                ResolvedUri { path: p, name }
            })
            .collect())
    }
}
