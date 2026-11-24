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

/// 切换移动端主题：同步更新 WebView data-theme 与 Android 系统栏颜色。
#[tauri::command]
pub fn mobile_set_theme(app: AppHandle, theme: String) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        plugin
            .0
            .run_mobile_plugin::<()>("setTheme", serde_json::json!({ "theme": theme }))
            .map_err(|e| format!("设置系统栏颜色失败: {}", e))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, theme);
        Ok(())
    }
}

/// 打开接收目录：移动端用系统文件管理器/分享查看应用私有接收目录。
#[tauri::command]
pub fn mobile_open_receive_dir(app: AppHandle) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        plugin
            .0
            .run_mobile_plugin::<()>("openReceiveDir", ())
            .map_err(|e| format!("打开接收目录失败: {}", e))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Ok(())
    }
}

/// 打开接收文件的落点目录：
/// - 传了 SAF 目录树 → 打开该文件夹
/// - 否则自动保存开启 → 打开 系统下载/RunHelp
/// - 都没有 → 打开应用私有接收目录
#[tauri::command]
pub fn mobile_open_saved_dir(app: AppHandle, tree: String, auto_export: bool) -> Result<(), String> {
    #[cfg(target_os = "android")]
    {
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        plugin
            .0
            .run_mobile_plugin::<()>(
                "openSavedDir",
                serde_json::json!({ "tree": tree, "autoExport": auto_export }),
            )
            .map_err(|e| format!("打开目录失败: {}", e))
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, tree, auto_export);
        Ok(())
    }
}

/// 打开系统相册选图（Android ACTION_PICK），返回 content:// URI 列表。
#[tauri::command]
pub fn mobile_pick_media(app: AppHandle) -> Result<Vec<String>, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct UriList {
            uris: Vec<String>,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let list = plugin
            .0
            .run_mobile_plugin::<UriList>("pickMedia", ())
            .map_err(|e| e.to_string())?;
        Ok(list.uris)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("当前平台不支持相册选择".to_string())
    }
}

/// 相册里的一条媒体（应用内九宫格混选图片/视频用）
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct MediaItem {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub video: bool,
    /// 拍摄/添加时间（Unix 秒），前端按日期分组用
    #[serde(default)]
    pub date: i64,
    /// base64 data URL 缩略图；生成失败时为空串
    #[serde(default)]
    pub thumb: String,
}

/// 列出相册最近的照片与视频（图片 + 视频混排），供应用内九宫格选择。
#[tauri::command]
pub fn mobile_list_media(app: AppHandle, limit: usize) -> Result<Vec<MediaItem>, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct MediaList {
            items: Vec<MediaItem>,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let list = plugin
            .0
            .run_mobile_plugin::<MediaList>(
                "listMedia",
                serde_json::json!({ "limit": limit as i64 }),
            )
            .map_err(|e| e.to_string())?;
        Ok(list.items)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, limit);
        Err("当前平台不支持相册选择".to_string())
    }
}

/// 已选文件夹（SAF 目录树）。
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct PickedFolder {
    pub uri: String,
    pub name: String,
}

/// 选择文件夹（Android ACTION_OPEN_DOCUMENT_TREE）。
/// `initial` 传当前已选的目录树 URI 时，系统选择器直接定位到该目录；
/// 传空则从存储根目录开始。
#[tauri::command]
pub fn mobile_pick_folder(app: AppHandle, initial: Option<String>) -> Result<PickedFolder, String> {
    #[cfg(target_os = "android")]
    {
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        plugin
            .0
            .run_mobile_plugin::<PickedFolder>(
                "pickFolder",
                serde_json::json!({ "initial": initial.unwrap_or_default() }),
            )
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, initial);
        Err("当前平台不支持文件夹选择".to_string())
    }
}

/// 把已接收文件导出到用户选定的 SAF 文件夹，返回成功数量。
#[tauri::command]
pub fn mobile_export_to_dir(app: AppHandle, uri: String, paths: Vec<String>) -> Result<usize, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct CopyResult {
            count: usize,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let res = plugin
            .0
            .run_mobile_plugin::<CopyResult>(
                "exportToDir",
                serde_json::json!({ "uri": uri, "paths": paths }),
            )
            .map_err(|e| e.to_string())?;
        Ok(res.count)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, uri, paths);
        Err("当前平台不支持导出到文件夹".to_string())
    }
}

