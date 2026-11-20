pub mod commands;
pub mod db;
pub mod models;
pub mod process;
pub mod state;

use tauri::{Manager, RunEvent};

pub fn run() {
    // 借鉴 Projects Runner / fix-path-env：Windows 下 GUI 应用从资源管理器或
    // 父进程启动时，std::process::Command 对 PATH 的继承有已知 bug（拿到的是
    // 陈旧的进程快照）。启动时把 PATH 显式读出来再写回，强制刷新环境快照，
    // 保证后续 spawn 的子进程看到的是当前真实的 PATH。
    #[cfg(windows)]
    if let Ok(path) = std::env::var("PATH") {
        std::env::set_var("PATH", path);
    }

    // 单实例插件必须是第一个注册的插件：重复双击图标时不再新开进程，
    // 而是把已经运行的窗口显示 / 取消最小化 / 置前。
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

    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| format!("无法获取数据目录: {}", e))?;
            let db_path = db::resolve_db_path(&data_dir);
            db::init_db(&db_path)?;
            app.manage(state::AppState::new(db_path));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::project::list_projects,
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
