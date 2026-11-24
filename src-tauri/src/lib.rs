pub mod commands;
pub mod db;
pub mod i18n;
pub mod mobile;
pub mod models;
pub mod platform;
pub mod process;
pub mod state;
pub mod sync;

use tauri::{Manager, RunEvent};

// 供前端判断当前平台：移动端只渲染数据同步页，桌面端渲染完整工作台。
// 用编译期 cfg 判定，比运行时 UA 判断可靠且零额外依赖。
#[tauri::command]
fn app_is_mobile() -> bool {
    cfg!(mobile)
}

// 移动端（Android/iOS）需要此宏生成 JNI/AppDelegate 入口；桌面端无副作用。
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // 刷新 PATH 环境快照（Windows GUI 启动的已知问题，实现见 platform 层）
    platform::refresh_path_env();

    // 单实例插件必须是第一个注册的插件：重复双击图标时不再新开进程，
    // 而是把已经运行的窗口显示 / 取消最小化 / 置前。
    #[allow(unused_mut)] // 仅桌面端会重新赋值（单实例插件）
    let mut builder = tauri::Builder::default();
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.unminimize();
                let _ = win.set_focus();
            }
        }));
    }

    #[allow(unused_mut)] // 仅桌面端会重新赋值（window-state 插件）
    let mut builder = builder
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        // 移动端原生桥（前台保活 / URI 适配）；桌面端插件仅注册空壳 command。
        .plugin(mobile::init());
    // 记住窗口大小/位置/最大化状态（仅桌面端有窗口概念；window-state 插件在移动端不可用）。
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_window_state::Builder::default().build());
    }

    builder
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法获取数据目录: {}", e))?;
            let db_path = db::resolve_db_path(&data_dir);
            db::init_db(&db_path)?;
            // 设备身份：自签证书持久化，指纹重启不变；可信指纹从库加载
            let (identity, trusted_fps) = {
                let conn = rusqlite::Connection::open(&db_path)
                    .map_err(|e| format!("打开数据库失败: {}", e))?;
                let id = sync::identity::load_or_create(&data_dir)
                    .map_err(|e| format!("加载设备身份失败: {}", e))?;
                let fps: std::collections::HashSet<String> = conn
                    .prepare("SELECT fingerprint FROM sync_devices WHERE trusted=1 AND fingerprint<>''")
                    .ok()
                    .and_then(|mut s| {
                        s.query_map([], |r| r.get::<_, String>(0)).ok().map(|rows| {
                            rows.filter_map(|r| r.ok()).collect()
                        })
                    })
                    .unwrap_or_default();
                (id, fps)
            };
            let sync_runtime = std::sync::Arc::new(sync::SyncRuntime::new(identity, trusted_fps));
            let app_state = state::AppState::new(db_path, sync_runtime);
            *app_state
                .app_handle
                .lock()
                .map_err(|e| format!("初始化同步事件句柄失败: {}", e))? = Some(app.handle().clone());
            state::AppState::set_global(app_state.clone());
            app.manage(app_state);

            // 窗口初始隐藏（tauri.conf.json visible:false），等 window-state 恢复完
            // 上次的尺寸/位置后再显示，避免先显示默认小窗口再变大造成闪烁。
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.show();
                let _ = win.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_is_mobile,
            mobile::mobile_start_keepalive,
            mobile::mobile_stop_keepalive,
            mobile::mobile_resolve_uris,
            mobile::mobile_set_theme,
            mobile::mobile_open_receive_dir,
            mobile::mobile_list_saved_files,
            mobile::mobile_open_saved_file,
            mobile::mobile_open_fm_root,
            mobile::mobile_pick_media,
            mobile::mobile_list_media,
            mobile::mobile_list_recent_files,
            mobile::mobile_list_apps,
            mobile::mobile_prepare_apks,
            mobile::mobile_pick_files,
            mobile::mobile_pick_folder,
            mobile::mobile_export_to_dir,
            mobile::mobile_save_to_downloads,
            sync::commands::sync_log_result,
            sync::commands::sync_probe_recent,
            commands::project::list_projects,
            commands::project::resolve_project_path,
            commands::project::open_folder,
            commands::project::open_external_url,
            commands::project::create_project,
            commands::project::update_project,
            commands::project::delete_project,
            commands::project::list_deleted_projects,
            commands::project::restore_project,
            commands::project::permanent_delete_project,
            commands::project::empty_recycle_bin,
            commands::command::list_commands,
            commands::command::save_command,
            commands::command::delete_command,
            commands::command::run_command,
            commands::command::run_adhoc,
            commands::command::stop_run,
            commands::command::pause_run,
            commands::command::resume_run,
            commands::command::list_history,
            commands::command::list_running,
            commands::command::clear_history,
            commands::pipeline::list_pipelines,
            commands::pipeline::save_pipeline,
            commands::pipeline::delete_pipeline,
            commands::pipeline::list_pipeline_steps,
            commands::pipeline::save_pipeline_steps,
            commands::pipeline::run_pipeline,
            commands::script::list_scripts,
            commands::script::get_script,
            commands::script::save_script,
            commands::script::delete_script,
            commands::settings::get_settings,
            commands::settings::get_shell_options,
            commands::settings::get_default_shell,
            commands::settings::set_setting,
            commands::settings::set_window_theme,
            commands::settings::data_dir,
            commands::settings::get_db_location,
            commands::settings::get_default_backup_dir,
            commands::settings::list_backups,
            commands::settings::restore_backup,
            commands::backup::run_backup,
            commands::compress::list_profiles,
            commands::compress::save_profile,
            commands::compress::delete_profile,
            commands::compress::run_compression,
            commands::compress::stop_compression,
            sync::commands::sync_start,
            sync::commands::sync_stop,
            sync::commands::sync_status,
            sync::commands::sync_list_pending,
            sync::commands::sync_pending_items,
            sync::commands::sync_set_device_name,
            sync::commands::sync_set_receive_dir,
            sync::commands::sync_list_devices,
            sync::commands::sync_pair_device,
            sync::commands::sync_set_favorite,
            sync::commands::sync_set_alias,
            sync::commands::sync_set_auto_accept,
            sync::commands::sync_remove_device,
            sync::commands::sync_respond,
            sync::commands::sync_respond_selected,
            sync::commands::sync_cancel_send,
            sync::commands::sync_send_text,
            sync::commands::sync_send_files,
            sync::commands::sync_list_history,
            sync::commands::sync_clear_history,
            sync::commands::sync_regenerate_identity,
        ])
        .build(tauri::generate_context!())
        .expect("构建 Tauri 应用失败")
        // 应用退出时清理所有由本软件启动的子进程，避免关闭软件后 python
        // server 之类常驻服务残留、占用端口（下次运行报端口被占用）。
        .run(|app, event| {
            if matches!(event, RunEvent::Exit | RunEvent::ExitRequested { .. }) {
                if let Some(app_state) = app.try_state::<state::AppState>() {
                    let pids: Vec<u32> = app_state
                        .runtime
                        .children
                        .lock()
                        .map(|m| m.values().copied().collect())
                        .unwrap_or_default();
                    for pid in pids {
                        process::kill_tree(pid);
                    }
                }
            }
        });
}
