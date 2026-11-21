<script setup lang="ts">
import { ref, onMounted, onUnmounted, provide, computed, watch, nextTick } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification';
import { api } from './lib/api';
import { t, setLocale } from './lib/i18n';
import type { Project, LogEvent, RunEndEvent, RunHistory } from './types';
import ProjectSidebar from './components/ProjectSidebar.vue';
import CommandCenter from './components/CommandCenter.vue';
import ImageCompress from './components/ImageCompress.vue';
import SettingsView from './components/SettingsView.vue';

const projects = ref<Project[]>([]);
const selected = ref<Project | null>(null);
type MainTab = 'commands' | 'compress' | 'settings' | 'tasks';
const tab = ref<MainTab>('commands');
const logs = ref<LogEvent[]>([]);
const activeRunId = ref<string>('');
const toast = ref('');
let toastTimer: number | undefined;

// 运行记录（只在「全局任务」页展示）
const history = ref<RunHistory[]>([]);
const running = ref<import('./types').RunningInfo[]>([]);

// 底部运行日志：默认收缩为一条标题栏，需要时再展开
const consoleCollapsed = ref(true);

// 日志面板高度档位：normal(210px) / half(50%) / full(接近全屏，保留标签栏)
type ConsoleSize = 'normal' | 'half' | 'full';
const consoleSize = ref<ConsoleSize>('normal');
// 拖拽调整后的自定义高度（像素），存在时优先生效
const consoleCustomHeight = ref<number | null>(null);
const consolePanel = ref<HTMLElement | null>(null);
const consoleBody = ref<HTMLElement | null>(null);

// 日志自动跟随：新日志到达时若用户正贴着底部则自动滚动，用户上滑后暂停并累计未读
const FOLLOW_THRESHOLD = 24; // 像素：距底部小于该值视为"在底部"
const isFollowing = ref(true);
const unreadLines = ref(0);
function onConsoleScroll() {
  const el = consoleBody.value;
  if (!el) return;
  const distance = el.scrollHeight - el.scrollTop - el.clientHeight;
  isFollowing.value = distance <= FOLLOW_THRESHOLD;
  if (isFollowing.value) unreadLines.value = 0;
}
function scrollConsoleToBottom() {
  const el = consoleBody.value;
  if (!el) return;
  el.scrollTop = el.scrollHeight;
  isFollowing.value = true;
  unreadLines.value = 0;
}
// 折叠/展开、切换档位后重新贴底（clientHeight 变化会改变判定）
function realignFollow() {
  void nextTick(scrollConsoleToBottom);
}
function jumpToLatest() {
  scrollConsoleToBottom();
}

// 全局任务刷新：点击后图标转一圈，纯 CSS 动画，几乎不占资源
const refreshing = ref(false);
async function refreshTasks() {
  if (refreshing.value) return;
  refreshing.value = true;
  const started = Date.now();
  try {
    await loadRunning();
    await loadHistory();
  } finally {
    // 至少转满一圈，避免刷新太快看不到反馈
    const remain = Math.max(0, 600 - (Date.now() - started));
    window.setTimeout(() => (refreshing.value = false), remain);
  }
}

// 品牌区版本号（与 Cargo.toml 版本一致，无需硬编码）
const appVersion = ref('');

// 运行日志：错误筛选
const onlyErrors = ref(false);
const ERROR_RE = /(?:error|exception|traceback|fatal|failed|failure|denied|refused|unhandled|cannot|invalid|not found|timeout|timed out|npm ERR|ERR!|错误|异常|失败|拒绝|无法|不存在|未找到|超时|致命)/i;
function isErrorLog(l: LogEvent): boolean {
  if (/^━━━ 【RunHelp】/.test(l.line)) {
    // 结束横幅：失败/超时标红；开始横幅与解释器行不标红
    return /执行失败|执行超时|failed|timeout/i.test(l.line);
  }
  return l.stream === 'stderr' && ERROR_RE.test(l.line);
}
const errorCount = computed(() => logs.value.filter(isErrorLog).length);
const visibleLogs = computed(() =>
  onlyErrors.value ? logs.value.filter(isErrorLog) : logs.value
);

// 窗口尺寸变化期间禁用过渡/动画，减轻最大化切小窗口时的闪屏
const resizing = ref(false);
let resizeTimer: number | undefined;
function onWindowResize() {
  resizing.value = true;
  if (resizeTimer) clearTimeout(resizeTimer);
  resizeTimer = window.setTimeout(() => (resizing.value = false), 180);
}