/// 把已接收文件保存到系统「下载/RunHelp」，返回成功数量。
#[tauri::command]
pub fn mobile_save_to_downloads(app: AppHandle, paths: Vec<String>) -> Result<usize, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct CopyResult {
            count: usize,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let res = plugin
            .0
            .run_mobile_plugin::<CopyResult>(
                "saveToDownloads",
                serde_json::json!({ "paths": paths }),
            )
            .map_err(|e| e.to_string())?;
        Ok(res.count)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, paths);
        Err("当前平台不支持保存到下载目录".to_string())
    }
}

/// 最近文件（非图片/视频），供移动端「文件」选择器直接定位到最近使用。
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct RecentFile {
    pub uri: String,
    pub name: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub date: i64,
}

/// 已安装应用（发 APK 用）。
#[derive(serde::Serialize, serde::Deserialize, Clone)]
pub struct AppEntry {
    /// 包名（package 是 Rust 关键字，序列化字段名保持不变）
    #[serde(rename = "package")]
    pub pkg: String,
    pub label: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub size: i64,
    #[serde(default)]
    pub system: bool,
    /// base64 data URL 图标
    #[serde(default)]
    pub icon: String,
}

/// 收件：列表查询
#[tauri::command]
pub fn mobile_list_recent_files(app: AppHandle, limit: usize) -> Result<Vec<RecentFile>, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct FileList {
            items: Vec<RecentFile>,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let list = plugin
            .0
            .run_mobile_plugin::<FileList>(
                "listRecentFiles",
                serde_json::json!({ "limit": limit as i64 }),
            )
            .map_err(|e| e.to_string())?;
        Ok(list.items)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, limit);
        Err("当前平台不支持最近文件".to_string())
    }
}

/// 已安装应用列表（excludeSystem / excludeNonLaunchable 与 LocalSend 一致）。
#[tauri::command]
pub fn mobile_list_apps(
    app: AppHandle,
    exclude_system: bool,
    exclude_non_launchable: bool,
) -> Result<Vec<AppEntry>, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct AppList {
            items: Vec<AppEntry>,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let list = plugin
            .0
            .run_mobile_plugin::<AppList>(
                "listApps",
                serde_json::json!({
                    "excludeSystem": exclude_system,
                    "excludeNonLaunchable": exclude_non_launchable,
                }),
            )
            .map_err(|e| e.to_string())?;
        Ok(list.items)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, exclude_system, exclude_non_launchable);
        Err("当前平台不支持应用列表".to_string())
    }
}

/// 把选中应用的 APK 复制到缓存目录，返回可发送路径。
#[tauri::command]
pub fn mobile_prepare_apks(app: AppHandle, packages: Vec<String>) -> Result<Vec<ResolvedUri>, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct ApkList {
            items: Vec<ResolvedUri>,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let list = plugin
            .0
            .run_mobile_plugin::<ApkList>("prepareApks", serde_json::json!({ "packages": packages }))
            .map_err(|e| e.to_string())?;
        Ok(list.items)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (app, packages);
        Err("当前平台不支持导出安装包".to_string())
    }
}

/// 系统文件选择器（浏览全部文件），起点为主存储根目录。
#[tauri::command]
pub fn mobile_pick_files(app: AppHandle) -> Result<Vec<String>, String> {
    #[cfg(target_os = "android")]
    {
        #[derive(serde::Deserialize)]
        struct UriList {
            uris: Vec<String>,
        }
        let plugin = app
            .try_state::<MobilePlugin<tauri::Wry>>()
            .ok_or("移动插件未初始化")?;
        let list = plugin
            .0
            .run_mobile_plugin::<UriList>("pickFiles", ())
            .map_err(|e| e.to_string())?;
        Ok(list.uris)
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = app;
        Err("当前平台不支持文件选择".to_string())
    }
}

#[tauri::command]
pub fn mobile_resolve_uris(app: AppHandle, uris: Vec<String>) -> Result<Vec<ResolvedUri>, String> {    #[cfg(target_os = "android")]
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
