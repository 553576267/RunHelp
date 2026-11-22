<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, inject } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { writeText, readText } from '@tauri-apps/plugin-clipboard-manager';
import { api } from '../lib/api';
import { t } from '../lib/i18n';
import { useActionFeedback } from '../lib/useActionFeedback';
import type {
  SyncDevice,
  SyncStatus,
  SyncRequestEvent,
  SyncRequestItem,
  SyncReceivedEvent,
  SyncProgressEvent,
} from '../types';

type View = 'send' | 'receive' | 'settings';

interface SendModal {
  transferId: string;
  ip: string;
  targetName: string;
  kind: string;
  items: SyncRequestItem[];
  stage: 'waiting' | 'accepted' | 'transferring' | 'done' | 'cancelled' | 'error';
  // 失败具体原因（如「对方已拒绝接收」），成功/取消时为空
  detail: string;
  sent: number;
  total: number;
}

// 最近一次接收任务：接收页只展示这一批文件的清单与实时进度
interface ReceiveTask {
  id: string;
  transferId: string;
  from: string;
  fromPlatform: string;
  fingerprint: string;
  trusted: boolean;
  kind: string;
  total: number;
  // 每个文件一项，name 与后端 receive-progress 的 name 对应
  items: { name: string; size: number; received: number; status: 'pending' | 'receiving' | 'done' }[];
  stage: 'incoming' | 'receiving' | 'done' | 'rejected';
}

const notify = inject<(m: string) => void>('notify', () => {});

const view = ref<View>('send');
const status = ref<SyncStatus | null>(null);
const devices = ref<SyncDevice[]>([]);
const selectedId = ref('');
const textInput = ref('');
const pairIp = ref('');
const editingName = ref(false);
const nameDraft = ref('');
const busy = ref(false);
const activeAction = ref('');
// 刷新按钮统一交互：冷却 600ms、图标原地旋转、期间重复点击忽略、不显示"已刷新"字样。
// 全平台规则由 useActionFeedback 统一定义；自动刷新不走这里，只在附近设备列表显示加载图标。
const { isActive: isActionActive, trigger: triggerAction, clear: clearAction } = useActionFeedback({ cooldownMs: 600 });

// 接收：只展示最近一次接收任务的清单与进度；弹窗数据只在内存、单次展示
const activeReceive = ref<ReceiveTask | null>(null);
const fpDialog = ref<SyncRequestEvent | null>(null);
const requestDialog = ref<SyncRequestEvent | null>(null);
const pendingDecision = ref<{ id: string; indices: number[] } | null>(null);

// 发送弹窗：等待接受 / 已接受文件列表，数据只存在于内存
const sendModal = ref<SendModal | null>(null);
const sendCanceling = ref(false);

let unlisteners: (() => void)[] = [];
let autoTimer: number | undefined;

const deviceList = computed(() => {
  return [...devices.value].sort((a, b) => {
    return Number(b.favorite) - Number(a.favorite)
      || Number(b.online) - Number(a.online)
      || b.last_seen - a.last_seen;
  });
});
const selectedDevice = computed(() => deviceList.value.find((d) => d.id === selectedId.value));
const incomingCount = computed(() => activeReceive.value?.stage === 'incoming' ? 1 : 0);
const displayName = (d: SyncDevice) => d.alias || d.name || d.id.slice(0, 8);
// 最近一次接收任务的整体进度（按各文件已收字节 / 总字节）
const receiveTaskPercent = computed(() => {
  const t = activeReceive.value;
  if (!t || !t.total) return 0;
  const done = t.items.reduce((sum, i) => sum + i.received, 0);
  return Math.min(100, Math.round(done / t.total * 100));
});

// 用请求事件构造/刷新最近一次接收任务（清单来自请求，进度初始为 pending）
function buildReceiveTask(req: SyncRequestEvent): ReceiveTask {
  return {
    id: req.id,
    transferId: req.transfer_id,
    from: req.from,
    fromPlatform: req.from_platform,
    fingerprint: req.fingerprint,
    trusted: req.trusted,
    kind: req.kind,
    total: req.size,
    items: req.items.map((item) => ({ name: item.name, size: item.size, received: 0, status: 'pending' })),
    stage: 'incoming',
  };
}

async function load() {
  try {
    status.value = await api.syncStatus();
    nameDraft.value = status.value.device_name;
    devices.value = await api.syncListDevices();
    if (!status.value.running) {
      await api.syncStart();
      status.value = await api.syncStatus();
    }
  } catch (e) {
    notify(t('同步服务启动失败: {error}', { error: String(e) }));
  }
}