// 记住当前选中的项目、页签与日志面板状态：右键刷新 / 重新打开后自动恢复
const UI_STATE_KEY = 'runhelp.ui.state';
interface UiState {
  selectedId: number | null;
  tab: MainTab;
  consoleCollapsed?: boolean;
  consoleSize?: ConsoleSize;
  consoleCustomHeight?: number | null;
}
function saveUiState() {
  try {
    const state: UiState = {
      selectedId: selected.value?.id ?? null,
      tab: tab.value,
      consoleCollapsed: consoleCollapsed.value,
      consoleSize: consoleSize.value,
      consoleCustomHeight: consoleCustomHeight.value,
    };
    localStorage.setItem(UI_STATE_KEY, JSON.stringify(state));
  } catch {
    /* localStorage 不可用时忽略 */
  }
}
function readUiState(): UiState | null {
  try {
    const raw = localStorage.getItem(UI_STATE_KEY);
    return raw ? (JSON.parse(raw) as UiState) : null;
  } catch {
    return null;
  }
}
// 在 setup 阶段同步读取，避免初始化 watch 提前覆盖掉持久化状态
const initialUiState = readUiState();
let uiStateApplied = false;
// 立即恢复日志面板状态，避免首屏闪烁
if (initialUiState) {
  if (typeof initialUiState.consoleCollapsed === 'boolean') consoleCollapsed.value = initialUiState.consoleCollapsed;
  if (initialUiState.consoleSize) consoleSize.value = initialUiState.consoleSize;
  if (typeof initialUiState.consoleCustomHeight === 'number') consoleCustomHeight.value = initialUiState.consoleCustomHeight;
}

provide('notify', (msg: string) => showToast(msg));

// 应用内确认对话框：Tauri 2 默认禁用原生 window.confirm，必须自绘。
interface ConfirmOptions {
  title?: string;
  danger?: boolean;
  confirmText?: string;
}
interface ConfirmState {
  message: string;
  title: string;
  danger: boolean;
  confirmText: string;
  resolve: (v: boolean) => void;
}
const confirmState = ref<ConfirmState | null>(null);
function askConfirm(
  message: string,
  options: ConfirmOptions = {}
): Promise<boolean> {
  return new Promise((resolve) => {
    confirmState.value = {
      message,
      title: options.title ?? t('请确认'),
      danger: options.danger ?? false,
      confirmText: options.confirmText ?? t('确认'),
      resolve,
    };
  });
}
function answerConfirm(v: boolean) {
  confirmState.value?.resolve(v);
  confirmState.value = null;
}
provide('confirm', askConfirm);

// 运行中任务：定时轮询，保证常驻服务可被终止
let runningTimer: number | undefined;
async function loadRunning() {
  try {
    running.value = await api.listRunning();
  } catch {
    /* 忽略：后端暂不可用时不影响其它功能 */
  }
}

// 当前项目类型可用的页签（全局设置已移到右上角按钮，这里只留全局任务）
const availableTabs = computed<{ key: MainTab; label: string }[]>(() => {
  const p = selected.value;
  const globalTabs: { key: MainTab; label: string }[] = [
    { key: 'tasks', label: t('全局任务') },
  ];
  if (!p) return globalTabs;
  if (p.type === 'image') {
    return [{ key: 'compress', label: t('图片压缩') }, ...globalTabs];
  }
  return [{ key: 'commands', label: t('命令中心') }, ...globalTabs];
});

// 全局设置虽不在页签栏，但仍是合法页签（由右上角按钮进入），需参与校验与持久化
function isTabAvailable(key: MainTab): boolean {
  return key === 'settings' || availableTabs.value.some((t) => t.key === key);
}

// 项目类型变化时，若当前页签不可用则自动切到第一个
watch(
  () => selected.value?.type,
  () => {
    if (!isTabAvailable(tab.value)) {
      tab.value = availableTabs.value[0]?.key ?? 'settings';
    }
  },
  { immediate: true }
);

// 进入全局任务页时刷新一次运行中列表与历史；同时持久化当前页签
watch(tab, (t) => {
  saveUiState();
  if (t === 'tasks') {
    loadRunning();
    loadHistory();
  }
});

// 切换项目时持久化选中项
watch(() => selected.value?.id, () => saveUiState());

// 点击项目：切到该项目可用的第一个页签
function selectProject(p: Project) {
  selected.value = p;
  tab.value = availableTabs.value[0]?.key ?? 'settings';
}

function showToast(msg: string) {
  toast.value = msg;
  if (toastTimer) clearTimeout(toastTimer);
  toastTimer = window.setTimeout(() => (toast.value = ''), 2600);
}

// 复制当前窗口日志（跟随「错误」筛选，复制的是可见内容）
async function copyLogs() {
  const text = visibleLogs.value.map((l) => l.line).join('\n');
  if (!text) {
    showToast(t('当前没有可复制的日志'));
    return;
  }
  try {
    if (navigator.clipboard?.writeText) {
      await navigator.clipboard.writeText(text);
    } else {
      // 降级：临时 textarea + execCommand（兼容个别 WebView2 权限场景）
      const ta = document.createElement('textarea');
      ta.value = text;
      ta.style.position = 'fixed';
      ta.style.opacity = '0';
      document.body.appendChild(ta);
      ta.select();
      document.execCommand('copy');
      document.body.removeChild(ta);
    }
    showToast(t('已复制 {count} 行日志', { count: visibleLogs.value.length }));
  } catch {
    showToast(t('复制失败，请检查剪贴板权限'));
  }
}

