import { ref, computed, markRaw, onMounted, onUnmounted, inject, provide } from 'vue';
import { listen } from '@tauri-apps/api/event';
import { open } from '@tauri-apps/plugin-dialog';
import { writeText, readText } from '@tauri-apps/plugin-clipboard-manager';
import { api } from './api';
import { t, setLocale } from './i18n';
import { useActionFeedback } from './useActionFeedback';
import type {
  SyncDevice,
  SyncStatus,
  SyncRequestEvent,
  SyncRequestItem,
  SyncReceivedEvent,
  SyncProgressEvent,
} from '../types';

export type SyncViewName = 'send' | 'receive' | 'settings';

export interface SendModal {
  transferId: string;
  ip: string;
  targetName: string;
  kind: string;
  items: SyncRequestItem[];
  stage: 'waiting' | 'accepted' | 'transferring' | 'done' | 'cancelled' | 'error';
  detail: string;
  sent: number;
  total: number;
}

export interface ReceiveTask {
  id: string;
  transferId: string;
  from: string;
  fromPlatform: string;
  fingerprint: string;
  trusted: boolean;
  kind: string;
  total: number;
  /** 文件明细：请求事件不再携带，接收页打开时才按需拉取（文件夹几百项时是关键优化） */
  items: { name: string; size: number; received: number; status: 'pending' | 'receiving' | 'done' }[];
  /** 清单里应有的条目数（明细未加载时也算得出总数） */
  itemCount: number;
  /** 明细是否已拉取 */
  itemsLoaded: boolean;
  /** 已落盘的文件数：完成判定按数量兜底，不依赖文件名与清单项一一匹配 */
  receivedCount: number;
  /** 接收页未打开时也累计进度，用于百分比 */
  receivedBytes: number;
  /** 每个文件上一次上报的已收字节，用于把进度换算成增量 */
  progressByName: Record<string, number>;
  stage: 'incoming' | 'receiving' | 'done' | 'rejected' | 'expired';
  /** 失效原因：等待超时 / 发送方掉线 */
  expireReason?: 'timeout' | 'offline';
}

export interface MediaEntry {
  uri: string;
  name: string;
  size: number;
  mime: string;
  video: boolean;
  /** 拍摄/添加时间（Unix 秒），用于按日期分组 */
  date: number;
  thumb: string;
}

/** 最近文件（非图片/视频） */
export interface RecentFileEntry {
  uri: string;
  name: string;
  size: number;
  mime: string;
  date: number;
}

/** 已安装应用（发 APK） */
export interface AppEntry {
  package: string;
  label: string;
  version: string;
  size: number;
  system: boolean;
  icon: string;
}

/* ------------------------------------------------------------------
 * 同步上下文：移动端页面被拆成多个子组件（接收页、发送状态弹层、相册、应用列表），
 * 目的是把「进度事件 → 整页重渲染」收敛成「只重绘弹层自己那几十个节点」。
 * 子组件通过 useSyncContext() 拿到与父组件同一份状态，避免几十个 prop 层层透传。
 * ------------------------------------------------------------------ */
export type SyncContext = ReturnType<typeof useSync>;
const SYNC_CTX_KEY = Symbol('runhelp-sync-ctx');

export function provideSyncContext(ctx: SyncContext) {
  provide(SYNC_CTX_KEY, ctx);
}

export function useSyncContext(): SyncContext {
  const ctx = inject<SyncContext | null>(SYNC_CTX_KEY, null);
  if (!ctx) throw new Error('useSyncContext() 只能用在 SyncViewMobile 的子组件里');
  return ctx;
}

/**
 * 桌面 / 移动端共用的数据同步逻辑（状态、事件监听、发送接收流程）。
 * 两个页面各自只负责模板与样式，业务逻辑只有这一份。
 */
