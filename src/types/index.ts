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
  stream: 'stdout' | 'stderr' | 'info';
  line: string;
}

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

export interface SyncStatus {
  device_id: string;
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
