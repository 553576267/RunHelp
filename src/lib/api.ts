import { invoke } from '@tauri-apps/api/core';

// 注意：Tauri 2 命令的顶层参数在 JS 侧必须用 camelCase
// （Rust 端 snake_case 参数由框架自动转换）。
// 但事件 payload 与嵌套结构体字段走 serde 默认命名，仍为 snake_case。
function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  return invoke<T>(cmd, args);
}

export const api = {
  listProjects: () => call<import('../types').Project[]>('list_projects'),
  openFolder: (path: string) => call<void>('open_folder', { path }),
  openExternalUrl: (url: string) => call<void>('open_external_url', { url }),
  resolveProjectPath: (base: string, relative: string) =>
    call<string>('resolve_project_path', { base, relative }),
  createProject: (name: string, path: string, type: string, description: string) =>
    call<number>('create_project', { name, path, kind: type, description }),
  updateProject: (id: number, name: string, path: string, type: string, description: string) =>
    call<void>('update_project', { id, name, path, kind: type, description }),
  deleteProject: (id: number) => call<void>('delete_project', { id }),
  listDeletedProjects: () => call<import('../types').Project[]>('list_deleted_projects'),
  restoreProject: (id: number) => call<void>('restore_project', { id }),
  permanentDeleteProject: (id: number) => call<void>('permanent_delete_project', { id }),
  emptyRecycleBin: () => call<number>('empty_recycle_bin'),

  listCommands: (projectId: number) =>
    call<import('../types').Command[]>('list_commands', { projectId }),
  saveCommand: (c: Partial<import('../types').Command> & { project_id: number }) =>
    call<number>('save_command', {
      id: c.id ?? null,
      projectId: c.project_id,
      name: c.name,
      cmd: c.cmd,
      cwd: c.cwd ?? '',
      // 留空时由 platform 层按当前平台取默认值，不在前端写死
      shell: c.shell ?? '',
      kind: c.type ?? 'custom',
      timeoutMs: c.timeout_ms ?? 0,
      sortOrder: c.sort_order ?? 0,
    }),
  deleteCommand: (id: number) => call<void>('delete_command', { id }),
  runCommand: (id: number) => call<string>('run_command', { commandId: id }),
  runAdhoc: (cmd: string, cwd: string, shell: string) =>
    call<string>('run_adhoc', { cmd, cwd, shell }),
  stopRun: (runId: string) => call<void>('stop_run', { runId }),
  pauseRun: (runId: string) => call<void>('pause_run', { runId }),
  resumeRun: (runId: string) => call<void>('resume_run', { runId }),
  listHistory: (limit: number) =>
    call<import('../types').RunHistory[]>('list_history', { limit }),
  listRunning: () => call<import('../types').RunningInfo[]>('list_running'),
  clearHistory: () => call<void>('clear_history'),

  listPipelines: (projectId: number) =>
    call<import('../types').Pipeline[]>('list_pipelines', { projectId }),
  savePipeline: (id: number | null, projectId: number, name: string, description: string) =>
    call<number>('save_pipeline', { id, projectId, name, description }),
  deletePipeline: (id: number) => call<void>('delete_pipeline', { id }),
  listPipelineSteps: (pipelineId: number) =>
    call<import('../types').PipelineStep[]>('list_pipeline_steps', { pipelineId }),
  savePipelineSteps: (
    pipelineId: number,
    // steps 是嵌套结构体，字段走 serde 默认命名，保持 snake_case
    steps: { command_id: number | null; step_type: string; delay_ms: number; on_error: string }[]
  ) => call<void>('save_pipeline_steps', { pipelineId, steps }),
  runPipeline: (id: number) => call<string>('run_pipeline', { pipelineId: id }),

  listScripts: () => call<import('../types').Script[]>('list_scripts'),
  getScript: (id: number) => call<import('../types').Script>('get_script', { id }),
  saveScript: (s: Partial<import('../types').Script> & { name: string }) =>
    call<number>('save_script', {
      id: s.id ?? null,
      name: s.name,
      language: s.language ?? 'python',
      sourceType: s.source_type ?? 'builtin',
      content: s.content ?? '',
      filePath: s.file_path ?? '',
    }),
  deleteScript: (id: number) => call<void>('delete_script', { id }),

  getSettings: () => call<Record<string, string>>('get_settings'),
  getShellOptions: () => call<import('../types').ShellOption[]>('get_shell_options'),
  getDefaultShell: () => call<string>('get_default_shell'),
  setSetting: (key: string, value: string) => call<void>('set_setting', { key, value }),
  setWindowTheme: (theme: string) => call<void>('set_window_theme', { theme }),
  dataDir: () => call<string>('data_dir'),
  getDbLocation: () => call<{ current: string }>('get_db_location'),

  // 数据库备份与恢复
  getDefaultBackupDir: () => call<string>('get_default_backup_dir'),
  listBackups: () => call<import('../types').BackupInfo[]>('list_backups'),
  restoreBackup: (name: string) => call<string>('restore_backup', { name }),
  runBackup: () => call<string>('run_backup'),

  listProfiles: (projectId: number) =>
    call<import('../types').CompressionProfile[]>('list_profiles', { projectId }),
  saveProfile: (
    id: number | null,
    projectId: number,
    name: string,
    paramsJson: string,
    scriptId: number | null
  ) => call<number>('save_profile', { id, projectId, name, paramsJson, scriptId }),
  deleteProfile: (id: number) => call<void>('delete_profile', { id }),
  runCompression: (profileId: number, inputPaths: string[]) =>
    call<string>('run_compression', { profileId, inputPaths }),
  stopCompression: (runId: string) => call<void>('stop_compression', { runId }),

  // 数据同步
  syncStart: () => call<void>('sync_start'),
  syncStop: () => call<void>('sync_stop'),
  syncStatus: () => call<import('../types').SyncStatus>('sync_status'),
  syncSetDeviceName: (name: string) => call<void>('sync_set_device_name', { name }),
  syncSetReceiveDir: (path: string) => call<void>('sync_set_receive_dir', { path }),
  syncListDevices: () => call<import('../types').SyncDevice[]>('sync_list_devices'),
  syncListPending: () => call<import('../types').SyncRequestEvent[]>('sync_list_pending'),
  // 待接收清单明细：请求事件只带数量，明细在打开接收页时按需拉取
  syncPendingItems: (transferId: string) =>
    call<{ name: string; size: number }[]>('sync_pending_items', { transferId }),
  syncPairDevice: (ip: string) =>
    call<import('../types').SyncDevice>('sync_pair_device', { ip }),
  syncSetFavorite: (fingerprint: string, favorite: boolean) =>
    call<void>('sync_set_favorite', { fingerprint, favorite }),
  syncSetAlias: (fingerprint: string, alias: string) =>
    call<void>('sync_set_alias', { fingerprint, alias }),
  syncSetAutoAccept: (fingerprint: string, enabled: boolean) =>
    call<void>('sync_set_auto_accept', { fingerprint, enabled }),
  syncRemoveDevice: (fingerprint: string) => call<void>('sync_remove_device', { fingerprint }),
  syncRespond: (requestId: string, accepted: boolean) =>
    call<void>('sync_respond', { requestId, accepted }),
  syncRespondSelected: (requestId: string, indices: number[]) =>
    call<void>('sync_respond_selected', { requestId, indices }),
  syncCancelSend: (ip: string, transferId: string) =>
    call<void>('sync_cancel_send', { ip, transferId }),
  syncSendText: (ip: string, text: string, kind: string, transferId: string) =>
    call<void>('sync_send_text', { ip, text, kind, transferId }),
  syncSendFiles: (ip: string, paths: string[], transferId: string, names?: string[]) =>
    call<void>('sync_send_files', { ip, paths, names: names ?? null, transferId }),
  syncListHistory: (limit: number) =>
    call<import('../types').SyncTransfer[]>('sync_list_history', { limit }),
  syncClearHistory: () => call<void>('sync_clear_history'),
  syncRegenerateIdentity: () => call<string>('sync_regenerate_identity'),
  syncLogResult: (payload: Record<string, string>) => call<void>('sync_log_result', { payload }),
  syncProbeRecent: (limit: number) => call<number>('sync_probe_recent', { limit }),
  appIsMobile: () => call<boolean>('app_is_mobile'),
  // 移动端把 content:// URI 解析为可读文件路径；桌面端原样返回。
  mobileResolveUris: (uris: string[]) =>
    call<{ path: string; name: string; rel?: string }[]>('mobile_resolve_uris', { uris }),
  mobilePickMedia: () => call<string[]>('mobile_pick_media'),
  mobileListMedia: (limit: number) =>
    call<{ uri: string; name: string; size: number; mime: string; video: boolean; date: number; thumb: string }[]>(
      'mobile_list_media',
      { limit },
    ),
  mobileListRecentFiles: (limit: number) =>
    call<{ uri: string; name: string; size: number; mime: string; date: number }[]>(
      'mobile_list_recent_files',
      { limit },
    ),
  mobileListApps: (excludeSystem: boolean, excludeNonLaunchable: boolean) =>
    call<{ package: string; label: string; version: string; size: number; system: boolean; icon: string }[]>(
      'mobile_list_apps',
      { excludeSystem, excludeNonLaunchable },
    ),
  mobilePrepareApks: (packages: string[]) =>
    call<{ path: string; name: string }[]>('mobile_prepare_apks', { packages }),
  mobilePickFiles: () => call<string[]>('mobile_pick_files'),
  mobilePickFolder: (initial?: string) =>
    call<{ uri: string; name: string }>('mobile_pick_folder', { initial: initial ?? '' }),
  mobileExportToDir: (uri: string, paths: string[]) =>
    call<number>('mobile_export_to_dir', { uri, paths }),
  mobileSaveToDownloads: (paths: string[]) => call<number>('mobile_save_to_downloads', { paths }),
  mobileSetTheme: (theme: string) => call<void>('mobile_set_theme', { theme }),
  mobileStartKeepalive: () => call<void>('mobile_start_keepalive'),
  mobileStopKeepalive: () => call<void>('mobile_stop_keepalive'),
  mobileOpenReceiveDir: () => call<void>('mobile_open_receive_dir'),
  /* 应用内「接收目录」：列文件 + 打开单个文件；系统文件管理器只作为次要入口开存储根 */
  mobileListSavedFiles: (tree: string) =>
    call<{ dir: string; items: { name: string; size: number; date: number; uri: string; mime: string }[] }>(
      'mobile_list_saved_files',
      { tree },
    ),
  mobileOpenSavedFile: (uri: string, mime: string) =>
    call<void>('mobile_open_saved_file', { uri, mime }),
  mobileOpenFmRoot: (tree: string) => call<void>('mobile_open_fm_root', { tree }),
};