onMounted(async () => {
  unlisteners.push(
    await listen<SyncRequestEvent>('sync://request', (e) => {
      const req = e.payload;
      // 每次请求覆盖为"最近一次接收"，接收页据此展示清单与进度。
      activeReceive.value = buildReceiveTask(req);
      // 未信任设备先核指纹；已信任设备直接弹请求摘要。
      if (!req.trusted) {
        fpDialog.value = req;
      } else {
        requestDialog.value = req;
      }
      if (view.value !== 'receive') {
        notify(t('收到传输请求'));
      }
    })
  );
  unlisteners.push(
    await listen<{ name: string; received: number; total: number; status: string }>('sync://receive-progress', (e) => {
      const d = e.payload;
      const task = activeReceive.value;
      if (!task) return;
      const item = task.items.find((i) => i.name === d.name);
      if (!item) return;
      if (d.status === 'done') {
        item.received = item.size;
        item.status = 'done';
      } else {
        item.received = d.received;
        item.status = 'receiving';
      }
      // 有文件开始落盘即视为接收中；全部 done 时收尾。
      if (task.stage === 'incoming') task.stage = 'receiving';
      if (task.items.length && task.items.every((i) => i.status === 'done')) task.stage = 'done';
    })
  );
  unlisteners.push(
    await listen<SyncReceivedEvent>('sync://received', async (e) => {
      const d = e.payload;
      // 兜底：确保对应清单项标记为完成（文本/剪贴板没有分块进度）。
      const task = activeReceive.value;
      if (task) {
        const item = task.items.find((i) => i.name === d.name) ?? task.items[0];
        if (item) { item.received = item.size; item.status = 'done'; }
        if (task.items.every((i) => i.status === 'done')) task.stage = 'done';
      }
      if (d.kind === 'text' || d.kind === 'clipboard') {
        try {
          await writeText(d.text);
          notify(t('已收到来自 {name} 的内容，已放入剪贴板', { name: d.from }));
        } catch {
          notify(t('已收到来自 {name} 的文字', { name: d.from }));
        }
      } else {
        notify(t('已收到 {name} 的文件：{path}', { name: d.from, path: d.path }));
      }
    })
  );
  unlisteners.push(
    await listen<SyncProgressEvent>('sync://progress', (e) => {
      const p = e.payload;
      const modal = sendModal.value;
      if (!modal || p.transfer_id !== modal.transferId) return;
      if (p.status === 'waiting') modal.stage = 'waiting';
      if (p.status === 'accepted') modal.stage = 'accepted';
      if (p.status === 'transferring') {
        modal.stage = 'transferring';
        modal.sent = p.sent;
        modal.total = p.total;
      }
      if (p.status === 'cancelled') modal.stage = 'cancelled';
      if (p.status === 'done') {
        modal.stage = 'done';
        modal.sent = p.sent;
        modal.total = p.total;
        notify(t('发送完成'));
        // 成功也保留弹窗，由发送方确认后手动关闭
      }
    })
  );
  unlisteners.push(
    await listen<{ transfer_id: string; error: string }>('sync://send-error', (e) => {
      const d = e.payload;
      const modal = sendModal.value;
      if (!modal || modal.transferId !== d.transfer_id) return;
      setSendFailure(modal, d.error);
    })
  );
  // 发送端撤销：接收方关闭对应请求的弹窗与待接收项（弹窗数据只在内存，单次展示）
  unlisteners.push(
    await listen<{ transfer_id: string }>('sync://request-cancelled', (e) => {
      const tid = e.payload.transfer_id;
      if (activeReceive.value?.transferId === tid) activeReceive.value = null;
      if (requestDialog.value?.transfer_id === tid) requestDialog.value = null;
      if (fpDialog.value?.transfer_id === tid) fpDialog.value = null;
      if (pendingDecision.value) pendingDecision.value = null;
    })
  );
  await load();
  // 挂载兜底：若此前因请求自动切页进来、事件在挂载前已发出，主动拉一次并补弹确认框
  void pullPending(true);
  // 自动刷新在后台静默进行，不体现任何加载状态、不干扰手动刷新按钮。
  autoTimer = window.setInterval(() => { void doRefreshDevices(false); }, 5000);
});

onUnmounted(() => {
  unlisteners.forEach((fn) => fn());
  if (autoTimer) clearInterval(autoTimer);
  clearAction();
});

async function doRefreshDevices(manual: boolean) {
  // 自动刷新在后台静默进行，不显示任何加载状态。
  try {
    devices.value = await api.syncListDevices();
  } catch (e) {
    if (manual) notify(String(e));
  }
}

// 手动刷新：走统一按钮反馈（图标旋转 600ms），不再显示"已刷新"字样
function refreshDevices() {
  triggerAction('refresh-devices', () => { void doRefreshDevices(true); });
}

async function pullPending(prompt = false) {
  try {
    const pending = await api.syncListPending();
    if (!pending.length) return;
    const req = pending[pending.length - 1];
    // 接收请求靠事件实时推送；刷新/挂载用于兜底：只要出现新的待确认请求就切换到最新一条，
    // 即使当前显示的是已拒绝/已完成的历史任务。
    if (!activeReceive.value || activeReceive.value.id !== req.id) {
      activeReceive.value = buildReceiveTask(req);
      // 自动切页进来时，事件可能在本组件挂载前就发出了；这里补弹确认框，
      // 与 sync://request 监听保持一致（未信任先核指纹，已信任直接弹摘要）。
      if (prompt) {
        if (!req.trusted) fpDialog.value = req;
        else requestDialog.value = req;
      }
    }
  } catch (e) {
    notify(String(e));
  }
}

// 手动刷新：走统一按钮反馈，与发送页刷新、命令中心运行按钮一致
function refreshIncoming() {
  triggerAction('refresh-incoming', () => { void pullPending(); });
}

async function saveName() {
  if (!nameDraft.value.trim()) return;
  try {
    await api.syncSetDeviceName(nameDraft.value.trim());
    status.value = await api.syncStatus();
    editingName.value = false;
    notify(t('设备名已保存'));
  } catch (e) {
    notify(String(e));
  }
}

