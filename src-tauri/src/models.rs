use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub path: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub description: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Command {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub cmd: String,
    pub cwd: String,
    pub shell: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub timeout_ms: i64,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pipeline {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStep {
    pub id: i64,
    pub pipeline_id: i64,
    pub command_id: Option<i64>,
    pub step_type: String,
    pub delay_ms: i64,
    pub on_error: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineStepDetail {
    pub id: i64,
    pub command_id: Option<i64>,
    pub command_name: String,
    pub step_type: String,
    pub delay_ms: i64,
    pub on_error: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Script {
    pub id: i64,
    pub name: String,
    pub language: String,
    pub source_type: String,
    pub content: String,
    pub file_path: String,
    pub version: i64,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompressionProfile {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub params_json: String,
    pub script_id: Option<i64>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunHistory {
    pub id: i64,
    pub project_id: Option<i64>,
    pub target: String,
    pub target_id: Option<i64>,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub exit_code: i64,
    pub status: String,
    pub log_path: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogEvent {
    pub run_id: String,
    pub stream: String,
    pub line: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct RunEndEvent {
    pub run_id: String,
    pub status: String,
    pub exit_code: i32,
    // 进程结束时把捕获到的 stderr 一并带回，防止流式日志丢失时看不到错误
    pub stderr: String,
}

/// 运行中的任务元数据（展示用），key 为可用于 stop_run 的 run_id。
#[derive(Debug, Clone)]
pub struct RunningMeta {
    pub target: String,
    pub started_at: String,
}

/// 暴露给前端的"运行中任务"信息。
#[derive(Debug, Clone, Serialize)]
pub struct RunningInfo {
    pub run_id: String,
    pub target: String,
    pub started_at: String,
    pub pid: u32,
    /// 当前是否处于暂停请求状态（供全局任务面板显示 暂停/继续）
    pub paused: bool,
}
