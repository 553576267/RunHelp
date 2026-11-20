use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command as ProcCommand, Stdio};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

use crate::models::{LogEvent, RunEndEvent};
use crate::state::AppState;

/// 容错 UTF-8 流式解码器（借鉴 Projects Runner）。
/// 子进程输出可能跨 read 边界切断一个多字节字符，或混入非 UTF-8 字节
/// （如 Windows 中文控制台的 GBK）。这里逐段解码：完整字符正常输出，
/// 非法字节用 U+FFFD 替换后继续，绝不因为坏字节中断——避免整段日志被吞。
#[derive(Default)]
struct Utf8StreamDecoder {
    pending: Vec<u8>,
}

impl Utf8StreamDecoder {
    fn push(&mut self, bytes: &[u8]) -> String {
        self.decode(bytes, false)
    }

    fn finish(&mut self) -> String {
        self.decode(&[], true)
    }

    fn decode(&mut self, bytes: &[u8], end_of_stream: bool) -> String {
        self.pending.extend_from_slice(bytes);
        let mut output = String::new();
        let mut cursor = 0;

        while cursor < self.pending.len() {
            match std::str::from_utf8(&self.pending[cursor..]) {
                Ok(valid) => {
                    output.push_str(valid);
                    cursor = self.pending.len();
                }
                Err(error) => {
                    let valid_end = cursor + error.valid_up_to();
                    if valid_end > cursor {
                        output.push_str(
                            std::str::from_utf8(&self.pending[cursor..valid_end])
                                .expect("UTF-8 valid prefix"),
                        );
                        cursor = valid_end;
                    }
                    if let Some(error_len) = error.error_len() {
                        output.push('\u{FFFD}');
                        cursor += error_len;
                    } else if end_of_stream {
                        output.push('\u{FFFD}');
                        cursor = self.pending.len();
                    } else {
                        // 字符可能在下一个 read 才完整，先留在 pending
                        break;
                    }
                }
            }
        }

        if cursor > 0 {
            self.pending.drain(..cursor);
        }
        output
    }
}

