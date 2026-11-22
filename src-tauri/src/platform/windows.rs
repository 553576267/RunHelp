//! Windows 平台实现：现有行为的唯一来源。
//! 本文件内的命令拼接、PATH 过滤、隐藏窗口等逻辑与改造前逐字等价，
//! 除非明确要改变 Windows 行为，否则移植其他平台时不要动这里。

use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use super::{CommandSpec, ShellKind};

/// 用户主动终止任务时写入历史的退出码（STATUS_CONTROL_C_EXIT）。
pub const STOPPED_EXIT_CODE: i32 = -1073741510; // 0xC000013A

/// Windows 上真实可用的 shell 选项。
pub fn available_shells() -> &'static [ShellKind] {
    &[ShellKind::PowerShell, ShellKind::Cmd, ShellKind::Bash]
}

/// 存量数据缺省值：保持原来的 PowerShell。
pub fn default_shell() -> ShellKind {
    ShellKind::PowerShell
}

/// 解析 shell 字符串；未知值仍按原逻辑回落到 PowerShell。
pub fn parse_shell(raw: &str) -> ShellKind {
    match raw.to_lowercase().as_str() {
        "cmd" => ShellKind::Cmd,
        "bash" | "git-bash" => ShellKind::Bash,
        _ => ShellKind::PowerShell,
    }
}

/// 把 shell + 命令文本转换成抽象命令描述。
/// cmd 的 chcp 65001 与 PowerShell 的 UTF-8 注入是中文不乱码的关键，不能删。
pub fn build_command_spec(shell: &str, cmd: &str) -> CommandSpec {
    match shell.to_lowercase().as_str() {
        "cmd" => CommandSpec::new(
            "cmd.exe",
            vec!["/C".into(), format!("chcp 65001 >nul && {}", cmd)],
        ),
        "bash" | "git-bash" => CommandSpec::new("bash", vec!["-lc".into(), cmd.into()]),
        _ => CommandSpec::new(
            "powershell.exe",
            vec![
                "-NoProfile".into(),
                "-ExecutionPolicy".into(),
                "Bypass".into(),
                "-Command".into(),
                format!(
                    "$OutputEncoding = [Console]::InputEncoding = [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new(); {}",
                    cmd
                ),
            ],
        ),
    }
}

/// GUI 应用启动子进程时隐藏控制台窗口。不隐藏会弹黑窗，
/// 用户关掉它等于向进程组发 CTRL_CLOSE，任务会被误判为失败。
pub fn apply_no_window_flags(proc_cmd: &mut ProcCommand) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    proc_cmd.creation_flags(CREATE_NO_WINDOW);
}

/// 递归终止进程树：taskkill /T /F。
pub fn kill_tree(pid: u32) {
    let _ = ProcCommand::new("taskkill")
        .args(["/PID", &pid.to_string(), "/T", "/F"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// 把指定解释器目录前置到 PATH，并过滤父进程注入的干扰路径。
/// 分号分隔、Scripts 目录、WindowsApps 过滤均为 Windows 特化行为。
pub fn build_python_path(existing: &str, python_dir: &Path) -> String {
    let dir_str = python_dir.to_string_lossy().to_string();
    let mut parts: Vec<String> = existing
        .split(';')
        .filter(|s| {
            if s.is_empty() {
                return false;
            }
            let low = s.to_lowercase();
            if low.contains("workbuddy") && (low.contains("python") || low.contains("binaries")) {
                return false;
            }
            if low.contains("windowsapps") && low.contains("python") {
                return false;
            }
            !s.eq_ignore_ascii_case(&dir_str)
        })
        .map(|s| s.to_string())
        .collect();
    let scripts_dir = python_dir.join("Scripts").to_string_lossy().to_string();
    parts.insert(0, dir_str);
    parts.insert(1, scripts_dir);
    parts.join(";")
}

/// 启动时刷新 PATH 环境快照（从资源管理器启动 GUI 时的已知问题）。
pub fn refresh_path_env() {
    if let Ok(path) = std::env::var("PATH") {
        std::env::set_var("PATH", path);
    }
}

/// 用户主目录：USERPROFILE 在 Windows 恒存在，回退 HOME，再回退临时目录。
pub fn home_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(std::env::temp_dir)
}