async function setReceiveDir() {
  try {
    const dir = await open({ directory: true, title: t('选择接收目录') });
    if (!dir || typeof dir !== 'string') return;
    await api.syncSetReceiveDir(dir);
    status.value = await api.syncStatus();
    notify(t('接收目录已保存'));
  } catch (e) {
    notify(String(e));
  }
}

async function doPair() {
  if (!pairIp.value.trim()) {
    notify(t('请填写对方 IP'));
    return;
  }
  busy.value = true;
  activeAction.value = 'pair';
  try {
    const device = await api.syncPairDevice(pairIp.value.trim());
    selectedId.value = device.id;
    await doRefreshDevices(false);
    pairIp.value = '';
    notify(t('已添加设备，传输时确认对方指纹即可建立信任'));
  } catch (e) {
    notify(t('配对失败: {error}', { error: String(e) }));
  } finally {
    busy.value = false;
    activeAction.value = '';
  }
}

async function toggleFavorite(device: SyncDevice) {
  try {
    await api.syncSetFavorite(device.fingerprint, !device.favorite);
    await doRefreshDevices(false);
  } catch (e) {
    notify(String(e));
  }
}

function selectDevice(device: SyncDevice) {
  if (!device.online) {
    notify(t('该设备当前离线'));
    return;
  }
  selectedId.value = device.id;
}

function newTransferId() {
  return `x${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

function basename(path: string) {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] || path;
}

// 发送失败：展示具体原因（拒绝 / 撤销 / 超时 / 未信任），弹窗保留到发送方手动关闭
function setSendFailure(modal: SendModal, msg: string) {
  if (modal.stage === 'cancelled' || modal.stage === 'done') return;
  if (msg.includes('撤销')) {
    modal.stage = 'cancelled';
    modal.detail = t('已撤销发送');
  } else {
    modal.stage = 'error';
    modal.detail = msg.includes('拒绝') ? t('对方已拒绝接收')
      : msg.includes('时限') || msg.includes('超时') ? t('对方未在时限内接收')
      : msg.includes('信任') ? t('对方尚未信任本机')
      : msg;
  }
  notify(modal.detail);
}

function handleSendError(e: unknown) {
  const modal = sendModal.value;
  if (!modal) return;
  setSendFailure(modal, String(e));
}

async function sendText(kind: 'text' | 'clipboard') {
  const target = selectedDevice.value;
  if (!target || !target.online || busy.value) return;
  let text = textInput.value;
  if (kind === 'clipboard') {
    try {
      text = await readText();
    } catch {
      notify(t('读取剪贴板失败，请先手动输入文字'));
      return;
    }
  }
  if (!text.trim()) {
    notify(t('内容不能为空'));
    return;
  }
  const transferId = newTransferId();
  sendModal.value = {
    transferId,
    ip: target.ip,
    targetName: displayName(target),
    kind,
    items: [{ name: kindLabel(kind), size: text.length }],
    stage: 'waiting',
    detail: '',
    sent: 0,
    total: text.length,
  };
  busy.value = true;
  activeAction.value = kind;
  try {
    await api.syncSendText(target.ip, text, kind, transferId);
    textInput.value = '';
  } catch (e) {
    handleSendError(e);
  } finally {
    busy.value = false;
    activeAction.value = '';
  }
}

async function pickFiles(folder: boolean) {
  const target = selectedDevice.value;
  if (!target || !target.online || busy.value) return;
  try {
    const result = await open({ directory: folder, multiple: true, title: folder ? t('选择文件夹') : t('选择文件') });
    if (!result) return;
    let paths = (Array.isArray(result) ? result : [result]).filter((p): p is string => typeof p === 'string');
    if (!paths.length) return;

    // Android 的文件选择器返回 content:// URI，Rust 端无法按绝对路径读取。
    // 先让原生插件把 URI 内容复制到应用私有目录，改用真实路径发送。
    let displayNames: string[] | null = null;
    if (paths.some((p) => p.startsWith('content://'))) {
      if (folder) {
        notify(t('安卓暂不支持发送整个文件夹，请选择文件'));
        return;
      }
      const resolved = await api.mobileResolveUris(paths);
      if (!resolved.length) return;
      paths = resolved.map((r) => r.path);
      displayNames = resolved.map((r) => r.name);
    }
    const items = paths.map((p, i) => ({ name: displayNames?.[i] ?? basename(p), size: 0 }));
    const transferId = newTransferId();
    sendModal.value = {
      transferId,
      ip: target.ip,
      targetName: displayName(target),
      kind: folder ? 'folder' : 'file',
      items,
      stage: 'waiting',
      detail: '',
      sent: 0,
      total: paths.length,
    };
    busy.value = true;
    activeAction.value = folder ? 'folder' : 'file';
    await api.syncSendFiles(target.ip, paths, transferId);
  } catch (e) {
    handleSendError(e);
  } finally {
    busy.value = false;
    activeAction.value = '';
  }
}

async function cancelSend() {
  const modal = sendModal.value;
  if (!modal || sendCanceling.value) return;
  sendCanceling.value = true;
  try {
    await api.syncCancelSend(modal.ip, modal.transferId);
    modal.stage = 'cancelled';
    modal.detail = '';
    notify(t('已撤销发送'));
    // 取消也保留弹窗，发送方确认后手动关闭
  } catch (e) {
    notify(String(e));
  } finally {
    sendCanceling.value = false;
  }
}

function allIndices(req: SyncRequestEvent) {
  return req.items.map((_, i) => i);
}

// 弹窗/接收页点"接收"= 默认全部接收；未信任设备先核指纹再确认。
function requestDecision(req: SyncRequestEvent, indices: number[]) {
  if (!req.trusted) {
    pendingDecision.value = { id: req.id, indices };
    fpDialog.value = req;
    return;
  }
  void doRespond(req, indices);
}

async function doRespond(req: SyncRequestEvent, indices: number[]) {
  if (fpDialog.value?.id === req.id) fpDialog.value = null;
  if (requestDialog.value?.id === req.id) requestDialog.value = null;
  if (pendingDecision.value?.id === req.id) pendingDecision.value = null;
  // 同步更新最近一次接收任务的状态：有选中项=接收中（进度随后到达），空=已拒绝。
  if (activeReceive.value?.id === req.id) {
    activeReceive.value.stage = indices.length ? 'receiving' : 'rejected';
  }
  try {
    await api.syncRespondSelected(req.id, indices);
  } catch (e) {
    notify(String(e));
  }
}

function approveReceive(req: SyncRequestEvent) {
  // 弹窗点接收 = 默认全部接收
  requestDecision(req, allIndices(req));
}

function approveFingerprint() {
  const req = fpDialog.value;
  if (!req) return;
  const indices = pendingDecision.value?.id === req.id
    ? pendingDecision.value.indices
    : allIndices(req);
  void doRespond(req, indices);
}

function rejectReceive(req: SyncRequestEvent) {
  requestDecision(req, []);
}

// 接收页的"全部接收"针对当前最近一次任务；用请求重建 indices。
function receiveAllRequests() {
  const req = incomingViewReq.value;
  if (req) requestDecision(req, allIndices(req));
}

// 把最近一次接收任务还原成请求事件，供接收页按钮复用确认流程。
const incomingViewReq = computed<SyncRequestEvent | null>(() => {
  const t = activeReceive.value;
  if (!t || t.stage !== 'incoming') return null;
  return {
    id: t.id,
    transfer_id: t.transferId,
    from: t.from,
    from_platform: t.fromPlatform,
    fingerprint: t.fingerprint,
    trusted: t.trusted,
    kind: t.kind,
    name: t.items.map((i) => i.name).join(', '),
    size: t.total,
    count: t.items.length,
    items: t.items.map((i) => ({ name: i.name, size: i.size })),
  };
});

function openIncoming() {
  requestDialog.value = null;
  fpDialog.value = null;
  view.value = 'receive';
}

function fmtSize(n: number) {
  if (!n) return '0 B';
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / 1024 / 1024).toFixed(2)} MB`;
}