/// 按 shell 类型构建命令。
/// cmd 分支保留 chcp 65001；powershell 用 -NoProfile -ExecutionPolicy Bypass，
/// 并显式把 Input/Output/OutputEncoding 设为 UTF-8，避免中文输出乱码。
fn shell_command(shell: &str, cmd: &str) -> (String, Vec<String>) {
    match shell.to_lowercase().as_str() {
        "cmd" => (
            "cmd.exe".into(),
            vec!["/C".into(), format!("chcp 65001 >nul && {}", cmd)],
        ),
        "bash" | "git-bash" => ("bash".into(), vec!["-lc".into(), cmd.into()]),
        _ => (
            "powershell.exe".into(),
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

fn emit_line(app: &AppHandle, run_id: &str, stream: &str, line: String) {
    let _ = app.emit(
        "run://log",
        LogEvent {
            run_id: run_id.to_string(),
            stream: stream.to_string(),
            line,
        },
    );
}

/// 读取设置里的 python_path，若它是具体解释器文件，则把其所在目录前置到 PATH。
/// 这样子进程里的 `python` / `pip` / `uvicorn` 都解析到用户指定的环境，
/// 避免 App 继承父进程（如 WorkBuddy）PATH 时用错解释器（典型报错：No module named 'fastapi'）。
fn apply_python_path(state: &AppState, proc_cmd: &mut ProcCommand) -> String {
    let python_path = state
        .conn()
        .ok()
        .and_then(|conn| {
            conn.query_row(
                "SELECT value FROM settings WHERE key='python_path'",
                [],
                |r| r.get::<_, String>(0),
            )
            .ok()
        })
        .unwrap_or_default();
    let trimmed = python_path.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("python")
        || trimmed.eq_ignore_ascii_case("python.exe")
    {
        return "[诊断] python_path 未配置具体路径，使用系统 PATH".to_string();
    }
    let p = Path::new(trimmed);
    if !p.exists() {
        return format!("[诊断] python_path 指向的文件不存在: {}", trimmed);
    }
    let dir = if p.is_file() {
        match p.parent() {
            Some(d) => d,
            None => return "[诊断] 无法确定 python 目录".into(),
        }
    } else {
        p
    };
    let dir_str = dir.to_string_lossy().to_string();
    let existing = std::env::var("PATH").unwrap_or_default();
    // 过滤掉父进程（如 WorkBuddy）注入的干扰路径，保留用户正常环境变量，
    // 再把指定 python 目录提到最前。
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
    let scripts_dir = dir.join("Scripts").to_string_lossy().to_string();
    parts.insert(0, dir_str.clone());
    parts.insert(1, scripts_dir.clone());
    let resolved = parts
        .iter()
        .map(|d| Path::new(d).join("python.exe"))
        .find(|p| p.exists())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|| "未找到 python.exe".to_string());
    let new_path = parts.join(";");
    proc_cmd.env("PATH", new_path);
    format!(
        "[诊断] 指定解释器={}；PATH 首目录={}；python 解析为={}",
        trimmed, dir_str, resolved
    )
}

/// 字节级无损读取一个输出流：按 \n 切块，用 Utf8StreamDecoder 容错解码后 emit。
/// 关键：绝不因非法字节 break（旧代码 BufReader::lines() 遇 GBK 中文会中断，
/// 导致整段 stderr 消失）。同时把每行追加到 `collect` 里供结束事件兜底。
fn spawn_reader<R: Read + Send + 'static>(
    reader: R,
    app: AppHandle,
    run_id: String,
    stream: &'static str,
    collect: Option<std::sync::Arc<std::sync::Mutex<String>>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let mut reader = BufReader::new(reader);
        let mut decoder = Utf8StreamDecoder::default();
        loop {
            let mut bytes = Vec::with_capacity(8192);
            match reader.by_ref().take(65536).read_until(b'\n', &mut bytes) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let decoded = decoder.push(&bytes);
            for raw in decoded.split_inclusive('\n') {
                if raw.is_empty() {
                    continue;
                }
                let line = raw.trim_end_matches(['\r', '\n']).to_string();
                emit_line(&app, &run_id, stream, line.clone());
                if let Some(buf) = &collect {
                    let mut buf = buf.lock().unwrap();
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(&line);
                }
            }
        }
        let tail = decoder.finish();
        if !tail.is_empty() {
            emit_line(&app, &run_id, stream, tail);
        }
    })
}