// 艺术字跳转开源仓库：WebView2 里 <a target=_blank> 会被拦截，改走系统默认浏览器
async function openRepo() {
  try {
    await openUrl('https://gitcode.com/binglengdexie/RunHelp');
  } catch {
    showToast(t('打开仓库失败'));
  }
}

async function loadProjects() {
  projects.value = await api.listProjects();
  const saved = uiStateApplied ? readUiState() : initialUiState;
  uiStateApplied = true;
  const savedProject =
    saved?.selectedId != null
      ? projects.value.find((p) => p.id === saved.selectedId) ?? null
      : null;
  if (savedProject) {
    selected.value = savedProject;
  } else if (!selected.value && projects.value.length) {
    selected.value = projects.value[0];
  }
  // 恢复页签；失效时按当前项目回退到第一个可用页签
  if (saved?.tab && isTabAvailable(saved.tab)) {
    tab.value = saved.tab;
  } else if (!selected.value) {
    tab.value = availableTabs.value[0]?.key ?? 'settings';
  }
}

function onProjectCreated(p: Project) {
  projects.value.unshift(p);
  selected.value = p;
}

function onProjectDeleted(id: number) {
  projects.value = projects.value.filter((p) => p.id !== id);
  if (selected.value?.id === id) {
    selected.value = projects.value[0] ?? null;
    if (!selected.value) tab.value = 'settings';
  }
}

async function onProjectRestored() {
  // 还原的项目重新出现在列表
  await loadProjects();
}

function onProjectUpdated(p: Project) {
  const idx = projects.value.findIndex((x) => x.id === p.id);
  if (idx >= 0) projects.value[idx] = p;
  selected.value = p;
}

async function loadHistory() {
  history.value = await api.listHistory(50);
  await loadRunning();
}

async function clearHistory() {
  if (!(await askConfirm(t('确定清空全部运行记录？此操作不可恢复。'), { title: t('清空运行记录'), danger: true, confirmText: t('清空') }))) return;
  await api.clearHistory();
  await loadHistory();
  showToast(t('运行记录已清空'));
}

async function stopRunning(runId: string) {
  if (!(await askConfirm(t('终止运行任务 [{runId}]？将结束对应的进程（含其子进程）。', { runId }), { title: t('终止运行任务'), danger: true, confirmText: t('终止') }))) return;
  try {
    await api.stopRun(runId);
    showToast(t('已终止 [{runId}]', { runId }));
  } catch (e) {
    showToast(t('终止失败: {error}', { error: String(e) }));
  }
  setTimeout(loadRunning, 500);
}

async function togglePauseRun(r: import('./types').RunningInfo) {
  try {
    if (r.paused) {
      await api.resumeRun(r.run_id);
      showToast(t('[{runId}] 已继续', { runId: r.run_id }));
    } else {
      await api.pauseRun(r.run_id);
      showToast(t('[{runId}] 已请求暂停（当前命令结束后生效）', { runId: r.run_id }));
    }
  } catch (e) {
    showToast(t('操作失败: {error}', { error: String(e) }));
  }
  setTimeout(loadRunning, 300);
}

// 统一追加日志：根据跟随状态决定滚到底还是累计未读，再做限流裁剪。
// 裁剪时补偿 scrollTop，避免用户正在翻历史时因删除旧行导致视口跳变。
function appendLogs(lines: LogEvent[]) {
  if (!lines.length) return;
  const el = consoleBody.value;
  // 暂停跟随时记录裁剪前状态，裁剪后用像素差补偿 scrollTop，保证视口内容不跳
  const compensate = !isFollowing.value && el && logs.value.length + lines.length > 1200;
  const beforeH = compensate ? el!.scrollHeight : 0;
  const beforeTop = compensate ? el!.scrollTop : 0;
  const removeCount = logs.value.length + lines.length > 1200
    ? logs.value.length + lines.length - 800 : 0;
  if (removeCount > 0) logs.value.splice(0, removeCount);
  logs.value.push(...lines);
  if (isFollowing.value) {
    realignFollow();
  } else {
    unreadLines.value += lines.length;
    if (compensate) {
      // DOM 更新后：删除掉的旧行高度 = 裁剪前后 scrollHeight 之差
      void nextTick(() => {
        const el2 = consoleBody.value;
        if (!el2) return;
        const removedPx = beforeH - el2.scrollHeight;
        el2.scrollTop = Math.max(0, beforeTop - removedPx);
      });
    }
  }
}

function handleLog(e: LogEvent) {
  appendLogs([e]);
}

