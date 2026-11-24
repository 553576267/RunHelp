//! 平台适配层：集中所有与操作系统相关的差异。
//!
//! 目录结构：
//! - `mod.rs`：平台无关的统一接口、类型定义与外部程序调用封装；
//! - `windows.rs`：Windows 具体实现（现有行为的唯一来源，移植时不要随意改动）；
//! - `unix.rs`：macOS / Linux 可编译骨架，真正支持时按平台细化。
//!
//! 设计约定：
//! - 业务代码不写 `#[cfg(...)]`，一律调用本模块的统一接口；
//! - Windows 分支与改造前逐字等价，保证现有行为不变；
//! - 新增能力时先在这里定义接口，再由各平台文件分别实现。

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use windows::{
    apply_no_window_flags, available_shells, build_command_spec, build_python_path, default_shell,
    home_dir, kill_tree, parse_shell, refresh_path_env, resolve_bash, shell_installed,
    STOPPED_EXIT_CODE,
};

#[cfg(not(windows))]
mod unix;
#[cfg(not(windows))]
pub use unix::{
    apply_no_window_flags, available_shells, build_command_spec, build_python_path, default_shell,
    home_dir, kill_tree, parse_shell, refresh_path_env, resolve_bash, shell_installed,
    STOPPED_EXIT_CODE,
};

use std::io;
use std::path::Path;

/// shell 的抽象类型。前端选项、默认值映射与命令构造都只认这个枚举，
/// 平台差异（哪些 shell 可用、老值如何映射）由 windows/unix 实现决定。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellKind {
    PowerShell,
    Cmd,
    Bash,
    Sh,
}

impl ShellKind {
    /// 解析数据库/前端传来的字符串；未知值回落到当前平台的默认 shell。
    pub fn parse(raw: &str) -> Self {
        parse_shell(raw)
    }

    /// 落库与 IPC 使用的稳定标识，与平台无关。
    pub fn key(self) -> &'static str {
        match self {
            ShellKind::PowerShell => "powershell",
            ShellKind::Cmd => "cmd",
            ShellKind::Bash => "bash",
            ShellKind::Sh => "sh",
        }
    }

    /// 供 i18n 字典查询的标签 key（专有名词也走字典，便于后续改文案）。
    pub fn label_key(self) -> &'static str {
        match self {
            ShellKind::PowerShell => "PowerShell",
            ShellKind::Cmd => "CMD",
            ShellKind::Bash => "Git Bash",
            ShellKind::Sh => "Shell (sh)",
        }
    }
}

/// 抽象后的命令描述：`run_shell` 只消费这个结构，不关心各平台怎么拼参数。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub program: String,
    pub args: Vec<String>,
}

impl CommandSpec {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
        }
    }
}

/// 兼容旧调用方：仍返回 (program, args) 元组。
/// 新代码请直接用 `build_command_spec` 消费 `CommandSpec`。
pub fn shell_command(shell: &str, cmd: &str) -> (String, Vec<String>) {
    let spec = build_command_spec(shell, cmd);
    (spec.program, spec.args)
}

/// 把存量数据里的老值（如非 Windows 上的 powershell）映射成当前平台可用项。
pub fn normalize_shell(raw: &str) -> ShellKind {
    let kind = ShellKind::parse(raw);
    if available_shells().contains(&kind) {
        kind
    } else {
        default_shell()
    }
}

/// 该退出码是否表示"用户主动中断"（关控制台 / Ctrl+C），应归为 stopped 而非 failed。
pub fn is_user_abort(code: i32) -> bool {
    code == STOPPED_EXIT_CODE
}

/// 配置值是 `python` / `python.exe` 这类未指定具体解释器的占位符。
pub fn is_plain_python_token(s: &str) -> bool {
    s.eq_ignore_ascii_case("python") || s.eq_ignore_ascii_case("python.exe")
}

/// 从设置值解析要启动的解释器：空白值统一回落到 PATH 里的 `python`。
/// 注意不能只靠 SQL 查询失败兜底——查到空字符串也算"没配置"，
/// 而新的 Rust 在 Windows 上对空 program 会直接报 "program path has no file name"。
pub fn python_program(configured: &str) -> String {
    let trimmed = configured.trim();
    if trimmed.is_empty() {
        "python".to_string()
    } else {
        trimmed.to_string()
    }
}

/// 默认备份目录：坚果云同步目录下的 RunHelpBackups。
/// 这是用户既有约定，不改成 app_data_dir（否则用户会找不到云同步备份）。
pub fn default_backup_dir() -> std::path::PathBuf {
    home_dir().join("Nutstore").join("RunHelpBackups")
}

/// 本机标识，规则与 Python 脚本 backup_db.py 的 safe_hostname() 完全一致。
/// 备份文件名形如 runhelp_<safe_hostname>_<时间>.db，清理时只认这个前缀。
pub fn safe_hostname() -> String {
    let raw = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let mut name: String = raw
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    while name.starts_with('_') {
        name.remove(0);
    }
    while name.ends_with('_') {
        name.pop();
    }
    if name.is_empty() {
        "pc".to_string()
    } else {
        name
    }
}

/// 本机备份文件名的固定前缀。
pub fn local_backup_prefix() -> String {
    format!("runhelp_{}_", safe_hostname())
}

/// 校验同步接收的相对路径，防止 `..` / 绝对路径穿越到接收目录之外。
/// 统一把 `\` 转成 `/`，拒绝盘符、前导 `/` 与任何 `..` 段。
pub fn safe_relative_path(raw: &str) -> Option<String> {
    let normalized = raw.replace('\\', "/");
    if normalized.starts_with('/') || normalized.contains(':') {
        return None;
    }
    let mut parts: Vec<&str> = Vec::new();
    for seg in normalized.split('/') {
        if seg.is_empty() || seg == "." {
            continue;
        }
        if seg == ".." {
            return None;
        }
        parts.push(seg);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// 用系统文件管理器打开目录。
pub fn open_folder(path: &str) -> io::Result<()> {
    open_path(Path::new(path))
}

/// 打开本地路径（目录或文件），统一走 opener 插件，天然跨平台。
pub fn open_path(path: &Path) -> io::Result<()> {
    tauri_plugin_opener::open_path(path, Option::<&str>::None)
        .map_err(|e| io::Error::other(e.to_string()))
}

/// 用系统默认浏览器打开 URL。
pub fn open_url(url: &str) -> io::Result<()> {
    tauri_plugin_opener::open_url(url, Option::<&str>::None)
        .map_err(|e| io::Error::other(e.to_string()))
}
