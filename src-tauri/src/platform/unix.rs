//! macOS / Linux 可编译骨架。
//! 这里只保证项目能在对应平台编译、基本命令能跑起来；
//! 真正支持某平台时再按发行版/终端差异细化，并补上对应测试。

use std::path::{Path, PathBuf};
use std::process::Command as ProcCommand;

use super::{CommandSpec, ShellKind};

/// POSIX 约定中断为 128 + SIGINT(2) = 130。
pub const STOPPED_EXIT_CODE: i32 = 130;

/// 非 Windows 上真实可用的 shell 选项。
pub fn available_shells() -> &'static [ShellKind] {
    &[ShellKind::Bash, ShellKind::Sh]
}

/// 老数据里常见的 powershell / cmd 在这里统一回落到 sh。
pub fn default_shell() -> ShellKind {
    ShellKind::Sh
}

/// 解析 shell 字符串；未知值与存量 Windows 值都回落到 sh。
pub fn parse_shell(raw: &str) -> ShellKind {
    match raw.to_lowercase().as_str() {
        "bash" | "git-bash" => ShellKind::Bash,
        "sh" | "zsh" | "fish" => ShellKind::Sh,
        _ => ShellKind::Sh,
    }
}

/// 非 Windows 上 bash 由系统提供，不需要额外定位。
pub fn resolve_bash() -> Option<PathBuf> {
    Some(PathBuf::from("bash"))
}

/// POSIX 上 bash/sh 由系统提供；缺失时 spawn 会给标准错误，这里不再额外判定。
pub fn shell_installed(_shell: &str) -> bool {
    true
}

/// 非 Windows 骨架：bash 分支与 Windows 一致，其余落到 sh -c。
pub fn build_command_spec(shell: &str, cmd: &str) -> CommandSpec {
    match shell.to_lowercase().as_str() {
        "bash" | "git-bash" => CommandSpec::new("bash", vec!["-lc".into(), cmd.into()]),
        _ => CommandSpec::new("sh", vec!["-c".into(), cmd.into()]),
    }
}

/// POSIX 无需隐藏控制台窗口，空实现。
pub fn apply_no_window_flags(_proc_cmd: &mut ProcCommand) {}

/// 目前只给单个进程发 TERM。
/// 真正支持时需改用进程组（setpgid + kill -TERM -PGID）才能递归终止。
pub fn kill_tree(pid: u32) {
    let _ = ProcCommand::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
}

/// 冒号分隔；venv 用 bin 目录，无 WindowsApps 过滤。
pub fn build_python_path(existing: &str, python_dir: &Path) -> String {
    let dir_str = python_dir.to_string_lossy().to_string();
    let mut parts: Vec<String> = existing
        .split(':')
        .filter(|s| {
            if s.is_empty() {
                return false;
            }
            let low = s.to_lowercase();
            if low.contains("workbuddy") && (low.contains("python") || low.contains("binaries")) {
                return false;
            }
            !s.eq_ignore_ascii_case(&dir_str)
        })
        .map(|s| s.to_string())
        .collect();
    let bin_dir = python_dir.join("bin").to_string_lossy().to_string();
    parts.insert(0, dir_str);
    parts.insert(1, bin_dir);
    parts.join(":")
}

/// 只有 Windows GUI 需要刷新 PATH 快照，空实现。
pub fn refresh_path_env() {}

/// 用户主目录：HOME 优先，兼容 USERPROFILE，再回退临时目录。
pub fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(std::env::temp_dir)
}