function handleRunEnd(e: RunEndEvent) {
  const incoming: LogEvent[] = [];
  // 兜底：把进程捕获到的 stderr 补进日志（流式可能已推送过，做去重）。
  if (e.stderr) {
    for (const line of e.stderr.split('\n')) {
      const text = line.trimEnd();
      if (!text) continue;
      const already = logs.value.some(
        (l) => l.run_id === e.run_id && l.stream === 'stderr' && l.line.trimEnd() === text
      ) || incoming.some(
        (l) => l.run_id === e.run_id && l.stream === 'stderr' && l.line.trimEnd() === text
      );
      if (!already) incoming.push({ run_id: e.run_id, stream: 'stderr', line: text });
    }
  }
  const label: Record<string, string> = {
    success: t('执行完成'),
    failed: t('执行失败'),
    timeout: t('执行超时'),
    stopped: t('已停止'),
  };
  const secs = (e.duration_ms / 1000).toFixed(1);
  incoming.push({
    run_id: e.run_id,
    stream: 'info',
    line: t('━━━ 【RunHelp】{label} · 退出码 {exitCode} · {secs}s ━━━', {
      label: label[e.status] ?? e.status,
      exitCode: e.exit_code,
      secs,
    }),
  });
  appendLogs(incoming);
  void maybeNotifySuccess(e);
  // 运行结束刷新记录（运行记录已移到「全局任务」页）
  loadHistory();
}

async function maybeNotifySuccess(e: RunEndEvent) {
  if (e.status !== 'success') return;
  // 流水线步骤 run_id 形如 xxx-s0，跳过，避免一步一弹
  if (/-s\d+$/.test(e.run_id)) return;
  try {
    const settings = await api.getSettings();
    if (settings.notify_on_success === '0') return;
    // 用 document.hasFocus() 而非 Tauri 的 window.isFocused()：后者在 WebView2 上
    // 依赖窗口事件同步，切换窗口时状态滞后，会出现前台误弹/后台漏弹。
    // document.hasFocus() 直接反映 WebView 是否持有焦点，最小化或被盖住时为 false。
    if (document.hasFocus()) return;
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === 'granted';
    if (!granted) return;
    const secs = (e.duration_ms / 1000).toFixed(1);
    await sendNotification({
      title: t('RunHelp · 执行完成'),
      body: t('{title} · 耗时 {secs}s', { title: e.title || t('任务'), secs }),
    });
  } catch {
    // 通知失败不影响主流程
  }
}

function onRun(id: string) {
  activeRunId.value = id;
  setTimeout(loadHistory, 400);
}

// 日志面板档位：点击在 normal → half → full 间循环
const SIZE_ORDER: ConsoleSize[] = ['normal', 'half', 'full'];
const sizeLabel = computed(() => ({
  normal: t('标准'), half: t('半屏'), full: t('全屏'),
}[consoleSize.value]));
function cycleConsoleSize() {
  const idx = SIZE_ORDER.indexOf(consoleSize.value);
  consoleSize.value = SIZE_ORDER[(idx + 1) % SIZE_ORDER.length];
  consoleCustomHeight.value = null; // 切档位清除自定义高度
  if (consoleCollapsed.value) consoleCollapsed.value = false;
  saveUiState();
  realignFollow();
}

// 面板高度样式：自定义高度优先，否则按档位
const consoleStyle = computed(() => {
  if (consoleCustomHeight.value != null) return { height: `${consoleCustomHeight.value}px` };
  if (consoleSize.value === 'half') return { height: '50%' };
  if (consoleSize.value === 'full') return { height: 'calc(100% - 8px)' };
  return { height: '210px' };
});

// 拖拽顶边调整高度：mousemove 跟手，mouseup 落定并持久化
let resizeCleanup: (() => void) | null = null;
function onResizeMove(ev: MouseEvent) {
  const panel = consolePanel.value;
  const content = panel?.parentElement;
  if (!panel || !content) return;
  const rect = content.getBoundingClientRect();
  const maxH = Math.max(120, rect.height - 80); // 主内容区至少保留 80px
  // 面板贴着内容区底部，高度 = 底边 - 鼠标Y（往上拖变大）
  const proposed = Math.min(Math.max(rect.bottom - ev.clientY, 120), maxH);
  consoleCustomHeight.value = Math.round(proposed);
}
function startResize(ev: MouseEvent) {
  if (consoleCollapsed.value) return;
  ev.preventDefault();
  document.body.style.userSelect = 'none';
  document.body.style.cursor = 'ns-resize';
  const cleanup = () => {
    window.removeEventListener('mousemove', onResizeMove);
    window.removeEventListener('mouseup', onUp);
    document.body.style.userSelect = '';
    document.body.style.cursor = '';
    resizeCleanup = null;
    saveUiState();
    realignFollow();
  };
  const onUp = () => cleanup();
  resizeCleanup = cleanup;
  window.addEventListener('mousemove', onResizeMove);
  window.addEventListener('mouseup', onUp);
}
function clearLogsAndReset() {
  logs.value = [];
  unreadLines.value = 0;
  isFollowing.value = true;
}