function kindLabel(kind: string) {
  const map: Record<string, string> = {
    text: t('文字'), clipboard: t('剪贴板'), file: t('文件'), folder: t('文件夹'),
  };
  return map[kind] ?? kind;
}

function platformLabel(platform: string) {
  const map: Record<string, string> = {
    windows: 'Windows', win32: 'Windows', linux: 'Linux', darwin: 'macOS', android: 'Android',
  };
  return map[platform] || platform || t('未知设备');
}

function shortFp(fp: string) {
  return fp ? fp.slice(0, 12).toUpperCase().replace(/(.{4})/g, '$1-').replace(/-$/, '') : '—';
}

function receiveStageLabel(stage: ReceiveTask['stage']) {
  const map: Record<ReceiveTask['stage'], string> = {
    incoming: t('待接收'), receiving: t('接收中'), done: t('接收完成'), rejected: t('已拒绝'),
  };
  return map[stage];
}

function stageLabel(modal: SendModal) {
  // 失败时优先显示具体原因，不再笼统写「发送失败」
  if (modal.stage === 'error' && modal.detail) return modal.detail;
  const map: Record<SendModal['stage'], string> = {
    waiting: t('等待对方接收'),
    accepted: t('对方已接收，正在发送'),
    transferring: t('正在发送'),
    done: t('发送完成'),
    cancelled: t('已撤销发送'),
    error: t('发送失败'),
  };
  return map[modal.stage];
}
</script>

