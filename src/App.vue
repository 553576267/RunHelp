<script setup lang="ts">
import { ref, onMounted, onUnmounted, provide, computed, watch } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { getVersion } from '@tauri-apps/api/app';
import { open as openUrl } from '@tauri-apps/plugin-shell';
import { api } from './lib/api';
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

// 记住当前选中的项目与页签：右键刷新 / 重新打开后自动恢复
const UI_STATE_KEY = 'runhelp.ui.state';
function saveUiState() {
  try {
    localStorage.setItem(
      UI_STATE_KEY,
      JSON.stringify({ selectedId: selected.value?.id ?? null, tab: tab.value })
    );
  } catch {
    /* localStorage 不可用时忽略 */
  }
}
function readUiState(): { selectedId: number | null; tab: MainTab } | null {
  try {
    const raw = localStorage.getItem(UI_STATE_KEY);
    return raw ? (JSON.parse(raw) as { selectedId: number | null; tab: MainTab }) : null;
  } catch {
    return null;
  }
}
// 在 setup 阶段同步读取，避免初始化 watch 提前覆盖掉持久化状态
const initialUiState = readUiState();
let uiStateApplied = false;

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
      title: options.title ?? '请确认',
      danger: options.danger ?? false,
      confirmText: options.confirmText ?? '确认',
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
    { key: 'tasks', label: '全局任务' },
  ];
  if (!p) return globalTabs;
  if (p.type === 'image') {
    return [{ key: 'compress', label: '图片压缩' }, ...globalTabs];
  }
  return [{ key: 'commands', label: '命令中心' }, ...globalTabs];
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
    showToast('当前没有可复制的日志');
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
    showToast(`已复制 ${visibleLogs.value.length} 行日志`);
  } catch {
    showToast('复制失败，请检查剪贴板权限');
  }
}