function statusText(s: string) {
  return { success: t('成功'), failed: t('失败'), timeout: t('超时'), running: t('运行中'), stopped: t('已停止') }[s] ?? s;
}

onMounted(async () => {
  window.addEventListener('resize', onWindowResize);
  await loadProjects();
  await loadHistory();
  await listen<LogEvent>('run://log', (e) => handleLog(e.payload));
  await listen<RunEndEvent>('run://end', (e) => {
    handleRunEnd(e.payload);
    loadRunning();
  });
  const settings = await api.getSettings();
  setLocale(settings.language);
  const theme = settings.theme ?? 'dark';
  document.documentElement.setAttribute('data-theme', theme);
  // 同步原生标题栏明暗与窗口背景色，避免浅色主题下顶部仍深色
  try { await api.setWindowTheme(theme); } catch { /* 旧版后端无此命令时忽略 */ }
  try { appVersion.value = await getVersion(); } catch { appVersion.value = ''; }
  // 每 3 秒刷新运行中列表：常驻服务可能在其它页签启动/终止。
  runningTimer = window.setInterval(loadRunning, 3000);
});

onUnmounted(() => {
  window.removeEventListener('resize', onWindowResize);
  if (resizeTimer) clearTimeout(resizeTimer);
  if (runningTimer) clearInterval(runningTimer);
  if (resizeCleanup) resizeCleanup();
});
</script>