<template>
  <div class="sync-page">
    <div class="page-head">
      <div>
        <h2>{{ t('数据同步') }}</h2>
        <p class="muted">{{ t('同一局域网内互传文件、文件夹、文字与剪贴板，无需联网') }}</p>
      </div>
    </div>

    <div class="entry-row">
      <button class="entry-card" :class="{ active: view === 'send' }" @click="view = 'send'">
        <svg class="entry-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21.5 2.5 10.8 13.2" /><path d="M21.5 2.5 15 21l-4.2-7.8L3 9l18.5-6.5Z" />
        </svg>
        <span class="entry-label">{{ t('发送') }}</span>
      </button>
      <button class="entry-card" :class="{ active: view === 'receive' }" @click="view = 'receive'">
        <svg class="entry-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 3v12" /><path d="m7 10 5 5 5-5" /><path d="M4 20h16" />
        </svg>
        <span class="entry-label">{{ t('接收') }}</span>
        <span v-if="incomingCount" class="entry-badge">{{ incomingCount }}</span>
      </button>
      <button class="entry-card" :class="{ active: view === 'settings' }" @click="view = 'settings'">
        <svg class="entry-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1.03 1.56V21a2 2 0 1 1-4 0v-.08a1.7 1.7 0 0 0-1.11-1.56 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.56-1.03H3a2 2 0 1 1 0-4h.08a1.7 1.7 0 0 0 1.56-1.11 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34H9a1.7 1.7 0 0 0 1.03-1.56V3a2 2 0 1 1 4 0v.08a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87V9a1.7 1.7 0 0 0 1.56 1.03H21a2 2 0 1 1 0 4h-.08a1.7 1.7 0 0 0-1.52 1.03Z" />
        </svg>
        <span class="entry-label">{{ t('设置') }}</span>
      </button>
    </div>

    <!-- 发送视图：左设备，右内容 -->
    <div v-show="view === 'send'" class="view-body">
      <div class="send-layout">
        <section class="card pad device-panel">
          <div class="section-title row-between">
            <span>{{ t('附近设备') }}</span>
            <button class="btn sm refresh-btn" :disabled="isActionActive('refresh-devices')" @click="refreshDevices">
              <svg class="refresh-icon" :class="{ spin: isActionActive('refresh-devices') }" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              {{ t('刷新') }}
            </button>
          </div>
          <div class="device-list">
            <div v-if="!deviceList.length" class="muted empty-hint">
              {{ t('未发现设备，请确认对方也打开了数据同步页，或在设置里手动添加') }}
            </div>
            <div
              v-for="d in deviceList"
              :key="d.id"
              class="device-card"
              :class="{ selected: selectedId === d.id, offline: !d.online, fav: d.favorite }"
              @click="selectDevice(d)"
            >
              <div class="device-main">
                <span class="dot" :class="{ on: d.online }"></span>
                <strong>{{ displayName(d) }}</strong>
                <span v-if="d.favorite" class="tag ok">{{ t('收藏') }}</span>
                <span class="muted tiny mono">{{ d.online ? d.ip : (t('离线') + ' · ' + d.last_ip) }}</span>
              </div>
              <div class="device-sub">
                <span class="muted tiny mono">{{ platformLabel(d.platform) }} · {{ shortFp(d.fingerprint) }}</span>
                <button class="tag-link favorite-btn" :class="{ active: d.favorite }" @click.stop="toggleFavorite(d)">
                  {{ d.favorite ? '★' : '☆' }} {{ d.favorite ? t('取消收藏') : t('收藏') }}
                </button>
              </div>
            </div>
          </div>
        </section>

        <section class="card pad compose-panel">
          <div class="section-title">{{ t('发送内容') }}</div>
          <div class="action-grid">
            <button class="action-card" :disabled="!selectedDevice || busy" @click="pickFiles(false)">
              <svg v-if="busy && activeAction === 'file'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">📄</span>
              <span>{{ t('文件') }}</span>
            </button>
            <button class="action-card" :disabled="!selectedDevice || busy" @click="pickFiles(true)">
              <svg v-if="busy && activeAction === 'folder'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">📁</span>
              <span>{{ t('文件夹') }}</span>
            </button>
            <button class="action-card" :disabled="!selectedDevice || busy" @click="sendText('clipboard')">
              <svg v-if="busy && activeAction === 'clipboard'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">📋</span>
              <span>{{ t('剪贴板') }}</span>
            </button>
            <button class="action-card" :disabled="!selectedDevice || busy" @click="sendText('text')">
              <svg v-if="busy && activeAction === 'text'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">✎</span>
              <span>{{ t('文字') }}</span>
            </button>
          </div>
          <div class="text-block">
            <label>{{ t('文字内容') }}</label>
            <textarea v-model="textInput" class="input textarea" :placeholder="t('输入要发送的文字')"></textarea>
          </div>
        </section>
      </div>
    </div>

    <!-- 接收视图：只展示最近一次接收任务的清单与进度 -->
    <div v-show="view === 'receive'" class="view-body">
      <section class="card pad receive-panel">
        <div class="section-title row-between">
          <span>{{ t('最近接收') }}</span>
          <div class="refresh-wrap">
            <button class="btn sm refresh-btn" :disabled="isActionActive('refresh-incoming')" @click="refreshIncoming">
              <svg class="refresh-icon" :class="{ spin: isActionActive('refresh-incoming') }" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              {{ t('刷新') }}
            </button>
            <button class="btn sm primary" v-if="incomingViewReq" @click="receiveAllRequests">
              {{ t('全部接收') }}
            </button>
          </div>
        </div>

        <div v-if="!activeReceive" class="muted empty-hint">{{ t('暂无待接收的内容') }}</div>

        <div v-else class="receive-task" :class="{ rejected: activeReceive.stage === 'rejected' }">
          <div class="request-head">
            <div class="request-from">
              <strong>{{ activeReceive.from }}</strong>
              <span class="muted tiny mono">{{ platformLabel(activeReceive.fromPlatform) }} · {{ shortFp(activeReceive.fingerprint) }}</span>
            </div>
            <span class="tag" :class="activeReceive.trusted ? 'ok' : 'warn'">
              {{ activeReceive.trusted ? t('可信设备') : t('首次连接') }}
            </span>
          </div>
          <div class="request-meta">
            <span>{{ kindLabel(activeReceive.kind) }}</span>
            <span class="muted">{{ fmtSize(activeReceive.total) }}</span>
            <span class="muted">{{ activeReceive.items.length }} {{ t('个文件') }}</span>
            <span class="tag" :class="{
              ok: activeReceive.stage === 'done',
              warn: activeReceive.stage === 'rejected',
              accent: activeReceive.stage === 'receiving',
            }">{{ receiveStageLabel(activeReceive.stage) }}</span>
          </div>
          <!-- 文件清单：自适应卡片剩余高度，超出时自身滚动 -->
          <div class="item-list">
            <div v-for="(item, index) in activeReceive.items" :key="`${activeReceive.id}-${index}`" class="item-row">
              <span class="item-status" :class="item.status">
                {{ item.status === 'done' ? '✓' : (item.status === 'receiving' ? '⇩' : '·') }}
              </span>
              <span class="item-name">{{ item.name }}</span>
              <span class="muted tiny">{{ fmtSize(item.size) }}</span>
            </div>
          </div>
          <!-- 整体进度条：待接收/已拒绝不显示 -->
          <div v-if="activeReceive.stage === 'receiving' || activeReceive.stage === 'done'" class="progress-bar receive-progress">
            <div class="progress-fill" :class="{ full: receiveTaskPercent >= 100 }" :style="{ width: receiveTaskPercent + '%' }"></div>
          </div>
          <div v-if="incomingViewReq" class="request-actions">
            <button class="btn sm" @click="rejectReceive(incomingViewReq)">{{ t('拒绝') }}</button>
          </div>
        </div>
      </section>
    </div>

    <!-- 设置视图 -->
    <div v-show="view === 'settings'" class="view-body">
      <div class="settings-grid">
        <div class="card pad">
          <div class="section-title">{{ t('本机信息') }}</div>
          <div class="field">
            <label>{{ t('设备名称（仅用于同步功能标识设备）') }}</label>
            <div v-if="editingName" class="row">
              <input v-model="nameDraft" class="input" @keyup.enter="saveName" />
              <button class="btn sm primary" @click="saveName">{{ t('保存') }}</button>
            </div>
            <div v-else class="row name-row">
              <strong>{{ status?.device_name || '—' }}</strong>
              <button class="btn sm" @click="editingName = true">{{ t('修改') }}</button>
            </div>
          </div>
          <div class="field" style="margin-top: 14px">
            <label>{{ t('本机指纹（首次连接对方确认时用）') }}</label>
            <div class="fingerprint-box mono">{{ status?.fingerprint_short || '—' }}</div>
          </div>
          <div class="field" style="margin-top: 14px">
            <label>{{ t('接收目录') }}</label>
            <div class="row">
              <input :value="status?.receive_dir || ''" class="input mono" readonly />
              <button class="btn sm" @click="setReceiveDir">{{ t('更改') }}</button>
            </div>
          </div>
        </div>

        <div class="card pad">
          <div class="section-title">{{ t('手动添加') }}</div>
          <p class="muted tiny">{{ t('发现不到设备时，输入对方 IP 添加；传输时核对指纹即可建立信任') }}</p>
          <div class="field" style="margin-top: 10px">
            <label>{{ t('对方 IP') }}</label>
            <input v-model="pairIp" class="input mono" placeholder="192.168.1.10" />
          </div>
          <button class="btn primary full" style="margin-top: 12px" :disabled="busy" @click="doPair">
            <svg v-if="busy && activeAction === 'pair'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
              <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
            </svg>
            {{ t('添加设备') }}
          </button>
        </div>
      </div>
    </div>

    <!-- 发送状态弹窗：等待接受 / 已接受文件列表 -->
    <div v-if="sendModal" class="modal-mask">
      <div class="modal send-modal">
        <h3>{{ sendModal.stage === 'accepted' || sendModal.stage === 'transferring' ? t('发送文件列表') : t('发送状态') }}</h3>
        <div class="modal-info">
          <div class="row-between"><span class="muted">{{ t('发送方') }}</span><strong>{{ status?.device_name || '—' }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('接收方') }}</span><strong>{{ sendModal.targetName }}</strong></div>
          <div class="row-between">
            <span class="muted">{{ t('状态') }}</span>
            <strong :class="{
              ok: sendModal.stage === 'accepted' || sendModal.stage === 'transferring' || sendModal.stage === 'done',
              warn: sendModal.stage === 'waiting',
              danger: sendModal.stage === 'error' || sendModal.stage === 'cancelled',
            }">
              {{ stageLabel(sendModal) }}
            </strong>
          </div>
        </div>
        <div v-if="sendModal.stage === 'accepted' || sendModal.stage === 'transferring'" class="modal-file-list">
          <div v-for="(item, index) in sendModal.items" :key="index" class="modal-file-row">
            <span>{{ item.name }}</span>
            <span class="muted tiny">{{ item.size ? fmtSize(item.size) : t('待发送') }}</span>
          </div>
        </div>
        <div v-if="sendModal.stage === 'transferring'" class="progress-bar">
          <div class="progress-fill" :style="{ width: (sendModal.total ? sendModal.sent / sendModal.total * 100 : 0) + '%' }"></div>
        </div>
        <div class="modal-actions">
          <button
            class="btn danger"
            :disabled="sendCanceling || sendModal.stage === 'done' || sendModal.stage === 'cancelled' || sendModal.stage === 'error'"
            @click="cancelSend"
          >{{ sendCanceling ? t('撤销中') : t('取消发送') }}</button>
          <button class="btn primary" @click="sendModal = null">{{ t('关闭') }}</button>
        </div>
      </div>
    </div>

    <!-- 接收请求摘要弹窗 -->
    <div v-if="requestDialog" class="modal-mask">
      <div class="modal request-modal">
        <h3>{{ t('收到传输请求') }}</h3>
        <div class="modal-info">
          <div class="row-between"><span class="muted">{{ t('发送方') }}</span><strong>{{ requestDialog.from }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('设备类型') }}</span><strong>{{ platformLabel(requestDialog.from_platform) }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('发送文件数量') }}</span><strong>{{ requestDialog.count }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('类型') }}</span><strong>{{ kindLabel(requestDialog.kind) }}</strong></div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="rejectReceive(requestDialog)">{{ t('拒绝') }}</button>
          <button class="btn" @click="openIncoming()">{{ t('查看清单') }}</button>
          <button class="btn primary" @click="approveReceive(requestDialog)">{{ t('接收') }}</button>
        </div>
      </div>
    </div>

    <!-- 指纹确认弹窗（TOFU） -->
    <div v-if="fpDialog" class="modal-mask">
      <div class="modal confirm-modal">
        <h3>{{ t('确认设备身份') }}</h3>
        <p>{{ t('首次连接，请核对对方显示的指纹是否一致，确认后建立信任') }}</p>
        <div class="fp-target">
          <div class="row-between"><span class="muted">{{ t('设备') }}</span><strong>{{ fpDialog.from }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('设备类型') }}</span><strong>{{ platformLabel(fpDialog.from_platform) }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('发送文件数量') }}</span><strong>{{ fpDialog.count }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('指纹') }}</span><strong class="mono">{{ shortFp(fpDialog.fingerprint) }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('类型') }}</span><strong>{{ kindLabel(fpDialog.kind) }} · {{ fmtSize(fpDialog.size) }}</strong></div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="rejectReceive(fpDialog)">{{ t('拒绝') }}</button>
          <button class="btn primary" @click="approveFingerprint">{{ t('信任并接收') }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 高度完全跟随 workspace 可用空间；不设固定下限，避免日志面板展开时撑破容器产生外层滚动条 */
.sync-page { height: 100%; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
.page-head { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 8px; flex-shrink: 0; }
.page-head h2 { font-size: 18px; font-weight: 600; }
.page-head p { font-size: 12px; margin-top: 4px; }
.entry-row { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; flex-shrink: 0; margin-bottom: 10px; }
.entry-card { position: relative; height: 50px; display: flex; align-items: center; justify-content: center; gap: 9px; border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1); border-radius: 10px; cursor: pointer; font-size: 13px; }
.entry-card:hover { border-color: var(--accent); }
.entry-card.active { border-color: var(--accent); background: var(--bg-3); color: var(--accent); }
.entry-icon { width: 20px; height: 20px; flex-shrink: 0; }
.entry-badge { position: absolute; top: 6px; right: 10px; min-width: 16px; height: 16px; padding: 0 4px; border-radius: 8px; background: var(--danger); color: #fff; font-size: 10px; line-height: 16px; }

/* 内容区优先吃满剩余高度；日志面板展开导致空间不足时，保留可读的最小高度，
   超出部分由 .sync-page 自身出页面级滚动条（不是把卡片压扁，也不是 workspace 外层滚动） */
.view-body { flex: 1; min-height: 300px; display: flex; overflow: hidden; }
.send-layout { flex: 1; min-width: 0; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 14px; }
.card.pad { padding: 16px; }
.device-panel, .compose-panel, .receive-panel { position: relative; min-width: 0; min-height: 0; display: flex; flex-direction: column; }
.section-title { font-size: 12px; color: var(--text-1); font-weight: 600; margin-bottom: 12px; }
.row-between { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
.field label, .text-block label { display: block; font-size: 11px; color: var(--text-2); margin-bottom: 5px; }
.hint { font-size: 11px; margin-top: 5px; display: block; }
.tiny { font-size: 10px; }
.row { display: flex; gap: 8px; align-items: center; }
.name-row strong { flex: 1; font-size: 14px; }
.full { width: 100%; }
.empty-hint { text-align: center; padding: 22px 8px; font-size: 12px; line-height: 1.6; }

.refresh-wrap { display: flex; align-items: center; gap: 8px; }
/* 冷却期保持普通箭头光标，与命令中心运行按钮一致（全局 .btn:disabled 是红色禁止） */
.btn.refresh-btn:disabled { cursor: default; opacity: .8; }
/* 刷新图标恒定占位：静止/旋转都用同一 SVG，避免切换文本↻导致按钮被撑大 */
.refresh-icon { width: 14px; height: 14px; flex-shrink: 0; }
.device-list { flex: 1; min-height: 0; overflow-y: auto; padding-right: 2px; }
.device-card { padding: 10px 12px; border: 1px solid var(--border); border-radius: 9px; margin-bottom: 8px; cursor: pointer; background: var(--bg-2); }
.device-card:hover { border-color: var(--accent); }
.device-card.selected { border-color: var(--accent); background: var(--bg-3); }
.device-card.fav { border-left: 3px solid var(--accent); }
.device-card.offline { opacity: .6; }
.device-main { display: flex; align-items: center; gap: 8px; min-width: 0; }
.device-main strong { font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.device-main .mono { margin-left: auto; white-space: nowrap; }
.device-sub { display: flex; align-items: center; gap: 10px; margin-top: 7px; }
.dot { width: 7px; height: 7px; border-radius: 50%; background: var(--text-2); flex-shrink: 0; }
.dot.on { background: var(--success); }
.tag { font-size: 10px; padding: 2px 7px; border-radius: 9px; background: var(--bg-3); color: var(--text-2); border: none; }
.tag.ok { color: var(--success); }
.tag.warn { color: var(--danger); }
.tag-link { margin-left: auto; font-size: 11px; color: var(--accent); background: transparent; border: none; cursor: pointer; }
.tag-link.active { color: var(--success); }

.action-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; justify-items: center; }
.action-card { width: 100%; height: 82px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-2); color: var(--text-1); display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; cursor: pointer; font-size: 12px; }
.action-card:hover:not(:disabled) { border-color: var(--accent); color: var(--accent); }
.action-card:disabled { opacity: .45; cursor: not-allowed; }
.action-icon { font-size: 24px; line-height: 1; }
.text-block { flex: 1; min-height: 0; display: flex; flex-direction: column; margin-top: 14px; }
.textarea { flex: 1; min-height: 78px; resize: none; }

.receive-panel { width: 100%; }
.count-pill { display: inline-block; margin-left: 7px; padding: 1px 7px; border-radius: 9px; background: var(--danger); color: #fff; font-size: 10px; }
/* 最近一次接收任务：撑满卡片高度，文件清单自适应剩余空间并自身滚动 */
.receive-task { flex: 1; min-height: 0; display: flex; flex-direction: column; padding: 12px; border: 1px solid var(--border); border-radius: 9px; background: var(--bg-2); }
.request-head { display: flex; justify-content: space-between; align-items: center; }
.request-from { display: flex; flex-direction: column; gap: 3px; }
.request-from strong { font-size: 13px; }
.request-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; margin-top: 8px; font-size: 12px; }
.item-list { flex: 1; min-height: 0; margin-top: 10px; overflow-y: auto; border: 1px solid var(--border-soft); border-radius: 8px; background: var(--bg-1); }
.item-row { display: flex; align-items: center; gap: 8px; padding: 7px 10px; border-bottom: 1px solid var(--border-soft); font-size: 12px; }
.item-row:last-child { border-bottom: none; }
.item-status { width: 14px; text-align: center; color: var(--text-2); flex-shrink: 0; }
.item-status.done { color: var(--success); }
.item-status.receiving { color: var(--accent); }
.item-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.progress-bar { height: 8px; margin-top: 10px; background: var(--bg-3); border-radius: 5px; overflow: hidden; flex-shrink: 0; }
.progress-fill { height: 100%; background: var(--accent); transition: width .2s; }
/* 接收进度条更细；到达 100% 时改为青色 */
.receive-progress { height: 4px; }
.receive-progress .progress-fill.full { background: #19b8b3; }
.tag.accent { color: var(--accent); }
.receive-task.rejected { opacity: .6; }
.request-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: 8px; margin-top: 12px; flex-shrink: 0; }

.settings-grid { flex: 1; min-width: 0; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 14px; overflow-y: auto; }
.fingerprint-box { padding: 10px 12px; border: 1px solid var(--accent); border-radius: 8px; background: var(--bg-2); color: var(--accent); font-size: 15px; letter-spacing: 2px; }
.btn.danger { color: var(--danger); border-color: var(--danger); }

.modal-mask { position: fixed; inset: 0; background: rgba(0,0,0,.55); display: flex; align-items: center; justify-content: center; z-index: 100; }
.modal { background: var(--bg-1); border: 1px solid var(--border); border-radius: 12px; padding: 22px; box-shadow: 0 18px 48px rgba(0,0,0,.35); }
.send-modal, .request-modal { width: 460px; }
.confirm-modal { width: 440px; }
.modal h3 { font-size: 16px; margin-bottom: 14px; }
.modal p { font-size: 13px; margin-bottom: 12px; line-height: 1.5; }
.modal-info { background: var(--bg-2); border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 9px; }
.modal-info > div { font-size: 12px; }
.modal-info .ok { color: var(--success); }
.modal-info .warn { color: var(--accent); }
.modal-info .danger { color: var(--danger); }
.modal-file-list { margin-top: 12px; max-height: 220px; overflow-y: auto; border: 1px solid var(--border); border-radius: 8px; }
.modal-file-row { display: flex; justify-content: space-between; gap: 10px; padding: 8px 10px; border-bottom: 1px solid var(--border-soft); font-size: 12px; }
.modal-file-row:last-child { border-bottom: none; }
.fp-target { background: var(--bg-2); border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 9px; }
.fp-target > div { font-size: 12px; }
.modal-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 18px; }

.spin { width: 14px; height: 14px; animation: btn-spin .7s linear infinite; }
.action-card .spin { width: 24px; height: 24px; }
@keyframes btn-spin { to { transform: rotate(360deg); } }
</style>