export function useSync() {
  const notify = inject<(m: string) => void>('notify', () => {});
  const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);

  const view = ref<SyncViewName>('send');
  const status = ref<SyncStatus | null>(null);
  const settings = ref<Record<string, string>>({});
  const devices = ref<SyncDevice[]>([]);
  const selectedId = ref('');
  const textInput = ref('');
  const pairIp = ref('');
  const editingName = ref(false);
  const nameDraft = ref('');
  const busy = ref(false);
  const isMobile = ref(false);

  const activeAction = ref('');
  const { isActive: isActionActive, trigger: triggerAction, clear: clearAction } = useActionFeedback({ cooldownMs: 600 });

  const activeReceive = ref<ReceiveTask | null>(null);
  // 最近接收到的文件路径（用于「保存到下载目录」）
  const receivedPaths = ref<string[]>([]);
  const fpDialog = ref<SyncRequestEvent | null>(null);
  const requestDialog = ref<SyncRequestEvent | null>(null);
  const pendingDecision = ref<{ id: string; indices: number[] } | null>(null);

  const sendModal = ref<SendModal | null>(null);
  const sendCanceling = ref(false);
  /**
   * 发送状态弹窗的显示开关（与 sendModal 数据解耦）。
   * 关闭弹窗后数据继续保留，后台的完成/拒绝事件仍能记录并提示，
   * 否则用户一关弹窗就再也看不到对方接受或拒绝的结果。
   */
  const sendVisible = ref(false);

  /* ---------- 应用内「接收目录」 ---------- */
  const receiveDirOpen = ref(false);
  const receiveDirLoading = ref(false);
  const receiveDirItems = ref<{ name: string; size: number; date: number; uri: string; mime: string }[]>([]);
  const receiveDirLabel = ref('');

  /* ---------- 应用内相册（图片 + 视频混选） ---------- */
  const mediaPickerOpen = ref(false);
  const mediaLoading = ref(false);
  const mediaItems = ref<MediaEntry[]>([]);
  const mediaSelected = ref<string[]>([]);
  // 用户拒绝过相册权限后不再反复弹系统授权框，直接走系统选择器兜底
  let mediaPermissionDenied = false;

  /* ---------- 最近文件（「文件」入口） ---------- */
  const filePickerOpen = ref(false);
  const fileLoading = ref(false);
  const fileItems = ref<RecentFileEntry[]>([]);
  const fileSelected = ref<string[]>([]);

  /* ---------- 已安装应用（「应用」入口） ---------- */
  const appPickerOpen = ref(false);
  const appLoading = ref(false);
  const appItems = ref<AppEntry[]>([]);
  const appSelected = ref<string[]>([]);
  // 与 LocalSend 一致：默认排除系统应用、排除无法启动的应用
  const appExcludeSystem = ref(true);
  const appExcludeNonLaunchable = ref(true);

  let unlisteners: (() => void)[] = [];
  let autoTimer: number | undefined;

  const deviceList = computed(() => [...devices.value].sort((a, b) =>
    Number(b.favorite) - Number(a.favorite)
    || Number(b.online) - Number(a.online)
    || b.last_seen - a.last_seen
  ));
  const selectedDevice = computed(() => deviceList.value.find((d) => d.id === selectedId.value));
  const incomingCount = computed(() => activeReceive.value?.stage === 'incoming' ? 1 : 0);
  const displayName = (d: SyncDevice) => d.alias || d.name || d.id.slice(0, 8);
  const receiveTaskPercent = computed(() => {
    const task = activeReceive.value;
    if (!task || !task.total) return 0;
    const done = task.items.length
      ? task.items.reduce((sum, i) => sum + i.received, 0)
      : task.receivedBytes;
    return Math.min(100, Math.round(done / task.total * 100));
  });

  function logResult(direction: 'send' | 'receive', from: string, to: string, kind: string, content: string, result: string, detail = '') {
    void api.syncLogResult({
      from, to,
      kind: kindLabel(kind),
      content: content.slice(0, 60),
      result,
      detail: detail.slice(0, 120),
    }).catch(() => {});
  }

  function buildReceiveTask(req: SyncRequestEvent): ReceiveTask {
    // 明细一律延迟：事件里的 items 恒为空（后端已经瘦身），几百项清单在主线程建对象
    // 正好卡在弹层入场动画那一帧。真正打开接收页时用 loadReceiveItems 拉。
    const inline = req.items ?? [];
    return {
      id: req.id,
      transferId: req.transfer_id,
      from: req.from,
      fromPlatform: req.from_platform,
      fingerprint: req.fingerprint,
      trusted: req.trusted,
      kind: req.kind,
      total: req.size,
      items: inline.map((item) => ({ name: item.name, size: item.size, received: 0, status: 'pending' })),
      itemCount: req.count || inline.length,
      itemsLoaded: inline.length > 0,
      receivedCount: 0,
      receivedBytes: 0,
      progressByName: {},
      stage: 'incoming',
    };
  }

  /**
   * 收到文件的提示合并。
   * 文件夹动辄几百个文件，每收到一个就弹一次 Toast 会让界面持续掉帧（每条都要走一遍
   * 渲染与动画）；600ms 内的多次「收到」合并成一条，单文件仍是原来的措辞。
   */
  let recvNotice: { from: string; dest: string; count: number } | null = null;
  let recvNoticeTimer: ReturnType<typeof setTimeout> | null = null;
  function queueReceivedNotice(from: string, dest: string) {
    recvNotice = recvNotice ?? { from, dest, count: 0 };
    recvNotice.count += 1;
    if (recvNoticeTimer) clearTimeout(recvNoticeTimer);
    recvNoticeTimer = setTimeout(() => {
      const batch = recvNotice;
      recvNotice = null;
      recvNoticeTimer = null;
      if (!batch) return;
      notify(
        batch.count > 1
          ? t('已收到来自 {name} 的 {count} 个文件：{path}', {
              name: batch.from,
              count: String(batch.count),
              path: batch.dest,
            })
          : t('已收到 {name} 的文件：{path}', { name: batch.from, path: batch.dest }),
      );
    }, 600);
  }

  /** 拉取当前待接收任务的明细（接收页打开时调用，只拉一次） */
  async function loadReceiveItems() {
    const task = activeReceive.value;
    if (!task || task.itemsLoaded) return;
    task.itemsLoaded = true;
    try {
      const items = await api.syncPendingItems(task.transferId);
      // 拉取期间任务可能已被替换/取消
      const cur = activeReceive.value;
      if (!cur || cur.transferId !== task.transferId) return;
      const done = cur.stage === 'done';
      cur.items = items.map((item) => ({
        name: item.name,
        size: item.size,
        received: done ? item.size : 0,
        status: done ? ('done' as const) : ('pending' as const),
      }));
      cur.itemCount = cur.items.length || cur.itemCount;
      if (cur.items.length && cur.items.every((i) => i.status === 'done')) cur.stage = 'done';
    } catch (e) {
      task.itemsLoaded = false;
      notify(String(e));
    }
  }

  /** 在「下一帧才建接收清单」的窗口期内被取消/超时的传输，避免卡片迟到复活 */
  const cancelledTransferIds = new Set<string>();

  async function load() {
    try {
      status.value = await api.syncStatus();
      nameDraft.value = status.value.device_name;
      settings.value = await api.getSettings();
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
        // 顺序有讲究：先把弹层亮出来（用户感知的是「弹得快不快」），
        // 接收清单（文件夹可能几百条）放到下一帧再建，别和入场动画抢同一帧。
        if (isMobile.value) {
          void nextPaint().then(() => {
            // 期间被取消/超时就不要把卡片再塞回接收页
            if (!cancelledTransferIds.has(req.transfer_id)) activeReceive.value = buildReceiveTask(req);
          });
        } else {
          activeReceive.value = buildReceiveTask(req);
          // 桌面端不放弹层，接收页要立刻显示明细
          void loadReceiveItems();
        }
        if (!req.trusted) fpDialog.value = req;
        else requestDialog.value = req;
        // 桌面端直接切到接收页签；移动端不切，避免接收页渲染与弹层动画抢同一帧造成卡顿
        // （移动端页签带红点提示，点「查看清单」再切过去）。
        if (!isMobile.value) view.value = 'receive';
        // 弹层本身即是提示，不再叠一层 Toast：两者同时做动画会明显掉帧。
      })
    );
    unlisteners.push(
      await listen<{ name: string; received: number; total: number; status: string }>('sync://receive-progress', (e) => {
        const d = e.payload;
        const task = activeReceive.value;
        if (!task) return;
        if (task.stage === 'incoming') task.stage = 'receiving';
        // 明细可能还没拉（接收页没打开）：只累计字节，百分比照样能走。
        // 进度事件按文件上报的是该文件的绝对已收字节，所以按文件名记上一次的值算增量。
        const prev = task.progressByName[d.name] ?? 0;
        const nextBytes = d.status === 'done' ? d.total : d.received;
        if (nextBytes > prev) {
          task.receivedBytes += nextBytes - prev;
          task.progressByName[d.name] = nextBytes;
        }
        // 明细未加载时没有行可画，逐项更新可以整段跳过；
        // 但字节已经收满时必须收尾，否则明细一直不来就会永远停在「接收中」。
        if (!task.items.length) {
          if (task.total > 0 && task.receivedBytes >= task.total) task.stage = 'done';
          return;
        }
        const item = task.items.find((i) => i.name === d.name);
        if (!item) return;
        if (d.status === 'done') {
          item.received = item.size;
          item.status = 'done';
        } else {
          item.received = d.received;
          item.status = 'receiving';
        }
        if (task.items.every((i) => i.status === 'done')) task.stage = 'done';
      })
    );
    unlisteners.push(
      await listen<SyncReceivedEvent>('sync://received', async (e) => {
        const d = e.payload;
        const task = activeReceive.value;
        if (task) {
          // 完成判定不能只靠「文件名和清单项对上」：发送端 rel path 与清单展示名
          // 一旦不一致（历史上就是缓存重名加 _1 后缀），清单项永远等不到自己的
          // done 事件，界面会一直停在「接收中」，而文件其实已经全部落盘。
          // 所以按「已收文件数 ≥ 清单条目数」兜底收尾。
          if (d.kind === 'file') task.receivedCount += 1;
          const item = task.items.find((i) => i.name === d.name)
            ?? task.items.find((i) => i.status !== 'done');
          if (item) { item.received = item.size; item.status = 'done'; }
          const expected = task.items.length || task.itemCount;
          const allItemsDone = task.items.length > 0 && task.items.every((i) => i.status === 'done');
          if (expected > 0 && (allItemsDone || task.receivedCount >= expected)) task.stage = 'done';
          // 文字/剪贴板是一问一答，没有逐项进度，收到即完成
          if (d.kind === 'text' || d.kind === 'clipboard') task.stage = 'done';
        }
        if (d.kind === 'text' || d.kind === 'clipboard') {
          try {
            await writeText(d.text);
            notify(t('已收到来自 {name} 的内容，已放入剪贴板', { name: d.from }));
          } catch {
            notify(t('已收到来自 {name} 的文字', { name: d.from }));
          }
          logResult('receive', d.from, status.value?.device_name || '本机', d.kind, d.text, t('成功'));
        } else {
          addReceivedPath(d.path);
          // 移动端实际落点是「下载/RunHelp」或用户选的文件夹，不能再把应用私有目录报给用户
          const dest = receiveDestLabel();
          queueReceivedNotice(d.from, dest || d.path);
          logResult('receive', d.from, status.value?.device_name || '本机', d.kind, d.name || d.path, t('成功'), dest || d.path);
          void autoExport(d.path);
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
          logResult('send', status.value?.device_name || '本机', modal.targetName, modal.kind,
            modal.items.map((i) => i.name).join('、'), t('成功'));
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
    unlisteners.push(
      await listen<{ transfer_id: string }>('sync://request-cancelled', (e) => {
        const tid = e.payload.transfer_id;
        cancelledTransferIds.add(tid);
        if (activeReceive.value?.transferId === tid) activeReceive.value = null;
        if (requestDialog.value?.transfer_id === tid) requestDialog.value = null;
        if (fpDialog.value?.transfer_id === tid) fpDialog.value = null;
        if (pendingDecision.value) pendingDecision.value = null;
      })
    );
    unlisteners.push(
      await listen<{ transfer_id: string; reason?: string }>('sync://request-timeout', (e) => {
        const tid = e.payload.transfer_id;
        cancelledTransferIds.add(tid);
        const offline = e.payload.reason === 'offline';
        // 超时/发送方掉线：保留卡片但标记失效，清掉弹层，避免看起来像传输卡住。
        if (activeReceive.value?.transferId === tid) {
          activeReceive.value.stage = 'expired';
          activeReceive.value.expireReason = offline ? 'offline' : 'timeout';
        }
        if (requestDialog.value?.transfer_id === tid) requestDialog.value = null;
        if (fpDialog.value?.transfer_id === tid) fpDialog.value = null;
        if (pendingDecision.value) pendingDecision.value = null;
        notify(offline ? t('发送方已离线，本次接收已取消') : t('对方长时间未确认，本次接收已取消'));
      })
    );
    // 后台主动探测完成后，仅拉取一次设备列表（不再次触发探测，避免循环）
    unlisteners.push(
      await listen('sync://devices-updated', () => { void refreshDeviceList(); })
    );
    isMobile.value = await api.appIsMobile().catch(() => false);
    await load();
    // 打开同步页时主动探测近期设备，立即刷新在线状态（LocalSend 式秒级判断）
    void probeRecent();
    void pullPending(true);
    // 定时只做轻量列表拉取；探测改由后台异步完成 + 事件驱动，不阻塞 UI/IPC
    autoTimer = window.setInterval(() => { void refreshDeviceList(); }, 5000);
  });

  onUnmounted(() => {
    unlisteners.forEach((fn) => fn());
    if (autoTimer) clearInterval(autoTimer);
    clearAction();
  });

  async function probeRecent() {
    try {
      // command 现在立即返回，探测在后端后台线程完成并通过 sync://devices-updated 通知
      await api.syncProbeRecent(10);
    } catch {
      /* 探测失败不影响普通发现流程 */
    }
  }

  // 轻量：只拉列表，不触发探测。定时器与探测完成事件都用它。
  // 内容没变就不赋值：定时器每 5 秒硬赋一次数组会让整棵页面（含所有弹层）重渲染，
  // 弹层动画期间正好撞上就会明显掉帧。这里按字段签名比对，只在真的变了才更新。
  function deviceSignature(list: SyncDevice[]): string {
    return list
      .map((d) => `${d.id}|${d.alias}|${d.name}|${d.online ? 1 : 0}|${d.ip}|${d.last_ip}|${d.platform}|${d.favorite ? 1 : 0}`)
      .join(',');
  }

  async function refreshDeviceList(manual = false) {
    try {
      const next = await api.syncListDevices();
      if (manual || deviceSignature(next) !== deviceSignature(devices.value)) {
        devices.value = next;
      }
    } catch (e) {
      if (manual) notify(String(e));
    }
  }

  // 手动刷新：先拉列表立即反馈，再触发后台探测（完成后事件会再刷新一次）
  async function doRefreshDevices(manual: boolean) {
    await refreshDeviceList(manual);
    void probeRecent();
  }

  function refreshDevices() {
    triggerAction('refresh-devices', () => { void doRefreshDevices(true); });
  }

  async function pullPending(prompt = false) {
    try {
      const pending = await api.syncListPending();
      if (!pending.length) return;
      const req = pending[pending.length - 1];
      if (!activeReceive.value || activeReceive.value.id !== req.id) {
        activeReceive.value = buildReceiveTask(req);
        // 桌面端没有「打开接收页」这个动作，直接补明细；移动端等接收页挂载再拉
        if (!isMobile.value) void loadReceiveItems();
        if (prompt) {
          if (!req.trusted) fpDialog.value = req;
          else requestDialog.value = req;
        }
      }
    } catch (e) {
      notify(String(e));
    }
  }

  function refreshIncoming() {
    triggerAction('refresh-incoming', () => { void pullPending(); });
  }

  async function saveName() {
    if (!nameDraft.value.trim()) return;
    try {
      await api.syncSetDeviceName(nameDraft.value.trim());
      status.value = await api.syncStatus();
      editingName.value = false;
      // 关弹层前失焦，避免输入框隐藏后软键盘悬着不收
      (document.activeElement as HTMLElement | null)?.blur?.();
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

  async function openReceiveDir() {
    await openSavedDir();
  }

  function mobileTheme() {
    return settings.value.mobile_theme || 'soft-light';
  }

  async function changeTheme(theme: string) {
    // 先应用本地主题，持久化/系统栏失败不阻断切换
    document.documentElement.setAttribute('data-theme', theme);
    settings.value.mobile_theme = theme;
    try {
      await api.setSetting('mobile_theme', theme);
    } catch (e) {
      notify(String(e));
    }
    try {
      await api.mobileSetTheme(theme);
    } catch {
      /* 系统栏颜色失败不影响页面主题 */
    }
  }

  async function changeLanguage(value: string) {
    setLocale(value);
    settings.value.language = value;
    try {
      await api.setSetting('language', value);
    } catch (e) {
      notify(String(e));
    }
  }

  /** 接收后自动保存到可见目录（默认开启） */
  /** 修改接收导出目录：选择系统文件夹（SAF），之后收到的文件自动复制过去。 */
  async function changeReceiveDir() {
    try {
      // 带上当前已选目录，系统选择器直接定位到那里，不用每次从根目录翻
      const folder = await api.mobilePickFolder(settings.value.mobile_receive_tree || '');
      settings.value.mobile_receive_tree = folder.uri;
      settings.value.mobile_receive_dir_name = folder.name;
      await api.setSetting('mobile_receive_tree', folder.uri);
      await api.setSetting('mobile_receive_dir_name', folder.name);
      notify(t('接收目录已更新为 {name}', { name: folder.name }));
    } catch (e) {
      if (!String(e).toLowerCase().includes('cancel')) notify(String(e));
    }
  }

  /** 恢复默认：不再复制到外部目录，文件留在应用私有目录。 */
  async function resetReceiveDir() {
    settings.value.mobile_receive_tree = '';
    settings.value.mobile_receive_dir_name = '';
    try {
      await api.setSetting('mobile_receive_tree', '');
      await api.setSetting('mobile_receive_dir_name', '');
      // 回读一次：本地赋值只是乐观更新，若写库失败（或别处又写回），界面会立刻暴露真相，
      // 而不是「恢复默认后还显示原来的目录」。
      settings.value = await api.getSettings();
    } catch (e) {
      notify(String(e));
    }
    notify(t('已恢复默认接收位置，之后收到的文件保存到「下载/RunHelp」'));
  }

  function addReceivedPath(path: string) {
    if (!path) return;
    receivedPaths.value = [...receivedPaths.value.slice(-49), path];
  }

  /**
   * 接收后自动导出到可见位置：
   * - 用户选过 SAF 文件夹 → 复制到该文件夹
   * - 否则若未关闭自动导出 → 保存到 系统下载/RunHelp（卓易通等兼容层也能在文件管理器找到）
   */
  async function autoExport(path: string) {
    if (!isMobile.value || !path) return;
    // 不再有「是否自动保存」开关：收到的文件一律复制到用户可见的位置。
    // 两个落点：用户选的 SAF 文件夹，或系统「下载/RunHelp」。
    const tree = settings.value.mobile_receive_tree || '';
    const dest = receiveDestLabel();
    try {
      if (tree) {
        await api.mobileExportToDir(tree, [path]);
      } else {
        await api.mobileSaveToDownloads([path]);
      }
    } catch (e) {
      // 导出失败时明确告知，否则用户会以为文件真的存到了提示的位置
      notify(t('已接收，但保存到 {path} 失败：{error}', {
        path: dest || t('应用私有目录'),
        error: String(e),
      }));
    }
  }

  /**
   * 接收后的实际落点文案（移动端）：
   * - 选过 SAF 文件夹 → 该文件夹名
   * - 否则自动保存开启 → 「下载/RunHelp」
   * - 都关掉 → 空串（调用方回落到应用私有目录路径）
   * 桌面端返回空串。
   */
  function receiveDestLabel() {
    if (!isMobile.value) return '';
    const treeName = settings.value.mobile_receive_dir_name;
    if (treeName) return treeName;
    return t('下载/RunHelp');
  }

  /**
   * 打开接收文件的所在位置（移动端）。
   *
   * 不再让系统文件管理器去开「按路径拼出来的目录 URI」：恢复默认接收位置后目录是
   * 「下载/RunHelp」，这个 URI 既没授权给我们也不是文件管理器认可的 tree URI，
   * 鸿蒙/EMUI 上会开出白屏页并卡死（用户手动选一次目录后又正常，正是因为它改走了
   * 有持久授权的 tree URI）。现在改成应用内列表：SAF 目录直接查子项，默认目录查
   * MediaStore，点条目打开文件；系统文件管理器只作为次要入口，且只开存储根。
   */
  async function openSavedDir() {
    if (!isMobile.value) {
      const dir = status.value?.receive_dir;
      if (!dir) return;
      try {
        await api.openFolder(dir);
      } catch (e) {
        notify(String(e));
      }
      return;
    }
    receiveDirOpen.value = true;
    if (!receiveDirItems.value.length) {
      receiveDirLoading.value = true;
      // 先让弹层画出来，再查目录（文件夹几百项时查询本身也要时间）
      await nextPaint();
      await loadSavedFiles();
    }
  }

  async function loadSavedFiles() {
    if (!isMobile.value) return;
    receiveDirLoading.value = true;
    try {
      const res = await api.mobileListSavedFiles(settings.value.mobile_receive_tree || '');
      receiveDirItems.value = markRaw(res.items ?? []);
      receiveDirLabel.value = res.dir || receiveDestLabel();
    } catch (e) {
      notify(String(e));
    } finally {
      receiveDirLoading.value = false;
    }
  }

  async function openSavedFile(item: { uri: string; mime: string }) {
    try {
      await api.mobileOpenSavedFile(item.uri, item.mime || '');
    } catch (e) {
      notify(String(e));
    }
  }

  async function openInFileManager() {
    try {
      await api.mobileOpenFmRoot(settings.value.mobile_receive_tree || '');
    } catch (e) {
      notify(String(e));
    }
  }

  /** 把最近接收的文件批量保存到系统「下载/RunHelp」（自动保存关闭时的兜底）。 */
  async function saveReceivedToDownloads() {
    if (!receivedPaths.value.length) {
      notify(t('暂无可保存的文件'));
      return;
    }
    try {
      const n = await api.mobileSaveToDownloads(receivedPaths.value);
      notify(n ? t('已保存 {count} 个文件到 下载/RunHelp', { count: String(n) }) : t('保存失败'));
    } catch (e) {
      notify(String(e));
    }
  }

  async function changeKeepAlive(enabled: boolean) {
    try {
      await api.setSetting('mobile_keepalive', enabled ? '1' : '0');
      const running = status.value?.running;
      if (enabled && running) await api.mobileStartKeepalive();
      if (!enabled && running) await api.mobileStopKeepalive();
      settings.value.mobile_keepalive = enabled ? '1' : '0';
      notify(enabled ? t('已开启后台保活') : t('已关闭后台保活，切后台后同步可能中断'));
    } catch (e) {
      notify(String(e));
    }
  }

  async function doPair(): Promise<boolean> {
    if (!pairIp.value.trim()) {
      notify(t('请填写对方 IP'));
      return false;
    }
    busy.value = true;
    activeAction.value = 'pair';
    try {
      const device = await api.syncPairDevice(pairIp.value.trim());
      selectedId.value = device.id;
      await doRefreshDevices(false);
      pairIp.value = '';
      notify(t('已添加设备，传输时确认对方指纹即可建立信任'));
      return true;
    } catch (e) {
      notify(t('配对失败: {error}', { error: String(e) }));
      return false;
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

  async function removeDevice(device: SyncDevice) {
    const ok = await confirm(
      t('确定删除设备「{name}」？将清除其信任与收藏记录；下次在线需重新确认指纹。', { name: displayName(device) }),
      { title: t('删除设备'), danger: true, confirmText: t('删除') }
    );
    if (!ok) return;
    try {
      await api.syncRemoveDevice(device.fingerprint);
      if (selectedId.value === device.id) selectedId.value = '';
      notify(t('已移除设备'));
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

  /**
   * 等到「真正画过一帧之后再继续」。
   * 弹层入场是 CSS 过渡，紧接着同步跑一次重 IPC（序列化上百个路径）会占住主线程，
   * 过渡的第一帧就被吞掉了——这就是「发文件夹时状态页卡一下」的来源。
   */
  const nextPaint = () =>
    new Promise<void>((resolve) => requestAnimationFrame(() => requestAnimationFrame(() => resolve())));

  function newTransferId() {
    return `x${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  }

  function basename(path: string) {
    const parts = path.split(/[\\/]/).filter(Boolean);
    return parts[parts.length - 1] || path;
  }

  function setSendFailure(modal: SendModal, msg: string) {
    if (modal.stage === 'cancelled' || modal.stage === 'done') return;
    if (msg.includes('撤销')) {
      modal.stage = 'cancelled';
      modal.detail = t('已撤销发送');
      logResult('send', status.value?.device_name || '本机', modal.targetName, modal.kind,
        modal.items.map((i) => i.name).join('、'), t('已撤销'), modal.detail);
    } else {
      modal.stage = 'error';
      modal.detail = msg.includes('拒绝') ? t('对方已拒绝接收')
        : msg.includes('时限') || msg.includes('超时') ? t('对方未在时限内接收')
        : msg.includes('信任') ? t('对方尚未信任本机')
        : msg;
      logResult('send', status.value?.device_name || '本机', modal.targetName, modal.kind,
        modal.items.map((i) => i.name).join('、'), t('失败'), modal.detail);
    }
    notify(modal.detail);
  }

  function handleSendError(e: unknown) {
    const modal = sendModal.value;
    if (!modal) {
      // 弹层还没建立就失败（选择器/读取阶段）：直接提示，避免静默无反馈
      notify(String(e));
      return;
    }
    setSendFailure(modal, String(e));
  }

  async function sendText(kind: 'text' | 'clipboard') {
    const target = selectedDevice.value;
    if (!target) {
      notify(t('请先选择设备'));
      return;
    }
    if (!target.online) {
      notify(t('该设备当前离线'));
      return;
    }
    if (busy.value) return;
    let text = textInput.value;
    if (kind === 'clipboard') {
      try {
        text = await readText();
      } catch {
        // 空剪贴板在部分 ROM 上直接抛异常，统一按「空」提示，不暴露底层错误
        notify(t('剪贴板为空'));
        return;
      }
      if (!text || !text.trim()) {
        notify(t('剪贴板为空'));
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
    sendVisible.value = true;
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

  /** 建立发送弹层并真正发起文件传输（文件 / 相册选择共用）。 */
  async function sendPaths(
    paths: string[],
    displayNames: string[] | null,
    target: SyncDevice,
    kind: 'file' | 'folder',
  ) {
    // markRaw：文件夹可能几百个条目，没必要让 Vue 深度代理它们（弹层不逐条渲染）
    const items = markRaw(paths.map((p, i) => ({ name: displayNames?.[i] ?? basename(p), size: 0 })));
    const transferId = newTransferId();
    sendModal.value = {
      transferId,
      ip: target.ip,
      targetName: displayName(target),
      kind,
      items,
      stage: 'waiting',
      detail: '',
      sent: 0,
      total: paths.length,
    };
    sendVisible.value = true;
    // 先让弹层画完入场动画，再发起（序列化几百个路径 + 后端建清单）
    await nextPaint();
    await api.syncSendFiles(target.ip, paths, transferId, displayNames ?? undefined);
  }

  /* ---------- 应用内相册混选 ---------- */

  /** 打开相册：先查 MediaStore（图片+视频混排），读不到时退回系统选择器。 */
  async function openMediaPicker() {
    const target = selectedDevice.value;
    if (!target) {
      notify(t('请先选择设备'));
      return;
    }
    if (!target.online) {
      notify(t('该设备当前离线'));
      return;
    }
    if (busy.value) return;
    if (mediaPermissionDenied) {
      await pickFiles('media');
      return;
    }
    mediaItems.value = [];
    mediaSelected.value = [];
    mediaPickerOpen.value = true;
    mediaLoading.value = true;
    try {
      mediaItems.value = await api.mobileListMedia(200);
    } catch {
      // 权限被拒或相册不可读：本次之后直接走系统选择器
      mediaPermissionDenied = true;
      mediaItems.value = [];
    } finally {
      mediaLoading.value = false;
    }
    if (!mediaItems.value.length) {
      // 没有权限 / 相册为空：退回系统选择器，至少还能选文件
      mediaPickerOpen.value = false;
      await pickFiles('media');
    }
  }

  function toggleMedia(uri: string) {
    const i = mediaSelected.value.indexOf(uri);
    if (i >= 0) mediaSelected.value.splice(i, 1);
    else mediaSelected.value.push(uri);
  }

  /** 相册确认：把选中的 content:// URI 复制成真实文件后发送。 */
  async function confirmMedia() {
    const target = selectedDevice.value;
    if (!target || !mediaSelected.value.length) return;
    const picked = mediaItems.value.filter((m) => mediaSelected.value.includes(m.uri));
    const uris = picked.map((m) => m.uri);
    const names = picked.map((m) => m.name);
    mediaPickerOpen.value = false;
    busy.value = true;
    activeAction.value = 'media';
    try {
      const resolved = await api.mobileResolveUris(uris);
      if (!resolved.length) {
        notify(t('所选内容为空或无法读取'));
        return;
      }
      await sendPaths(
        resolved.map((r) => r.path),
        resolved.map((r, i) => names[i] || r.name),
        target,
        'file',
      );
    } catch (e) {
      if (!String(e).toLowerCase().includes('cancel')) notify(String(e));
    } finally {
      busy.value = false;
      activeAction.value = '';
    }
  }

  /* ---------- 最近文件 ---------- */

  /** 点「文件」：直接给最近用过的文件，省去在系统目录里翻找。 */
  async function openFilePicker() {
    const target = selectedDevice.value;
    if (!target) {
      notify(t('请先选择设备'));
      return;
    }
    if (!target.online) {
      notify(t('该设备当前离线'));
      return;
    }
    if (busy.value) return;
    fileItems.value = [];
    fileSelected.value = [];
    filePickerOpen.value = true;
    fileLoading.value = true;
    try {
      fileItems.value = await api.mobileListRecentFiles(150);
    } catch {
      fileItems.value = [];
    } finally {
      fileLoading.value = false;
    }
  }

  function toggleFile(uri: string) {
    const i = fileSelected.value.indexOf(uri);
    if (i >= 0) fileSelected.value.splice(i, 1);
    else fileSelected.value.push(uri);
  }

  /** 最近文件确认：与相册一致，先把 content:// 复制成真实文件。 */
  async function confirmFilePick() {
    const target = selectedDevice.value;
    if (!target || !fileSelected.value.length) return;
    const picked = fileItems.value.filter((f) => fileSelected.value.includes(f.uri));
    const uris = picked.map((f) => f.uri);
    const names = picked.map((f) => f.name);
    filePickerOpen.value = false;
    busy.value = true;
    activeAction.value = 'file';
    try {
      const resolved = await api.mobileResolveUris(uris);
      if (!resolved.length) {
        notify(t('所选内容为空或无法读取'));
        return;
      }
      await sendPaths(
        resolved.map((r) => r.path),
        resolved.map((r, i) => names[i] || r.name),
        target,
        'file',
      );
    } catch (e) {
      if (!String(e).toLowerCase().includes('cancel')) notify(String(e));
    } finally {
      busy.value = false;
      activeAction.value = '';
    }
  }

  /** 浏览全部文件：系统 SAF，起点为存储根目录（不再是上次的下载目录）。 */
  async function browseFiles() {
    const target = selectedDevice.value;
    if (!target || busy.value) return;
    filePickerOpen.value = false;
    busy.value = true;
    activeAction.value = 'file';
    try {
      const uris = await api.mobilePickFiles();
      if (!uris.length) return;
      const resolved = await api.mobileResolveUris(uris);
      if (!resolved.length) {
        notify(t('所选内容为空或无法读取'));
        return;
      }
      await sendPaths(resolved.map((r) => r.path), resolved.map((r) => r.rel || r.name), target, 'file');
    } catch (e) {
      if (!String(e).toLowerCase().includes('cancel')) notify(String(e));
    } finally {
      busy.value = false;
      activeAction.value = '';
    }
  }

  /* ---------- 已安装应用（发 APK） ---------- */

  async function openAppPicker() {
    const target = selectedDevice.value;
    if (!target) {
      notify(t('请先选择设备'));
      return;
    }
    if (!target.online) {
      notify(t('该设备当前离线'));
      return;
    }
    if (busy.value) return;
    appSelected.value = [];
    appPickerOpen.value = true;
    await loadApps();
  }

  /** 按当前筛选条件重新读取应用列表（切换「排除系统应用」等选项时调用）。 */
  async function loadApps() {
    appLoading.value = true;
    try {
      appItems.value = await api.mobileListApps(appExcludeSystem.value, appExcludeNonLaunchable.value);
    } catch (e) {
      appItems.value = [];
      notify(String(e));
    } finally {
      appLoading.value = false;
    }
  }

  function toggleApp(pkg: string) {
    const i = appSelected.value.indexOf(pkg);
    if (i >= 0) appSelected.value.splice(i, 1);
    else appSelected.value.push(pkg);
  }

  /** 确认发送：原生把 APK 复制到缓存目录后按普通文件发送。 */
  async function confirmApps() {
    const target = selectedDevice.value;
    if (!target || !appSelected.value.length) return;
    appPickerOpen.value = false;
    busy.value = true;
    activeAction.value = 'app';
    try {
      const apks = await api.mobilePrepareApks(appSelected.value);
      if (!apks.length) {
        notify(t('没有可发送的安装包'));
        return;
      }
      await sendPaths(apks.map((a) => a.path), apks.map((a) => a.name), target, 'file');
    } catch (e) {
      notify(String(e));
    } finally {
      busy.value = false;
      activeAction.value = '';
    }
  }

  /** 六宫格「媒体」入口：移动端走应用内相册，桌面端走系统选择器。 */
  function pickMediaEntry() {
    if (isMobile.value) void openMediaPicker();
    else void pickFiles('media');
  }

  /** 六宫格「文件」「应用」入口：移动端走应用内列表，桌面端走系统选择器。 */
  function pickFileEntry() {
    if (isMobile.value) void openFilePicker();
    else void pickFiles('file');
  }

  function pickAppEntry() {
    if (isMobile.value) void openAppPicker();
    else void pickFiles('app');
  }

  /**
   * 选择并发送文件。kind 决定选择器：
   * - file：普通文档选择器
   * - media：移动端走原生相册（ACTION_PICK），桌面端按图片/视频过滤
   * - app：安装包（Android MIME application/vnd.android.package-archive，桌面 .apk）
   * - folder：移动端走原生 SAF 目录树选择（dialog 插件在 Android 不支持目录）
   */
  async function pickFiles(kind: 'file' | 'media' | 'app' | 'folder') {
    const target = selectedDevice.value;
    if (!target) {
      notify(t('请先选择设备'));
      return;
    }
    if (!target.online) {
      notify(t('该设备当前离线'));
      return;
    }
    if (busy.value) return;
    const isFolder = kind === 'folder';
    busy.value = true;
    activeAction.value = kind;
    try {
      let paths: string[] = [];
      let displayNames: string[] | null = null;

      if (isMobile.value && (kind === 'media' || isFolder)) {
        // 相册 / 文件夹：系统 dialog 插件在 Android 上不支持这两个模式，走原生选择器
        const uris = isFolder
          ? [(await api.mobilePickFolder()).uri]
          : await api.mobilePickMedia();
        if (!uris.length) return;
        const resolved = await api.mobileResolveUris(uris);
        if (!resolved.length) {
          notify(t('所选内容为空或无法读取'));
          return;
        }
        paths = resolved.map((r) => r.path);
        displayNames = resolved.map((r) => r.rel || r.name);
      } else {
        let options: Parameters<typeof open>[0];
        if (isFolder) {
          options = { directory: true, recursive: true, multiple: false, title: t('选择文件夹') };
        } else if (kind === 'app') {
          options = {
            multiple: true,
            title: t('选择安装包'),
            filters: [{ name: t('安装包'), extensions: isMobile.value
              ? ['application/vnd.android.package-archive'] : ['apk'] }],
          };
        } else if (kind === 'media') {
          options = {
            multiple: true,
            title: t('选择媒体'),
            filters: [{ name: t('图片与视频'), extensions: isMobile.value
              ? ['image/*', 'video/*'] : ['png', 'jpg', 'jpeg', 'gif', 'webp', 'mp4', 'mov'] }],
          };
        } else {
          options = { multiple: true, title: t('选择文件') };
        }
        const result = await open(options);
        if (!result) return;
        paths = (Array.isArray(result) ? result : [result]).filter((p): p is string => typeof p === 'string');
        if (!paths.length) return;
        if (paths.some((p) => p.startsWith('content://'))) {
          const resolved = await api.mobileResolveUris(paths);
          if (!resolved.length) {
            notify(t('所选内容为空或无法读取'));
            return;
          }
          paths = resolved.map((r) => r.path);
          displayNames = resolved.map((r) => r.rel || r.name);
        }
      }

      await sendPaths(paths, displayNames, target, isFolder ? 'folder' : 'file');
    } catch (e) {
      // 用户取消选择不算失败，静默返回
      if (!String(e).toLowerCase().includes('cancel')) handleSendError(e);
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
    } catch (e) {
      notify(String(e));
    } finally {
      sendCanceling.value = false;
    }
  }

  function allIndices(req: SyncRequestEvent) {
    // 清单明细已不在事件里，索引按数量推导（后端按 0..count-1 接收）
    const n = req.items.length || req.count || 0;
    return Array.from({ length: n }, (_, i) => i);
  }

  function requestDecision(req: SyncRequestEvent, indices: number[]) {
    if (!req.trusted) {
      pendingDecision.value = { id: req.id, indices };
      fpDialog.value = req;
      return;
    }
    void doRespond(req, indices);
  }

  async function doRespond(req: SyncRequestEvent, indices: number[]) {
    const content = req.items.length
      ? req.items.map((i) => i.name).join('、')
      : t('{count} 个文件', { count: String(req.count || 0) });
    logResult('receive', req.from, status.value?.device_name || '本机', req.kind,
      content, indices.length ? t('接收中') : t('拒绝'));
    if (fpDialog.value?.id === req.id) fpDialog.value = null;
    if (requestDialog.value?.id === req.id) requestDialog.value = null;
    if (pendingDecision.value?.id === req.id) pendingDecision.value = null;
    if (activeReceive.value?.id === req.id) {
      activeReceive.value.stage = indices.length ? 'receiving' : 'rejected';
    }
    // 接受后才切到接收页看进度：弹层出现那一刻不切，避免动画掉帧
    if (indices.length) view.value = 'receive';
    // 应答后后端就把待接收项移出内存了，明细必须在应答前拿到，否则接收页只能空着
    if (activeReceive.value?.id === req.id && !activeReceive.value.itemsLoaded) {
      await loadReceiveItems();
    }
    try {
      await api.syncRespondSelected(req.id, indices);
    } catch (e) {
      notify(String(e));
    }
  }

  function approveReceive(req: SyncRequestEvent) {
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

  function receiveAllRequests() {
    const req = incomingViewReq.value;
    if (req) requestDecision(req, allIndices(req));
  }

  const incomingViewReq = computed<SyncRequestEvent | null>(() => {
    const task = activeReceive.value;
    if (!task || task.stage !== 'incoming') return null;
    return {
      id: task.id,
      transfer_id: task.transferId,
      from: task.from,
      from_platform: task.fromPlatform,
      fingerprint: task.fingerprint,
      trusted: task.trusted,
      kind: task.kind,
      name: task.items.length ? task.items.map((i) => i.name).join(', ') : '',
      size: task.total,
      count: task.itemCount,
      items: task.items.map((i) => ({ name: i.name, size: i.size })),
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
    const p = (platform || '').toLowerCase();
    const map: Record<string, string> = {
      windows: 'Windows', win32: 'Windows', linux: 'Linux', darwin: 'macOS',
      macos: 'macOS', android: 'Android', ios: 'iOS',
    };
    if (map[p]) return map[p];
    // 探测兜底值/空值统一显示「未知设备」，不要把 unknown 原样抛给用户
    if (!p || p === 'unknown') return t('未知设备');
    return platform;
  }

  function shortFp(fp: string) {
    return fp ? fp.slice(0, 12).toUpperCase().replace(/(.{4})/g, '$1-').replace(/-$/, '') : '—';
  }

  function receiveStageLabel(stage: ReceiveTask['stage'], reason?: 'timeout' | 'offline') {
    if (stage === 'expired') return reason === 'offline' ? t('对方已离线') : t('已超时');
    const map: Record<ReceiveTask['stage'], string> = {
      incoming: t('待接收'), receiving: t('接收中'), done: t('接收完成'), rejected: t('已拒绝'),
      expired: t('已超时'),
    };
    return map[stage];
  }

  function stageLabel(modal: SendModal) {
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

  return {
    t, view, status, settings, devices, selectedId, textInput, pairIp, editingName, nameDraft, busy, isMobile,
    activeAction, isActionActive, activeReceive, fpDialog, requestDialog, pendingDecision,
    sendModal, sendVisible, sendCanceling, deviceList, selectedDevice, incomingCount, displayName, receiveTaskPercent,
    mediaPickerOpen, mediaLoading, mediaItems, mediaSelected,
    openMediaPicker, toggleMedia, confirmMedia, pickMediaEntry,
    filePickerOpen, fileLoading, fileItems, fileSelected,
    openFilePicker, toggleFile, confirmFilePick, browseFiles, pickFileEntry,
    appPickerOpen, appLoading, appItems, appSelected,
    appExcludeSystem, appExcludeNonLaunchable,
    openAppPicker, loadApps, toggleApp, confirmApps, pickAppEntry,
    load, doRefreshDevices, refreshDevices, pullPending, refreshIncoming, saveName, setReceiveDir, openReceiveDir,
    changeTheme, changeKeepAlive, changeLanguage, changeReceiveDir, resetReceiveDir, receivedPaths,
    saveReceivedToDownloads, openSavedDir,
    receiveDirOpen, receiveDirLoading, receiveDirItems, receiveDirLabel,
    loadSavedFiles, openSavedFile, openInFileManager,
    doPair, toggleFavorite, removeDevice, selectDevice, sendText, pickFiles, cancelSend,
    requestDecision, approveReceive, approveFingerprint, rejectReceive, receiveAllRequests, openIncoming,
    incomingViewReq, fmtSize, kindLabel, platformLabel, shortFp, receiveStageLabel, stageLabel,
    receiveDestLabel, loadReceiveItems,
  };
}