<template>
  <div class="app" :class="{ resizing }">
    <aside class="sidebar">
      <!-- 品牌字标：艺术字 RunHelp + 版本号，点击打开开源仓库 -->
      <div class="brand">
        <a
          class="brand-lockup"
          href="https://gitcode.com/binglengdexie/RunHelp"
          :title="t('打开 RunHelp 开源仓库')"
          @click.prevent="openRepo"
        >
          <div class="brand-art">
            <span class="art-run">Run</span><span class="art-help">Help</span>
          </div>
          <div class="brand-version">{{ appVersion }}</div>
        </a>
      </div>
      <ProjectSidebar
        :projects="projects"
        :selected="selected"
        @created="onProjectCreated"
        @deleted="onProjectDeleted"
        @updated="onProjectUpdated"
        @restored="onProjectRestored"
        @select="selectProject"
      />
    </aside>

    <main class="main">
      <header class="topbar">
        <nav class="tabs">
          <button
            v-for="t in availableTabs"
            :key="t.key"
            :class="{ active: tab === t.key }"
            @click="tab = t.key"
          >{{ t.label }}</button>
        </nav>
        <div class="topbar-right">
          <button
            class="btn settings-btn"
            :class="{ active: tab === 'settings' }"
            :title="t('全局设置')"
            @click="tab = 'settings'"
          >
            <svg viewBox="0 0 20 20" width="15" height="15" aria-hidden="true">
              <path fill="currentColor" d="M10 6.2a3.8 3.8 0 1 0 0 7.6 3.8 3.8 0 0 0 0-7.6Zm0 5.6a1.8 1.8 0 1 1 0-3.6 1.8 1.8 0 0 1 0 3.6Z" />
              <path fill="currentColor" d="m17.2 11.4.9.7-.9 1.6-1.1-.4a6.9 6.9 0 0 1-1.3 1.3l.2 1.1-1.6.9-.7-.9a7 7 0 0 1-1.5.6l-.4 1.1h-1.8l-.4-1.1a7 7 0 0 1-1.5-.6l-.7.9-1.6-.9.2-1.1a6.9 6.9 0 0 1-1.3-1.3l-1.1.4-.9-1.6.9-.7a7.2 7.2 0 0 1 0-1.5l-.9-.7.9-1.6 1.1.4a6.9 6.9 0 0 1 1.3-1.3l-.2-1.1 1.6-.9.7.9a7 7 0 0 1 1.5-.6L8 4.7h1.8l.4 1.1a7 7 0 0 1 1.5.6l.7-.9 1.6.9-.2 1.1a6.9 6.9 0 0 1 1.3 1.3l1.1-.4.9 1.6-.9.7a7.2 7.2 0 0 1 0 1.4Z" opacity=".85" />
            </svg> {{ t('全局设置') }}
          </button>
        </div>
      </header>

      <div class="content">
        <div class="workspace">
          <CommandCenter
            v-if="tab === 'commands'"
            :project="selected"
            :running="running"
            @run="onRun"
            @clear-logs="clearLogsAndReset"
          />
          <ImageCompress
            v-else-if="tab === 'compress'"
            :project="selected"
            @run="onRun"
          />
          <SettingsView v-else-if="tab === 'settings'" />

          <!-- 全局任务：跨项目查看并管理所有运行中任务与历史 -->
          <div v-else-if="tab === 'tasks'" class="task-page">
            <div class="page-head">
              <div>
                <h2>{{ t('全局任务') }}</h2>
                <p class="muted">{{ t('查看并管理所有项目启动的运行任务，终止常驻服务后可释放端口') }}</p>
              </div>
              <button class="btn" :title="t('刷新运行中任务')" :disabled="refreshing" @click="refreshTasks">
                <svg class="refresh-icon" :class="{ spinning: refreshing }" viewBox="0 0 20 20" width="15" height="15" aria-hidden="true">
                  <path d="M16.5 10a6.5 6.5 0 1 1-1.9-4.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
                  <path d="M16.5 2.6v4h-4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
                </svg> {{ t('刷新') }}
              </button>
            </div>

            <section class="card task-card">
              <div class="task-card-head">
                <strong><span class="pulse"></span> {{ t('运行中任务') }} <span class="muted">{{ running.length }}</span></strong>
                <span class="muted hint">{{ t('每 3 秒自动刷新') }}</span>
              </div>
              <div v-if="!running.length" class="task-empty muted">{{ t('当前没有运行中的任务') }}</div>
              <div v-for="r in running" :key="r.run_id" class="task-row">
                <span class="mono muted task-rid">[{{ r.run_id }}]</span>
                <span class="task-target" :title="r.target">{{ r.target }}</span>
                <span class="muted task-at">{{ r.started_at }}</span>
                <span class="muted task-pid">PID {{ r.pid }}</span>
                <span v-if="r.paused" class="tag paused">{{ t('已暂停') }}</span>
                <button class="btn sm" @click="togglePauseRun(r)">{{ r.paused ? t('继续') : t('暂停') }}</button>
                <button class="btn sm danger" @click="stopRunning(r.run_id)">{{ t('终止') }}</button>
              </div>
            </section>

            <section class="card task-card">
              <div class="task-card-head">
                <strong>{{ t('运行记录') }} <span class="muted">{{ history.length }}</span></strong>
                <button class="btn ghost sm" @click="clearHistory">{{ t('清空记录') }}</button>
              </div>
              <div v-if="!history.length" class="task-empty muted">{{ t('暂无运行记录') }}</div>
              <div v-for="h in history" :key="h.id" class="history-row">
                <span class="status-dot" :class="h.status"></span>
                <span class="mono muted">#{{ h.id }}</span>
                <span class="history-target" :title="h.target">{{ h.target }}</span>
                <span class="muted">{{ h.started_at }}</span>
                <span class="status-text" :class="h.status">{{ statusText(h.status) }}</span>
              </div>
            </section>
          </div>
        </div>

        <!-- 运行日志默认折叠，所有页签常驻显示；点标题栏空白处即可展开/收起 -->
        <section
          ref="consolePanel"
          class="console"
          :class="{ collapsed: consoleCollapsed }"
          :style="consoleStyle"
        >
          <!-- 顶边拖拽手柄：仅展开态可拖动调整高度 -->
          <div
            v-if="!consoleCollapsed"
            class="console-resize-handle"
            :title="t('拖动调整日志高度')"
            @mousedown="startResize"
          ></div>
          <div
            class="console-head"
            :title="consoleCollapsed ? t('展开运行日志') : t('收起运行日志')"
            @click="consoleCollapsed = !consoleCollapsed; saveUiState(); realignFollow()"
          >
            <div class="console-toggle">
              <span class="chevron">{{ consoleCollapsed ? '▴' : '▾' }}</span>
              <span class="console-title">{{ t('运行日志') }}</span>
              <span v-if="consoleCollapsed && errorCount" class="err-count">{{ errorCount }}</span>
              <!-- 档位切换归左侧视图控制区，与清空物理隔离 -->
              <button
                class="btn ghost sm size-btn"
                :title="t('日志高度：{size}（点击切换）', { size: sizeLabel })"
                @click.stop="cycleConsoleSize"
              >{{ sizeLabel }}</button>
            </div>
            <div class="console-actions" @click.stop>
              <span class="muted">{{ t('{count} 行', { count: visibleLogs.length }) }}</span>
              <button
                class="btn sm"
                :class="{ 'filter-active': onlyErrors }"
                :title="t('仅显示错误日志')"
                @click="onlyErrors = !onlyErrors; realignFollow()"
              >{{ t('错误') }} <span class="err-count">{{ errorCount }}</span></button>
              <button class="btn ghost sm" :title="t('复制当前日志')" @click="copyLogs">{{ t('复制') }}</button>
              <span class="console-action-sep"></span>
              <button class="btn danger sm" :title="t('清空所有日志（不可恢复）')" @click="clearLogsAndReset">{{ t('清空') }}</button>
            </div>
          </div>
          <div v-if="!consoleCollapsed" ref="consoleBody" class="console-body mono" @scroll.passive="onConsoleScroll">
            <div v-if="!visibleLogs.length" class="console-placeholder muted">
              {{ onlyErrors && errorCount === 0 ? t('当前没有错误日志') : t('命令输出会实时显示在这里...') }}
            </div>
            <div
              v-for="(l, i) in visibleLogs"
              :key="i"
              class="log-line"
              :class="{ 'is-error': isErrorLog(l) }"
            >{{ l.line }}</div>
          </div>
          <!-- 暂停跟随时浮出的"跳到最新"，点击恢复自动滚动 -->
          <button
            v-if="!consoleCollapsed && !isFollowing && unreadLines"
            class="jump-latest"
            @click="jumpToLatest"
          >↓ {{ t('跳到最新') }} · {{ t('{count} 行新日志', { count: unreadLines }) }}</button>
        </section>
      </div>
    </main>

    <div v-if="toast" class="toast">{{ toast }}</div>

    <!-- 全局确认对话框（替代被 Tauri 禁用的原生 confirm） -->
    <div v-if="confirmState" class="modal-mask" @click.self="answerConfirm(false)">
      <div class="modal confirm-modal" role="dialog" aria-modal="true">
        <h3 :class="{ danger: confirmState.danger }">{{ confirmState.title }}</h3>
        <p class="confirm-message">{{ confirmState.message }}</p>
        <div class="modal-actions">
          <button class="btn" @click="answerConfirm(false)">{{ t('取消') }}</button>
          <button
            class="btn"
            :class="confirmState.danger ? 'danger' : 'primary'"
            @click="answerConfirm(true)"
          >{{ confirmState.confirmText }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.app { display: flex; height: 100%; background: var(--bg-0); }
/* 窗口尺寸变化时关掉过渡与动画，避免最大化/还原时 WebView2 重绘闪屏 */
.app.resizing *,
.app.resizing *::before,
.app.resizing *::after {
  transition: none !important;
  animation: none !important;
}
.sidebar {
  width: 232px; flex-shrink: 0; background: var(--bg-1);
  border-right: 1px solid var(--border); display: flex; flex-direction: column;
}
.brand {
  height: 60px; display: flex; align-items: center; justify-content: center;
  padding: 0 18px; border-bottom: 1px solid var(--border-soft);
}
.brand-lockup {
  display: flex; flex-direction: column; align-items: center; gap: 5px;
  text-decoration: none; cursor: pointer; border-radius: 6px;
  padding: 4px 8px; margin: -4px -8px; transition: opacity .15s;
}
.brand-lockup:hover { opacity: .82; }
.brand-lockup:active { opacity: .65; }
/* RunHelp 艺术字：Run 用主文字色，Help 用渐变填充，斜体呈现速度感 */
.brand-art {
  font-size: 21px; font-weight: 800; font-style: italic; letter-spacing: .06em;
  line-height: 1; white-space: nowrap;
}
.art-run { color: var(--text-0); }
.art-help {
  background: linear-gradient(100deg, var(--accent) 0%, var(--success) 100%);
  -webkit-background-clip: text; background-clip: text;
  -webkit-text-fill-color: transparent; color: var(--accent);
}
.brand-version {
  font-family: var(--mono); font-size: 10px; line-height: 1;
  color: var(--text-2);
}
.main { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.topbar {
  height: 60px; flex-shrink: 0; display: flex; align-items: center;
  justify-content: space-between; padding: 0 22px;
  border-bottom: 1px solid var(--border); background: var(--bg-1);
}
.tabs { display: flex; gap: 4px; }
.tabs button {
  height: 60px; padding: 0 16px; background: none; border: none;
  color: var(--text-1); cursor: pointer; position: relative; font-size: 13px;
}
.tabs button:hover { color: var(--text-0); }
.tabs button.active { color: var(--accent); font-weight: 600; }
.tabs button.active::after {
  content: ''; position: absolute; left: 14px; right: 14px; bottom: 0;
  height: 2px; background: var(--accent); border-radius: 2px 2px 0 0;
}
.topbar-right { font-size: 12px; display: flex; align-items: center; gap: 12px; }
.settings-btn { gap: 7px; }
.settings-btn.active { border-color: var(--accent); color: var(--accent); }
.content { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.workspace { flex: 1; min-height: 0; overflow: auto; padding: 20px 22px; }
.console {
  position: relative;
  height: 210px; flex-shrink: 0; border-top: 1px solid var(--border);
  background: var(--bg-1); display: flex; flex-direction: column;
}
/* 收缩态高度 53px：与左侧回收站上方分割线（sidebar-foot 顶边）齐平 */
.console.collapsed { height: 53px !important; }
/* 顶边拖拽手柄：加宽可点击区域，视觉上仍是一条细线 */
.console-resize-handle {
  position: absolute; top: -3px; left: 0; right: 0; height: 8px;
  cursor: ns-resize; z-index: 3;
}
.console-resize-handle::after {
  content: ''; position: absolute; top: 3px; left: 0; right: 0; height: 2px;
  background: transparent; transition: background .15s;
}
.console-resize-handle:hover::after { background: var(--accent); }
/* 左侧档位按钮：与折叠箭头同属视图控制区 */
.size-btn { margin-left: 4px; font-size: 11px; }
/* 危险操作（清空）前的分隔线，与右侧常规操作隔开 */
.console-action-sep { width: 1px; height: 15px; background: var(--border); }
.console-head {
  height: 36px; flex-shrink: 0; display: flex; align-items: center;
  justify-content: space-between; padding: 0 16px;
  border-bottom: 1px solid var(--border-soft); cursor: pointer;
  user-select: none;
}
.console.collapsed .console-head { height: 100%; border-bottom: none; }
.console-toggle {
  display: inline-flex; align-items: center; gap: 8px;
  color: inherit;
}
.chevron { font-size: 10px; color: var(--text-2); }
.console-title { font-size: 12px; font-weight: 600; color: var(--text-0); }
.console-actions { display: flex; align-items: center; gap: 10px; font-size: 11px; }
.btn.filter-active { border-color: var(--danger); color: var(--danger); background: rgba(224,92,92,.1); }
.err-count {
  display: inline-flex; align-items: center; justify-content: center;
  min-width: 18px; height: 16px; padding: 0 4px; border-radius: 8px;
  background: rgba(224,92,92,.16); color: var(--danger); font-size: 10px; font-weight: 700;
}
.console-body { flex: 1; min-height: 0; overflow: auto; padding: 8px 16px; font-size: 12px; line-height: 1.65; }
.console-placeholder { height: 100%; display: flex; align-items: center; justify-content: center; }
/* 暂停跟随时浮在日志右下角的"跳到最新" */
.jump-latest {
  position: absolute; right: 20px; bottom: 12px; z-index: 4;
  height: 27px; padding: 0 12px; border-radius: 14px;
  background: var(--accent); color: #fff; border: none;
  font-size: 11px; cursor: pointer;
  box-shadow: 0 2px 8px rgba(0, 0, 0, .25);
}
.jump-latest:hover { background: var(--accent-hover); }
.log-line { white-space: pre-wrap; word-break: break-all; color: var(--text-1); }
.log-line.stdout { color: var(--text-0); }
.log-line.stderr { color: var(--text-1); }
.log-line.info { color: var(--accent); }
/* 只有判定为错误的日志才标红，避免正常输出（如 npm 进度写 stderr）被误标 */
.log-line.is-error { color: var(--danger); }
.history-row { display: flex; align-items: center; gap: 10px; padding: 6px 2px; border-bottom: 1px solid var(--border-soft); font-size: 12px; }
.history-target { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.status-dot { width: 7px; height: 7px; border-radius: 50%; background: var(--text-2); flex-shrink: 0; }
.status-dot.success { background: var(--success); }
.status-dot.failed, .status-dot.timeout { background: var(--danger); }
.status-dot.running { background: var(--warn); }
.status-dot.stopped { background: var(--text-2); }
.status-text.success { color: var(--success); }
.status-text.failed, .status-text.timeout { color: var(--danger); }
.status-text.stopped { color: var(--text-2); }

/* 全局任务页 */
.task-page { display: flex; flex-direction: column; gap: 16px; }
.task-page .page-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 16px; }
.task-page .page-head h2 { font-size: 18px; font-weight: 600; }
.task-page .page-head p { font-size: 12px; margin-top: 4px; }
/* 刷新图标：SVG 几何中心即旋转中心，transform 原地旋转不位移、不晃动；纯 CSS 动画开销可忽略 */
.refresh-icon { display: inline-block; flex-shrink: 0; transform-origin: 50% 50%; }
.refresh-icon.spinning { animation: refresh-spin .7s linear infinite; }
@keyframes refresh-spin { to { transform: rotate(360deg); } }
.task-card { padding: 14px 16px; }
.task-card-head { display: flex; align-items: center; justify-content: space-between; margin-bottom: 8px; font-size: 13px; }
.task-card-head strong { display: inline-flex; align-items: center; gap: 8px; color: var(--accent); }
.task-card-head .hint { font-size: 11px; }
.task-empty { padding: 26px 0; text-align: center; font-size: 12px; }
.task-row { display: flex; align-items: center; gap: 12px; padding: 9px 2px; border-top: 1px solid var(--border-soft); font-size: 12px; }
.task-row:first-of-type { border-top: none; }
.task-rid { flex-shrink: 0; }
.task-target { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.task-at, .task-pid { white-space: nowrap; flex-shrink: 0; }
.tag.paused { background: var(--warn); color: #fff; }
.pulse { width: 8px; height: 8px; border-radius: 50%; background: var(--success); animation: pulse 1.4s infinite; flex-shrink: 0; }
@keyframes pulse { 0%,100% { opacity: 1; } 50% { opacity: .35; } }

/* 全局确认对话框 */
.confirm-modal { width: 420px; max-width: 92vw; }
.confirm-modal h3.danger { color: var(--danger); }
.confirm-message { font-size: 13px; line-height: 1.7; color: var(--text-1); white-space: pre-line; }
</style>
