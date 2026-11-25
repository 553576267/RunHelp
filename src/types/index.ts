export interface Project {
  id: number;
  name: string;
  path: string;
  type: 'dev' | 'image';
  description: string;
  created_at: string;
  updated_at: string;
}

export interface ShellOption {
  value: string;
  label: string;
}

export interface Command {
  id: number;
  project_id: number;
  name: string;
  cmd: string;
  cwd: string;
  // 具体取值由后端按平台提供（Windows: powershell/cmd/bash；Unix: bash/sh）
  shell: string;
  type: 'install' | 'build' | 'run' | 'custom';
  timeout_ms: number;
  sort_order: number;
}

export interface Pipeline {
  id: number;
  project_id: number;
  name: string;
  description: string;
}

export interface PipelineStep {
  id: number;
  command_id: number | null;
  command_name: string;
  step_type: 'cmd' | 'delay';
  delay_ms: number;
  on_error: 'stop' | 'continue';
  sort_order: number;
}

export interface Script {
  id: number;
  name: string;
  language: string;
  source_type: 'builtin' | 'file';
  content: string;
  file_path: string;
  version: number;
  updated_at: string;
}

export interface CompressionProfile {
  id: number;
  project_id: number;
  name: string;
  params_json: string;
  script_id: number | null;
  created_at: string;
}

export interface RunHistory {
  id: number;
  project_id: number | null;
  target: string;
  target_id: number | null;
  started_at: string;
  ended_at: string | null;
  exit_code: number;
  status: string;
  log_path: string;
}

export interface LogEvent {
  run_id: string;
  /**
   * `warn` 只给「设备状态」这类自身带级别的日志用（后端 level=warn 时下发）：
   * 它们不是进程的 stderr，混进 stderr 会被错误正则染红，看起来像执行失败。
   */
  stream: 'stdout' | 'stderr' | 'info' | 'warn';
  line: string;
}

/** 日志行语义分类：决定配色与「仅错误」筛选，见 lib/logLine.ts */
export type LogLineKind =
  | 'step' // 步骤 N: 执行中
  | 'step-ok' // 步骤 N 完成: 成功
  | 'step-bad' // 步骤 N 完成: 失败
  | 'banner' // ━━━ 开始执行 ━━━
  | 'banner-ok' // ━━━ 执行完成 · 退出码 0 ━━━
  | 'banner-bad' // ━━━ 执行失败 / 超时 ━━━
  | 'banner-stopped' // ━━━ 已停止 ━━━
  | 'meta' // 解释器 / 执行命令：等标签行
  | 'echo' // $ 命令回显
  | 'out' // 进程 stdout / 普通 stderr
  | 'warn' // 警告
  | 'err' // 错误
  | 'info'; // 其它 RunHelp 提示

/** 入队后的日志行：kind 只算一次，渲染直接读 */
export type LogLine = LogEvent & { kind: LogLineKind };

export interface RunEndEvent {
  run_id: string;
  title: string;
  status: string;
  exit_code: number;
  stderr: string;
  duration_ms: number;
}

export interface RunningInfo {
  run_id: string;
  target: string;
  started_at: string;
  pid: number;
  paused: boolean;
  command_id: number | null;
  pipeline_id: number | null;
}

export interface BackupInfo {
  name: string;
  path: string;
  size: number;
  modified: string;
  local: boolean;
}

export interface CompressionParams {
  output_dir: string;
  quality: number;
  max_width: number;
  max_height: number;
  format: 'keep' | 'jpg' | 'png' | 'webp';
  naming: 'keep' | 'prefix' | 'suffix';
  name_prefix: string;
  name_suffix: string;
  strip_metadata: boolean;
  recursive: boolean;
  overwrite: boolean;
  min_size_kb: number;
}

export const defaultCompressionParams: CompressionParams = {
  output_dir: '',
  quality: 80,
  max_width: 0,
  max_height: 0,
  format: 'keep',
  naming: 'suffix',
  name_prefix: '',
  name_suffix: '_compressed',
  strip_metadata: true,
  recursive: true,
  overwrite: false,
  min_size_kb: 0,
};

// ===== 数据同步 =====
export interface SyncDevice {
  id: string;
  name: string;
  platform: string;
  ip: string;
  port: number;
  fingerprint: string;
  last_seen: number;
  trusted: boolean;
  auto_accept: boolean;
  favorite: boolean;
  alias: string;
  last_ip: string;
  online: boolean;
}

/// 设备上下线诊断日志：排查「设备离线/上线来回切」时看判定原因
export interface SyncStateLog {
  ts: number;
  level: string;
  device: string;
  line: string;
}

export interface SyncStatus {  device_id: string;
  device_name: string;
  fingerprint: string;
  fingerprint_short: string;
  receive_dir: string;
  running: boolean;
  port: number;
}

export interface SyncRequestItem {
  name: string;
  size: number;
}

export interface SyncRequestEvent {
  id: string;
  transfer_id: string;
  from: string;
  from_platform: string;
  fingerprint: string;
  trusted: boolean;
  kind: string;
  name: string;
  size: number;
  count: number;
  /** 明细按需拉取（后端事件里恒为空数组），见 sync_pending_items */
  items: SyncRequestItem[];
  /** 后端是否已按设置把窗口置前；前端据此决定是否自动切到同步页 */
  focus?: boolean;
}

export interface SyncReceivedEvent {
  kind: string;
  name: string;
  from: string;
  text: string;
  path: string;
}

export interface SyncProgressEvent {
  transfer_id: string;
  sent: number;
  total: number;
  status: string;
}

export interface SyncTransfer {
  id: number;
  direction: string;
  peer: string;
  kind: string;
  name: string;
  size: number;
  status: string;
  error: string;
  created_at: string;
}