pub fn execute(
    app: AppHandle,
    state: &AppState,
    run_id: &str,
    proc_cmd: &mut ProcCommand,
    timeout_ms: i64,
) -> Result<(i32, String), String> {
    proc_cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .stdin(Stdio::null())
        // 输出走管道时 Python 默认全缓冲，print 不会即时可见；强制无缓冲。
        // PYTHONIOENCODING 保证 Python 向管道输出 UTF-8（不依赖控制台代码页），
        // 避免中文在子进程里被编码成 GBK 后被 UTF-8 解码器当成坏字节。
        .env("PYTHONUNBUFFERED", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env("PYTHONIOENCODING", "utf-8");

    // Windows 下 GUI 应用启动控制台子进程会弹出一个黑窗口。用户关掉它等于
    // 向整个进程组发 CTRL_CLOSE 信号，会把 python 服务一起杀掉（退出码
    // -1073741510 = 0xC000013A），既误导又影响使用。CREATE_NO_WINDOW 让
    // 子进程无控制台静默运行，stdout/stderr 仍走上面的管道捕获到界面。
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        proc_cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let diag = apply_python_path(state, proc_cmd);
    emit_line(&app, run_id, "info", diag);

    let mut child = proc_cmd
        .spawn()
        .map_err(|e| format!("启动进程失败: {}", e))?;
    let pid = child.id();
    state
        .runtime
        .children
        .lock()
        .unwrap()
        .insert(run_id.to_string(), pid);

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let app_out = app.clone();
    let rid_out = run_id.to_string();
    let app_err = app.clone();
    let rid_err = run_id.to_string();

    // 收集 stderr 全文，进程结束时随 run://end 带回前端兜底。
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stderr_buf_thread = stderr_buf.clone();

    let out_thread = stdout.map(|out| {
        spawn_reader(
            out,
            app_out,
            rid_out,
            "stdout",
            None,
        )
    });
    let err_thread = stderr.map(|err| {
        spawn_reader(
            err,
            app_err,
            rid_err,
            "stderr",
            Some(stderr_buf_thread),
        )
    });

    let start = Instant::now();
    let mut timed_out = false;
    let final_code: i32 = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s.code().unwrap_or(1),
            Ok(None) => {
                if timeout_ms > 0 && start.elapsed().as_millis() as i64 >= timeout_ms {
                    timed_out = true;
                    kill_tree(pid);
                    let _ = child.wait();
                    break 124;
                }
                std::thread::sleep(Duration::from_millis(120));
            }
            Err(e) => return Err(format!("等待进程失败: {}", e)),
        }
    };

    if let Some(t) = out_thread {
        let _ = t.join();
    }
    if let Some(t) = err_thread {
        let _ = t.join();
    }
    state.runtime.children.lock().unwrap().remove(run_id);
    state.runtime.running_meta.lock().unwrap().remove(run_id);
    // 用户主动终止的 run 直接判为 stopped，避免结束事件把它覆盖成 failed。
    let was_stopped = state
        .runtime
        .stopped
        .lock()
        .unwrap()
        .remove(run_id);
    state.runtime.history_ids.lock().unwrap().remove(run_id);
    let code = final_code;

    // 0xC000013A (STATUS_CONTROL_C_EXIT)：控制台窗口被关闭 / Ctrl+C 中断，
    // 这不是程序失败，而是用户主动结束了常驻服务，归为 stopped。
    #[cfg(windows)]
    let ctrl_close = code == -1073741510; // STATUS_CONTROL_C_EXIT (0xC000013A)
    #[cfg(not(windows))]
    let ctrl_close = code == 130;
    let status_text = if timed_out {
        "timeout"
    } else if was_stopped || ctrl_close {
        "stopped"
    } else if code == 0 {
        "success"
    } else {
        "failed"
    };
    if timed_out {
        emit_line(&app, run_id, "stderr", "命令超时，已强制终止".into());
    }
    let stderr_text = stderr_buf.lock().unwrap().clone();
    let _ = app.emit(
        "run://end",
        RunEndEvent {
            run_id: run_id.to_string(),
            status: status_text.to_string(),
            exit_code: code,
            stderr: stderr_text,
        },
    );

    Ok((code, status_text.to_string()))
}

pub fn run_shell(
    app: AppHandle,
    state: &AppState,
    run_id: &str,
    cmd: &str,
    cwd: &str,
    shell: &str,
    timeout_ms: i64,
) -> Result<(i32, String), String> {
    let (program, args) = shell_command(shell, cmd);
    let mut proc_cmd = ProcCommand::new(program);
    proc_cmd.args(&args);
    if !cwd.is_empty() {
        let p = Path::new(cwd);
        if p.exists() {
            proc_cmd.current_dir(p);
        }
    }
    emit_line(&app, run_id, "stdout", format!("$ {}", cmd));
    execute(app, state, run_id, &mut proc_cmd, timeout_ms)
}

pub fn kill_tree(pid: u32) {
    #[cfg(windows)]
    {
        let _ = ProcCommand::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(not(windows))]
    {
        use std::process::Command as C;
        let _ = C::new("kill").arg("-TERM").arg(pid.to_string()).status();
    }
}

pub fn wait_if_paused(state: &AppState, run_id: &str) {
    let runtime = state.runtime.clone();
    let rid = run_id.to_string();
    loop {
        let paused = runtime
            .paused
            .lock()
            .unwrap()
            .get(&rid)
            .map(|f| f.load(Ordering::SeqCst))
            .unwrap_or(false);
        if !paused {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
}