// 艺术字跳转开源仓库：WebView2 里 <a target=_blank> 会被拦截，改走系统默认浏览器
async function openRepo() {
  try {
    await openUrl('https://gitcode.com/binglengdexie/RunHelp');
  } catch {
    showToast('打开仓库失败');
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
  if (!(await askConfirm('确定清空全部运行记录？此操作不可恢复。', { title: '清空运行记录', danger: true, confirmText: '清空' }))) return;
  await api.clearHistory();
  await loadHistory();
  showToast('运行记录已清空');
}

async function stopRunning(runId: string) {
  if (!(await askConfirm(`终止运行任务 [${runId}]？将结束对应的进程（含其子进程）。`, { title: '终止运行任务', danger: true, confirmText: '终止' }))) return;
  try {
    await api.stopRun(runId);
    showToast(`已终止 [${runId}]`);
  } catch (e) {
    showToast(`终止失败: ${e}`);
  }
  setTimeout(loadRunning, 500);
}

async function togglePauseRun(r: import('./types').RunningInfo) {
  try {
    if (r.paused) {
      await api.resumeRun(r.run_id);
      showToast(`[${r.run_id}] 已继续`);
    } else {
      await api.pauseRun(r.run_id);
      showToast(`[${r.run_id}] 已请求暂停（当前命令结束后生效）`);
    }
  } catch (e) {
    showToast(`操作失败: ${e}`);
  }
  setTimeout(loadRunning, 300);
}

function handleLog(e: LogEvent) {
  logs.value.push(e);
  // 限制日志条数，避免长时间运行后 DOM 节点过多推高 WebView2 内存
  if (logs.value.length > 1200) logs.value.splice(0, 400);
}

function handleRunEnd(e: RunEndEvent) {
  // 兜底：把进程捕获到的 stderr 补进日志（流式可能已推送过，做去重）。
  if (e.stderr) {
    for (const line of e.stderr.split('\n')) {
      const text = line.trimEnd();
      if (!text) continue;
      const already = logs.value.some(
        (l) => l.run_id === e.run_id && l.stream === 'stderr' && l.line.trimEnd() === text
      );
      if (!already) {
        logs.value.push({ run_id: e.run_id, stream: 'stderr', line: text });
      }
    }
  }
  const label: Record<string, string> = {
    success: '执行完成',
    failed: '执行失败',
    timeout: '执行超时',
    stopped: '已停止',
  };
  const secs = (e.duration_ms / 1000).toFixed(1);
  logs.value.push({
    run_id: e.run_id,
    stream: 'info',
    line: `━━━ 【RunHelp】${label[e.status] ?? e.status} · 退出码 ${e.exit_code} · ${secs}s ━━━`,
  });
  if (logs.value.length > 1200) logs.value.splice(0, 400);
  // 运行结束刷新记录（运行记录已移到「全局任务」页）
  loadHistory();
}

function onRun(id: string) {
  activeRunId.value = id;
  setTimeout(loadHistory, 400);
}

const consoleTitle = computed(() =>
  activeRunId.value ? `运行日志 · ${activeRunId.value}` : '运行日志'
);

function statusText(s: string) {
  return { success: '成功', failed: '失败', timeout: '超时', running: '运行中', stopped: '已停止' }[s] ?? s;
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
  const theme = (await api.getSettings()).theme ?? 'dark';
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
          title="打开 RunHelp 开源仓库"
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
            title="全局设置"
            @click="tab = 'settings'"
          >
            <svg viewBox="0 0 20 20" width="15" height="15" aria-hidden="true">
              <path fill="currentColor" d="M10 6.2a3.8 3.8 0 1 0 0 7.6 3.8 3.8 0 0 0 0-7.6Zm0 5.6a1.8 1.8 0 1 1 0-3.6 1.8 1.8 0 0 1 0 3.6Z" />
              <path fill="currentColor" d="m17.2 11.4.9.7-.9 1.6-1.1-.4a6.9 6.9 0 0 1-1.3 1.3l.2 1.1-1.6.9-.7-.9a7 7 0 0 1-1.5.6l-.4 1.1h-1.8l-.4-1.1a7 7 0 0 1-1.5-.6l-.7.9-1.6-.9.2-1.1a6.9 6.9 0 0 1-1.3-1.3l-1.1.4-.9-1.6.9-.7a7.2 7.2 0 0 1 0-1.5l-.9-.7.9-1.6 1.1.4a6.9 6.9 0 0 1 1.3-1.3l-.2-1.1 1.6-.9.7.9a7 7 0 0 1 1.5-.6L8 4.7h1.8l.4 1.1a7 7 0 0 1 1.5.6l.7-.9 1.6.9-.2 1.1a6.9 6.9 0 0 1 1.3 1.3l1.1-.4.9 1.6-.9.7a7.2 7.2 0 0 1 0 1.4Z" opacity=".85" />
            </svg> 全局设置
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
            @clear-logs="logs = []"
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
                <h2>全局任务</h2>
                <p class="muted">查看并管理所有项目启动的运行任务，终止常驻服务后可释放端口</p>
              </div>
              <button class="btn" title="刷新运行中任务" :disabled="refreshing" @click="refreshTasks">
                <svg class="refresh-icon" :class="{ spinning: refreshing }" viewBox="0 0 20 20" width="15" height="15" aria-hidden="true">
                  <path d="M16.5 10a6.5 6.5 0 1 1-1.9-4.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
                  <path d="M16.5 2.6v4h-4" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" />
                </svg> 刷新
              </button>
            </div>

            <section class="card task-card">
              <div class="task-card-head">
                <strong><span class="pulse"></span> 运行中任务 <span class="muted">{{ running.length }}</span></strong>
                <span class="muted hint">每 3 秒自动刷新</span>
              </div>
              <div v-if="!running.length" class="task-empty muted">当前没有运行中的任务</div>
              <div v-for="r in running" :key="r.run_id" class="task-row">
                <span class="mono muted task-rid">[{{ r.run_id }}]</span>
                <span class="task-target" :title="r.target">{{ r.target }}</span>
                <span class="muted task-at">{{ r.started_at }}</span>
                <span class="muted task-pid">PID {{ r.pid }}</span>
                <span v-if="r.paused" class="tag paused">已暂停</span>
                <button class="btn sm" @click="togglePauseRun(r)">{{ r.paused ? '继续' : '暂停' }}</button>
                <button class="btn sm danger" @click="stopRunning(r.run_id)">终止</button>
              </div>
            </section>

            <section class="card task-card">
              <div class="task-card-head">
                <strong>运行记录 <span class="muted">{{ history.length }}</span></strong>
                <button class="btn ghost sm" @click="clearHistory">清空记录</button>
              </div>
              <div v-if="!history.length" class="task-empty muted">暂无运行记录</div>
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
          class="console"
          :class="{ collapsed: consoleCollapsed }"
        >
          <div
            class="console-head"
            :title="consoleCollapsed ? '展开运行日志' : '收起运行日志'"
            @click="consoleCollapsed = !consoleCollapsed"
          >
            <div class="console-toggle">
              <span class="chevron">{{ consoleCollapsed ? '▴' : '▾' }}</span>
              <span class="console-title">{{ consoleTitle }}</span>
              <span v-if="consoleCollapsed && errorCount" class="err-count">{{ errorCount }}</span>
            </div>
            <div class="console-actions" @click.stop>
              <span class="muted">{{ visibleLogs.length }} 行</span>
              <button
                class="btn sm"
                :class="{ 'filter-active': onlyErrors }"
                title="仅显示错误日志"
                @click="onlyErrors = !onlyErrors"
              >错误 <span class="err-count">{{ errorCount }}</span></button>
              <button class="btn ghost sm" title="复制当前日志" @click="copyLogs">复制</button>
              <button class="btn ghost sm" @click="logs = []">清空</button>
            </div>
          </div>
          <div v-if="!consoleCollapsed" class="console-body mono">
            <div v-if="!visibleLogs.length" class="console-placeholder muted">
              {{ onlyErrors && errorCount === 0 ? '当前没有错误日志' : '命令输出会实时显示在这里...' }}
            </div>
            <div
              v-for="(l, i) in visibleLogs"
              :key="i"
              class="log-line"
              :class="{ 'is-error': isErrorLog(l) }"
            >{{ l.line }}</div>
          </div>
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
          <button class="btn" @click="answerConfirm(false)">取消</button>
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
  height: 210px; flex-shrink: 0; border-top: 1px solid var(--border);
  background: var(--bg-1); display: flex; flex-direction: column;
}
/* 收缩态高度 53px：与左侧回收站上方分割线（sidebar-foot 顶边）齐平 */
.console.collapsed { height: 53px; }
.console-head {
  height: 36px; flex-shrink: 0; display: flex; align-items: center;
  justify-content: space-between; padding: 0 16px;
  border-bottom: 1px solid var(--border-soft); cursor: pointer;
}
.console.collapsed .console-head { height: 100%; border-bottom: none; }
.console-toggle {
  display: inline-flex; align-items: center; gap: 8px;
  color: inherit;
}
.console-toggle:hover .console-title { color: var(--accent); }
.chevron { font-size: 10px; color: var(--text-2); }
.console-title { font-size: 12px; font-weight: 600; color: var(--text-0); }
.console-actions { display: flex; align-items: center; gap: 10px; font-size: 11px; }
.btn.filter-active { border-color: var(--danger); color: var(--danger); background: rgba(224,92,92,.1); }
.err-count {
  display: inline-flex; align-items: center; justify-content: center;
  min-width: 18px; height: 16px; padding: 0 4px; border-radius: 8px;
  background: rgba(224,92,92,.16); color: var(--danger); font-size: 10px; font-weight: 700;
}
.console-body { flex: 1; overflow: auto; padding: 8px 16px; font-size: 12px; line-height: 1.65; }
.console-placeholder { height: 100%; display: flex; align-items: center; justify-content: center; }
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
