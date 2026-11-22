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
    let lang = crate::i18n::lang(state);
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
    if trimmed.is_empty() || crate::platform::is_plain_python_token(trimmed) {
        // 未配置具体解释器时不输出提示，避免每次运行都刷一行
        return String::new();
    }
    let p = Path::new(trimmed);
    if !p.exists() {
        return crate::i18n::trf(
            &lang,
            "━━━ 【RunHelp】解释器: {path} (路径不存在) ━━━",
            &[("path", trimmed)],
        );
    }
    let dir = if p.is_file() {
        match p.parent() {
            Some(d) => d,
            None => return String::new(),
        }
    } else {
        p
    };
    // 过滤父进程注入的干扰路径、前置指定解释器目录（平台差异见 platform 层）
    let existing = std::env::var("PATH").unwrap_or_default();
    let new_path = crate::platform::build_python_path(&existing, dir);
    proc_cmd.env("PATH", new_path);
    crate::i18n::trf(&lang, "━━━ 【RunHelp】解释器: {path} ━━━", &[("path", trimmed)])
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
    title: &str,
) -> Result<(i32, String), String> {
    let lang = crate::i18n::lang(state);
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

    // 隐藏子进程控制台窗口（Windows 专属行为，实现见 platform 层）
    crate::platform::apply_no_window_flags(proc_cmd);

    let mut child = match proc_cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            // spawn 失败会绕过函数末尾的统一清理，这里主动清掉运行态，
            // 避免前端留下永远不会消失的"幽灵角标"和卡住的暂停标志。
            state.runtime.running_meta.lock().unwrap().remove(run_id);
            return Err(crate::i18n::trf(
                &lang,
                "启动进程失败: {error}",
                &[("error", &e.to_string())],
            ));
        }
    };
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
            Err(e) => {
                // 同上：异常返回前清理运行态，保证角标不残留
                state.runtime.children.lock().unwrap().remove(run_id);
                state.runtime.running_meta.lock().unwrap().remove(run_id);
                return Err(crate::i18n::trf(
                    &lang,
                    "等待进程失败: {error}",
                    &[("error", &e.to_string())],
                ));
            }
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

    // 用户主动中断（关控制台 / Ctrl+C）归为 stopped 而非 failed，判定见 platform 层
    let user_abort = crate::platform::is_user_abort(code);
    let status_text = if timed_out {
        "timeout"
    } else if was_stopped || user_abort {
        "stopped"
    } else if code == 0 {
        "success"
    } else {
        "failed"
    };
    if timed_out {
        emit_line(&app, run_id, "stderr", crate::i18n::tr(&lang, "命令超时，已强制终止"));
    }
    let stderr_text = stderr_buf.lock().unwrap().clone();
    let duration_ms = start.elapsed().as_millis() as u64;
    let _ = app.emit(
        "run://end",
        RunEndEvent {
            run_id: run_id.to_string(),
            title: title.to_string(),
            status: status_text.to_string(),
            exit_code: code,
            stderr: stderr_text,
            duration_ms,
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
    label: &str,
) -> Result<(i32, String), String> {
    let lang = crate::i18n::lang(state);
    // 只消费抽象后的命令描述，具体 shell 拼接由 platform 层按平台实现。
    let spec = crate::platform::build_command_spec(shell, cmd);
    let mut proc_cmd = ProcCommand::new(spec.program);
    proc_cmd.args(&spec.args);
    if !cwd.is_empty() {
        let p = Path::new(cwd);
        if p.exists() {
            proc_cmd.current_dir(p);
        }
    }
    // 统一输出模板：开始横幅（带命令描述）→ 解释器行 → 命令描述 + 回显 → 结果描述
    let title = if label.trim().is_empty() {
        crate::i18n::tr(&lang, "━━━ 【RunHelp】开始执行 ━━━")
    } else {
        crate::i18n::trf(&lang, "━━━ 【RunHelp】开始执行：{label} ━━━", &[("label", label)])
    };
    emit_line(&app, run_id, "info", title);
    let diag = apply_python_path(state, &mut proc_cmd);
    if !diag.is_empty() {
        emit_line(&app, run_id, "info", diag);
    }
    emit_line(&app, run_id, "info", crate::i18n::tr(&lang, "【RunHelp】执行命令："));
    emit_line(&app, run_id, "stdout", format!("$ {}", cmd));
    emit_line(&app, run_id, "info", crate::i18n::tr(&lang, "【RunHelp】执行结果："));
    execute(app, state, run_id, &mut proc_cmd, timeout_ms, label)
}

/// 兼容转发：平台实现已收敛到 platform 层，保留本入口让既有调用方
/// （backup / command / compress / lib）无需改动。新代码可直接调 platform::kill_tree。
pub fn kill_tree(pid: u32) {
    crate::platform::kill_tree(pid);
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
