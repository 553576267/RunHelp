<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import {
  NavBar as VanNavBar, Tabbar as VanTabbar, TabbarItem as VanTabbarItem,
  Button as VanButton, Icon as VanIcon, CellGroup as VanCellGroup, Cell as VanCell,
  Field as VanField, Progress as VanProgress, Empty as VanEmpty, Popup as VanPopup,
  Switch as VanSwitch, Loading as VanLoading, Picker as VanPicker,
} from 'vant';
import 'vant/es/nav-bar/style';
import 'vant/es/tabbar/style';
import 'vant/es/tabbar-item/style';
import 'vant/es/button/style';
import 'vant/es/icon/style';
import 'vant/es/cell/style';
import 'vant/es/cell-group/style';
import 'vant/es/field/style';
import 'vant/es/progress/style';
import 'vant/es/empty/style';
import 'vant/es/popup/style';
import 'vant/es/switch/style';
import 'vant/es/loading/style';
import 'vant/es/picker/style';
import 'vant/es/toast/style';
import 'vant/es/dialog/style';
import { useSync } from '../lib/useSync';
import { locale } from '../lib/i18n';

const {
  t, view, status, textInput, pairIp, editingName, nameDraft, busy, settings,
  activeAction, isActionActive, activeReceive, fpDialog, requestDialog,
  sendModal, sendVisible, sendCanceling, deviceList, selectedId, selectedDevice, displayName, receiveTaskPercent,
  refreshDevices, refreshIncoming, saveName, openReceiveDir,
  changeTheme, changeKeepAlive, changeLanguage, changeReceiveDir, resetReceiveDir,
  changeAutoExport, openSavedDir,
  doPair, toggleFavorite, removeDevice, selectDevice, sendText, pickFiles, cancelSend,
  mediaPickerOpen, mediaLoading, mediaItems, mediaSelected,
  pickMediaEntry, toggleMedia, confirmMedia,
  browseFiles,
  appPickerOpen, appLoading, appItems, appSelected,
  appExcludeSystem, appExcludeNonLaunchable,
  pickAppEntry, loadApps, toggleApp, confirmApps,
  approveReceive, approveFingerprint, rejectReceive, receiveAllRequests, openIncoming,
  incomingViewReq, fmtSize, kindLabel, platformLabel, shortFp, receiveStageLabel, stageLabel,
} = useSync();

/* 键盘避让由原生 decorView 内边距统一处理（见 MainActivity），
 * 这里不再叠加 visualViewport 计算，否则弹层会被抬高两次。 */
// 文本弹层：等弹出动画结束再聚焦，键盘与弹层同步上升，避免动画中途抖动
const textFieldRef = ref<{ focus: () => void } | null>(null);
function focusTextField() {
  textFieldRef.value?.focus();
}

const requestShow = computed(() => !!requestDialog.value);
const fpShow = computed(() => !!fpDialog.value);

/* 弹层内容常驻渲染（无数据时用占位对象）：
 * 首次弹出时不再临时创建整棵 DOM，动画帧里只有 transform，明显更跟手。 */
const sendView = computed(() => sendModal.value ?? {
  transferId: '', ip: '', targetName: '—', kind: 'file',
  items: [] as { name: string; size: number }[],
  stage: 'waiting' as const, detail: '', sent: 0, total: 0,
});
const requestView = computed(() => requestDialog.value ?? {
  id: '', transfer_id: '', from: '—', from_platform: '', fingerprint: '', trusted: true,
  kind: 'file', name: '', size: 0, count: 0, items: [] as { name: string; size: number }[],
});
const sendPercent = computed(() => {
  const m = sendModal.value;
  return m && m.total ? Math.min(100, Math.round(m.sent / m.total * 100)) : 0;
});

// 文本弹层：点「文本」才打开，输入后发送
const textModal = ref(false);
const hasText = computed(() => !!textInput.value.trim());
// 对方 IP：与设备名称一样，点编辑图标弹出居中对话框
const pairDialogOpen = ref(false);

/** 关弹层前先失焦，否则输入框被移除后键盘可能悬着不收 */
function blurActive() {
  const el = document.activeElement as HTMLElement | null;
  el?.blur?.();
}

function closeTextModal() {
  blurActive();
  textModal.value = false;
}

function closeNameDialog() {
  blurActive();
  editingName.value = false;
}

function closePairDialog() {
  blurActive();
  pairDialogOpen.value = false;
}

function pairAndClose() {
  // 失败时不关弹层：用户可以直接改 IP 重试，不用重新点编辑图标
  void doPair().then((ok) => {
    if (ok) closePairDialog();
  });
}

/** 保存设备名：先失焦再提交，避免弹窗关闭后键盘悬着 */
function saveNameAndClose() {
  blurActive();
  void saveName();
}

function sendTextAndClose() {
  void sendText('text');
  // 内容为空时保持弹层，让用户看到「内容不能为空」提示而不是白丢一次输入
  if (hasText.value) closeTextModal();
}

/* ---------- 下拉刷新 ----------
 * 只在列表滚到顶部时接管手势：拖动时列表阻尼下移，松手超过阈值才刷新；
 * 刷新期间在设备区中心显示转圈，不再使用箭头/文字提示。
 */
const scrollEl = ref<HTMLElement | null>(null);
const dragging = ref(false);
const pullStart = ref(0);
const pullY = ref(0);
const refreshing = ref(false);
const PULL_THRESHOLD = 46;

function onTouchStart(e: TouchEvent) {
  if (refreshing.value) return;
  const el = scrollEl.value;
  if (el && el.scrollTop > 0) {
    pullStart.value = 0;
    return;
  }
  pullStart.value = e.touches[0]?.clientY ?? 0;
  dragging.value = true;
}

function onTouchMove(e: TouchEvent) {
  if (!pullStart.value) return;
  const dy = (e.touches[0]?.clientY ?? 0) - pullStart.value;
  if (dy <= 0) {
    pullY.value = 0;
    return;
  }
  // 阻尼：越往下拉越沉，最多 84px
  pullY.value = Math.min(84, dy * 0.5);
}

function onTouchEnd() {
  dragging.value = false;
  pullStart.value = 0;
  if (pullY.value >= PULL_THRESHOLD) {
    void runRefresh();
  } else {
    pullY.value = 0;
  }
}

async function runRefresh() {
  if (refreshing.value) return;
  refreshing.value = true;
  pullY.value = 0;
  const before = deviceList.value.map((d) => (d.online ? 1 : 0)).join('');
  try {
    refreshDevices();
    // 手动刷新走「严格探测」（不应答即判定离线），通常 0.3~0.9 秒出结果。
    // 转圈等到列表真的变化、或超时 1.2 秒，避免「闪一下但什么都没变」的错觉。
    const start = Date.now();
    while (Date.now() - start < 1200) {
      await new Promise((resolve) => window.setTimeout(resolve, 80));
      const now = deviceList.value.map((d) => (d.online ? 1 : 0)).join('');
      if (now !== before && Date.now() - start >= 320) break;
    }
  } finally {
    refreshing.value = false;
  }
}

// 六宫格：文件 / 媒体(相册) / 剪贴板 / 文本 / 文件夹 / 应用(安装包)
function pick(kind: 'file' | 'media' | 'app' | 'folder') {
  if (kind === 'media') {
    mediaFilter.value = 'all';
    pickMediaEntry();
    return;
  }
  if (kind === 'file') {
    // 「文件」直接进系统文件选择器（有最近/图片/视频/文档等分类，见系统自带体验）
    void browseFiles();
    return;
  }
  if (kind === 'app') {
    appQuery.value = '';
    pickAppEntry();
    return;
  }
  void pickFiles(kind);
}

/* ---------- 相册 / 最近文件 日期分组 ---------- */
function dayLabel(ts: number) {
  if (!ts) return t('更早');
  const d = new Date(ts * 1000);
  const today = new Date();
  const yesterday = new Date(Date.now() - 86400000);
  const sameDay = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
  if (sameDay(d, today)) return t('今天');
  if (sameDay(d, yesterday)) return t('昨天');
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

/** 按日期把相册/文件列表切成若干段（列表本身已按时间倒序） */
function groupByDay<T extends { date: number }>(items: T[]) {
  const groups: { label: string; items: T[] }[] = [];
  for (const item of items) {
    const label = dayLabel(item.date);
    const last = groups[groups.length - 1];
    if (last && last.label === label) last.items.push(item);
    else groups.push({ label, items: [item] });
  }
  return groups;
}

/* 相册筛选：全部 / 只看照片 / 只看视频（本地过滤，不再查一次 MediaStore） */
const mediaFilter = ref<'all' | 'photo' | 'video'>('all');
const mediaVisible = computed(() => mediaItems.value.filter((m) => {
  if (mediaFilter.value === 'photo') return !m.video;
  if (mediaFilter.value === 'video') return m.video;
  return true;
}));
const mediaVisibleGroups = computed(() => groupByDay(mediaVisible.value));
const mediaFilterOptions = computed(() => [
  { key: 'all' as const, label: t('全部'), count: mediaItems.value.length },
  { key: 'photo' as const, label: t('照片'), count: mediaItems.value.filter((m) => !m.video).length },
  { key: 'video' as const, label: t('视频'), count: mediaItems.value.filter((m) => m.video).length },
]);

/* 应用搜索：一堆图标里翻名字太慢，顶部给个即时过滤框 */
const appQuery = ref('');
const appVisible = computed(() => {
  const q = appQuery.value.trim().toLowerCase();
  if (!q) return appItems.value;
  return appItems.value.filter((a) => a.label.toLowerCase().includes(q) || a.package.toLowerCase().includes(q));
});

/* 应用列表右上角筛选菜单 */
const appMenuOpen = ref(false);

async function toggleAppFilter(kind: 'system' | 'nonlaunch', value: boolean) {
  if (kind === 'system') appExcludeSystem.value = value;
  else appExcludeNonLaunchable.value = value;
  await loadApps();
}

/* ---------- 设置页下拉选择 ---------- */
const themePickerShow = ref(false);
const langPickerShow = ref(false);
const themeColumns = computed(() => [
  { text: t('柔光浅'), value: 'soft-light' },
  { text: t('柔光青深色'), value: 'soft-cyan' },
]);
const themeValue = computed(() => settings.value.mobile_theme || 'soft-light');
const themeLabel = computed(() => themeColumns.value.find((c) => c.value === themeValue.value)?.text || t('柔光浅'));
const langColumns = computed(() => [
  { text: '中文', value: 'zh-CN' },
  { text: 'English', value: 'en-US' },
]);
const langValue = computed(() => (locale.value === 'en-US' ? 'en-US' : 'zh-CN'));
const langLabel = computed(() => langColumns.value.find((c) => c.value === langValue.value)?.text || '中文');

type PickerConfirm = { selectedValues: (string | number)[] };
function onThemeConfirm({ selectedValues }: PickerConfirm) {
  themePickerShow.value = false;
  const v = String(selectedValues[0] ?? '');
  if (v && v !== themeValue.value) void changeTheme(v);
}
function onLangConfirm({ selectedValues }: PickerConfirm) {
  langPickerShow.value = false;
  const v = String(selectedValues[0] ?? '');
  if (v && v !== langValue.value) void changeLanguage(v);
}

// 接收目录：优先展示用户选的文件夹名，否则提示默认位置
const receiveDirLabel = computed(() => {
  const name = settings.value.mobile_receive_dir_name;
  if (name) return name;
  return t('默认：下载/RunHelp');
});
const autoExportOn = computed(() => settings.value.mobile_auto_export !== '0');

function fileStateLabel(state: string) {
  if (state === 'done') return t('接收完成');
  if (state === 'receiving') return t('接收中');
  return t('待接收');
}

/** 失效（超时/发送方掉线）时，未完成的项统一显示失效原因，不再显示「待接收」 */
function fileTagLabel(state: string) {
  const task = activeReceive.value;
  if (task?.stage === 'expired' && state !== 'done') {
    return task.expireReason === 'offline' ? t('对方已离线') : t('已超时');
  }
  return fileStateLabel(state);
}

/* ---------- Android 返回手势：先关弹层，再退出 ----------
 * MainActivity 接管了返回键：WebView 有历史就 goBack()，没有才结束 Activity。
 * 所以只要有弹层，就压入一条历史记录；侧滑/返回键触发 popstate 时关掉最上层弹层，
 * 不会再出现「关个弹层把应用也关了」。
 */
const onAndroid = /Android/i.test(typeof navigator === 'undefined' ? '' : navigator.userAgent);

type OverlayCloser = { open: () => boolean; close: () => void };
// 顺序即层级：后声明的在上面，返回时从后往前找第一个还开着的关掉
const overlayClosers: OverlayCloser[] = [
  { open: () => appMenuOpen.value, close: () => (appMenuOpen.value = false) },
  { open: () => themePickerShow.value, close: () => (themePickerShow.value = false) },
  { open: () => langPickerShow.value, close: () => (langPickerShow.value = false) },
  { open: () => appPickerOpen.value, close: () => (appPickerOpen.value = false) },
  { open: () => mediaPickerOpen.value, close: () => (mediaPickerOpen.value = false) },
  { open: () => textModal.value, close: closeTextModal },
  { open: () => editingName.value, close: closeNameDialog },
  { open: () => pairDialogOpen.value, close: closePairDialog },
  { open: () => sendVisible.value, close: () => (sendVisible.value = false) },
  {
    open: () => fpShow.value,
    close: () => {
      if (fpDialog.value) void rejectReceive(fpDialog.value);
    },
  },
  {
    open: () => requestShow.value,
    close: () => {
      if (requestDialog.value) void rejectReceive(requestDialog.value);
    },
  },
];
const anyOverlayOpen = computed(() => overlayClosers.some((c) => c.open()));
let guardPushed = false;
let ignorePop = false;

watch(anyOverlayOpen, (open) => {
  if (!onAndroid) return;
  if (open && !guardPushed) {
    guardPushed = true;
    try {
      history.pushState({ rhOverlay: 1 }, '');
    } catch {
      guardPushed = false;
    }
  } else if (!open && guardPushed) {
    // 由界面按钮关的：把压入的那条历史退掉，保持历史栈干净
    guardPushed = false;
    ignorePop = true;
    history.back();
  }
});

function onPopState() {
  if (ignorePop) {
    ignorePop = false;
    return;
  }
  if (!guardPushed) return;
  guardPushed = false;
  const top = [...overlayClosers].reverse().find((c) => c.open());
  top?.close();
}

if (onAndroid) window.addEventListener('popstate', onPopState);
onBeforeUnmount(() => {
  if (onAndroid) window.removeEventListener('popstate', onPopState);
});
</script>

<template>
  <div class="mobile-sync">
    <van-nav-bar class="brand-nav" :border="false">
      <template #title>
        <span class="brand-title">
          <span class="brand-mark">RunHelp</span>
          <span class="brand-sub">{{ t('数据同步') }}</span>
        </span>
      </template>
    </van-nav-bar>

    <!-- 发送 -->
    <div v-show="view === 'send'" class="view-body send-view">
      <div class="device-zone">
        <div class="zone-head">
          <span>{{ t('附近设备') }}</span>
          <span v-if="deviceList.length" class="zone-sub">{{ deviceList.length }}</span>
        </div>
        <div
          ref="scrollEl"
          class="device-scroll"
          @touchstart.passive="onTouchStart"
          @touchmove.passive="onTouchMove"
          @touchend.passive="onTouchEnd"
          @touchcancel.passive="onTouchEnd"
        >
          <div class="device-list" :class="{ dragging }" :style="{ transform: `translateY(${pullY}px)` }">
            <div v-if="!deviceList.length" class="hint-empty">
              {{ t('未发现设备，请确认对方也打开了数据同步页，或在设置里手动添加') }}
            </div>
            <div
              v-for="d in deviceList"
              :key="d.id"
              class="device-card"
              :class="{ selected: selectedId === d.id, offline: !d.online }"
              @click="selectDevice(d)"
            >
              <span class="dot" :class="{ on: d.online }"></span>
              <div class="device-info">
                <div class="dev-name">{{ displayName(d) }}</div>
                <div class="device-sub">
                  <span>{{ platformLabel(d.platform) }} · {{ shortFp(d.fingerprint) }}</span>
                  <span class="muted">{{ d.online ? d.ip : (t('离线') + ' · ' + d.last_ip) }}</span>
                </div>
              </div>
              <div class="row-actions" @click.stop>
                <button class="star-btn" :class="{ on: d.favorite }" :aria-label="d.favorite ? t('取消收藏') : t('收藏')" @click="toggleFavorite(d)">
                  <svg viewBox="0 0 24 24" :fill="d.favorite ? 'currentColor' : 'none'" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
                    <path d="m12 2.6 2.9 5.9 6.5.95-4.7 4.6 1.1 6.45L12 17.45l-5.8 3.05 1.1-6.45-4.7-4.6 6.5-.95L12 2.6Z" />
                  </svg>
                </button>
                <button class="trash-btn" :aria-label="t('删除设备')" @click="removeDevice(d)">
                  <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M4 7h16M10 4h4M6 7l1 13h10l1-13M10 11v5M14 11v5" />
                  </svg>
                </button>
              </div>
            </div>
          </div>
        </div>
        <!-- 刷新中：设备区正中央转圈 -->
        <div v-if="refreshing" class="refresh-pill">
          <van-loading size="18" />
          <span>{{ t('刷新中') }}</span>
        </div>
      </div>

      <div class="content-zone">
        <div class="zone-head">
          <span>{{ t('发送内容') }}</span>
          <span v-if="selectedDevice" class="target-hint">{{ t('目标') }}: {{ displayName(selectedDevice) }}</span>
          <span v-else class="target-hint warn">{{ t('请先选择设备') }}</span>
        </div>
        <div class="action-grid">
          <button :disabled="!selectedDevice || busy" @click="pick('file')">
            <van-icon name="description" size="22" :class="{ spin: busy && activeAction === 'file' }" />
            <span>{{ t('文件') }}</span>
          </button>
          <button :disabled="!selectedDevice || busy" @click="pick('media')">
            <van-icon name="photo" size="22" :class="{ spin: busy && activeAction === 'media' }" />
            <span>{{ t('媒体') }}</span>
          </button>
          <button :disabled="!selectedDevice || busy" @click="sendText('clipboard')">
            <svg class="grid-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" :class="{ spin: busy && activeAction === 'clipboard' }">
              <rect x="6" y="4" width="12" height="17" rx="2" />
              <path d="M9 4V3h6v1M9 9h6M9 13h6M9 17h4" />
            </svg>
            <span>{{ t('剪贴板') }}</span>
          </button>
          <button :disabled="!selectedDevice || busy" @click="textModal = true">
            <van-icon name="edit" size="22" />
            <span>{{ t('文本') }}</span>
          </button>
          <button :disabled="!selectedDevice || busy" @click="pick('folder')">
            <svg class="grid-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round" :class="{ spin: busy && activeAction === 'folder' }">
              <path d="M3 7.5h6l2 2h10v8.5a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V7.5Z" />
              <path d="M3 7.5V6a2 2 0 0 1 2-2h3.6l2 2" />
            </svg>
            <span>{{ t('文件夹') }}</span>
          </button>
          <button :disabled="!selectedDevice || busy" @click="pick('app')">
            <van-icon name="apps-o" size="22" :class="{ spin: busy && activeAction === 'app' }" />
            <span>{{ t('应用') }}</span>
          </button>
        </div>
      </div>
    </div>

    <!-- 接收：只在页签可见时渲染，避免请求到达时和弹层动画抢同一帧 -->
    <div v-if="view === 'receive'" class="view-body receive-view">
      <div class="receive-summary">
        <div class="zone-head">
          <span>{{ t('最近接收') }}</span>
          <span v-if="activeReceive" class="stage" :class="activeReceive.stage">{{ receiveStageLabel(activeReceive.stage, activeReceive.expireReason) }}</span>
        </div>
        <van-empty v-if="!activeReceive" :description="t('暂无待接收的内容')" />
        <div v-else class="receive-card" :class="{ rejected: activeReceive.stage === 'rejected', expired: activeReceive.stage === 'expired' }">
          <div class="receive-head">
            <div>
              <strong>{{ activeReceive.from }}</strong>
              <div class="muted tiny">{{ platformLabel(activeReceive.fromPlatform) }} · {{ shortFp(activeReceive.fingerprint) }}</div>
            </div>
            <span class="kind-chip">{{ kindLabel(activeReceive.kind) }}</span>
          </div>
          <van-progress
            v-if="activeReceive.stage === 'receiving' || activeReceive.stage === 'done'"
            :percentage="receiveTaskPercent"
            class="receive-progress"
          />
        </div>
      </div>

      <!-- 文件列表：独立块，铺满剩余空间 -->
      <div v-if="activeReceive" class="file-panel">
        <div class="file-head">
          <span>{{ t('文件列表') }}</span>
          <span class="file-count">{{ activeReceive.items.length }} {{ t('个文件') }} · {{ fmtSize(activeReceive.total) }}</span>
        </div>
        <div class="file-scroll">
          <div v-for="(item, index) in activeReceive.items" :key="`${activeReceive.id}-${index}`" class="file-row">
            <span class="file-state" :class="item.status">
              <van-icon v-if="item.status === 'done'" name="success" size="12" />
              <van-loading v-else-if="item.status === 'receiving'" size="12" />
              <span v-else class="state-ring" />
            </span>
            <div class="file-main">
              <div class="file-name">{{ item.name }}</div>
              <div class="file-size">{{ fmtSize(item.size) }}</div>
            </div>
            <span class="file-tag" :class="activeReceive.stage === 'expired' && item.status === 'pending' ? 'expired' : item.status">
              {{ fileTagLabel(item.status) }}
            </span>
          </div>
        </div>
      </div>

      <div class="receive-actions">
        <van-button plain type="danger" :disabled="!incomingViewReq" @click="incomingViewReq && rejectReceive(incomingViewReq)">{{ t('拒绝') }}</van-button>
        <van-button type="primary" :disabled="!incomingViewReq" @click="receiveAllRequests">{{ t('全部接收') }}</van-button>
        <van-button plain @click="refreshIncoming"><van-icon name="replay" :class="{ spin: isActionActive('refresh-incoming') }" /> {{ t('刷新') }}</van-button>
        <van-button plain @click="openSavedDir"><van-icon name="folder-o" /> {{ t('打开目录') }}</van-button>
      </div>
    </div>

    <!-- 设置 -->
    <div v-if="view === 'settings'" class="view-body settings-view">
      <div class="settings-scroll">
        <div class="zone-head"><span>{{ t('本机信息') }}</span></div>
        <van-cell-group inset>
          <van-cell :title="t('设备名称')" :value="status?.device_name || '—'">
            <template #right-icon>
              <button class="edit-icon" :aria-label="t('修改')" @click="editingName = true">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M4 20h4L20 8l-4-4L4 16v4ZM14 6l4 4" />
                </svg>
              </button>
            </template>
          </van-cell>
          <van-cell :title="t('本机指纹')" :value="status?.fingerprint_short || '—'" class="fp-cell" />
          <van-cell :title="t('接收目录')" :label="receiveDirLabel" is-link @click="changeReceiveDir" />
          <van-cell v-if="settings.mobile_receive_dir_name" :title="t('恢复默认接收位置')" is-link @click="resetReceiveDir" />
          <van-cell :title="t('打开接收目录')" is-link @click="openReceiveDir" />
        </van-cell-group>

        <div class="zone-head spaced"><span>{{ t('同步选项') }}</span></div>
        <van-cell-group inset>
          <van-cell :title="t('后台保活')" :label="t('切到后台后继续保持设备发现与接收')">
            <template #right-icon>
              <van-switch :model-value="settings.mobile_keepalive !== '0'" @update:model-value="changeKeepAlive" />
            </template>
          </van-cell>
          <van-cell :title="t('接收后自动保存到 下载/RunHelp')" :label="t('便于在文件管理器中找到接收的文件')">
            <template #right-icon>
              <van-switch :model-value="autoExportOn" @update:model-value="changeAutoExport" />
            </template>
          </van-cell>
          <van-cell :title="t('界面主题')" :value="themeLabel" is-link @click="themePickerShow = true" />
          <van-cell :title="t('语言')" :value="langLabel" is-link @click="langPickerShow = true" />
        </van-cell-group>

        <div class="zone-head spaced"><span>{{ t('手动添加') }}</span></div>
        <van-cell-group inset>
          <van-cell :title="t('对方 IP')" :value="pairIp || t('未填写')">
            <template #right-icon>
              <button class="edit-icon" :aria-label="t('修改')" @click="pairDialogOpen = true">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M4 20h4L20 8l-4-4L4 16v4ZM14 6l4 4" />
                </svg>
              </button>
            </template>
          </van-cell>
        </van-cell-group>
      </div>
    </div>

    <!-- 底部页签：作为普通布局元素参与排版，不再固定悬浮 -->
    <van-tabbar v-model="view" :fixed="false" :border="false" class="bottom-tabbar" active-color="var(--accent)">
      <van-tabbar-item name="send" icon="send-gift-o">{{ t('发送') }}</van-tabbar-item>
      <van-tabbar-item name="receive" icon="down" :dot="!!incomingViewReq">{{ t('接收') }}</van-tabbar-item>
      <van-tabbar-item name="settings" icon="setting-o">{{ t('设置') }}</van-tabbar-item>
    </van-tabbar>

    <!-- 文本输入：居中对话框（浮在页面之上，锁定背景滚动） -->
    <van-popup
      v-model:show="textModal"
      :lazy-render="false"
      position="center"
      round
      :lock-scroll="true"
      teleport="body"
      class="m-dialog"
      @opened="focusTextField"
    >
      <div class="dialog-body">
        <div class="dialog-title">{{ t('发送文字') }}</div>
        <van-field
          ref="textFieldRef"
          v-model="textInput"
          type="textarea"
          :rows="4"
          :placeholder="t('输入要发送的文字')"
          class="text-field"
        />
        <div class="dialog-actions">
          <van-button plain @click="closeTextModal">{{ t('取消') }}</van-button>
          <van-button type="primary" :disabled="!selectedDevice || !hasText || busy" @click="sendTextAndClose">{{ t('发送') }}</van-button>
        </div>
      </div>
    </van-popup>

    <!-- 发送状态弹窗：显示开关与数据分离，关闭后仍可追踪结果 -->
    <van-popup v-model:show="sendVisible" :lazy-render="false" teleport="body" position="bottom" round :close-on-click-overlay="false" class="sheet">
      <div class="sheet-body">
        <div class="sheet-title">{{ t('发送状态') }}</div>
        <div class="status-line"><span class="muted">{{ t('接收方') }}</span><strong>{{ sendView.targetName }}</strong></div>
        <div class="status-line"><span class="muted">{{ t('状态') }}</span><strong>{{ stageLabel(sendView) }}</strong></div>
        <van-progress v-if="sendView.stage === 'transferring'" :percentage="sendPercent" class="send-progress" />
        <div class="sheet-actions">
          <van-button plain type="danger" :disabled="sendCanceling || ['done','cancelled','error'].includes(sendView.stage)" @click="cancelSend">{{ sendCanceling ? t('撤销中') : t('取消发送') }}</van-button>
          <van-button type="primary" @click="sendVisible = false">{{ t('关闭') }}</van-button>
        </div>
      </div>
    </van-popup>

    <!-- 应用内相册：全屏、按日期分组，图片 + 视频混选 -->
    <van-popup
      v-model:show="mediaPickerOpen"
      :lazy-render="false"
      position="bottom"
      :close-on-click-overlay="false"
      class="full-picker"
    >
      <div class="picker-head">
        <button class="picker-back" :aria-label="t('取消')" @click="mediaPickerOpen = false">
          <van-icon name="arrow-left" size="18" />
        </button>
        <div class="picker-title">
          <strong>{{ t('照片与视频') }}</strong>
          <span class="picker-sub">{{ mediaItems.length }} {{ t('项') }}</span>
        </div>
        <button
          class="picker-done"
          :disabled="!mediaSelected.length || busy"
          @click="confirmMedia"
        >{{ mediaSelected.length ? t('发送 ({count})', { count: String(mediaSelected.length) }) : t('发送') }}</button>
      </div>

      <div class="picker-filter">
        <button
          v-for="f in mediaFilterOptions"
          :key="f.key"
          class="filter-chip"
          :class="{ on: mediaFilter === f.key }"
          @click="mediaFilter = f.key"
        >{{ f.label }}<span class="chip-count">{{ f.count }}</span></button>
      </div>

      <div class="picker-scroll">
        <div v-if="mediaLoading" class="picker-hint">
          <van-loading size="20" />
          <span>{{ t('正在读取相册') }}</span>
        </div>
        <div v-else-if="!mediaItems.length" class="picker-hint">{{ t('相册里没有可发送的图片或视频') }}</div>
        <div v-else-if="!mediaVisible.length" class="picker-hint">{{ t('该类内容为空') }}</div>
        <template v-else>
          <section v-for="g in mediaVisibleGroups" :key="g.label" class="media-section">
            <div class="section-label">{{ g.label }}</div>
            <div class="media-grid">
              <button
                v-for="m in g.items"
                :key="m.uri"
                class="media-cell"
                :class="{ on: mediaSelected.includes(m.uri) }"
                @click="toggleMedia(m.uri)"
              >
                <img v-if="m.thumb" :src="m.thumb" :alt="m.name" decoding="async" />
                <span v-else class="media-fallback">
                  <van-icon :name="m.video ? 'video-o' : 'photo-o'" size="22" />
                </span>
                <span v-if="m.video" class="media-video">
                  <van-icon name="play" size="9" />
                </span>
                <span class="media-check" :class="{ show: mediaSelected.includes(m.uri) }">
                  {{ mediaSelected.indexOf(m.uri) + 1 }}
                </span>
              </button>
            </div>
          </section>
        </template>
      </div>
    </van-popup>

    <!-- 已安装应用：默认排除系统应用/无法启动的应用（与 LocalSend 一致） -->
    <van-popup
      v-model:show="appPickerOpen"
      :lazy-render="false"
      position="bottom"
      :close-on-click-overlay="false"
      class="full-picker"
    >
      <div class="picker-head">
        <button class="picker-back" :aria-label="t('取消')" @click="appPickerOpen = false">
          <van-icon name="arrow-left" size="18" />
        </button>
        <div class="picker-title">
          <strong>{{ t('安装包') }}</strong>
          <span class="picker-sub">{{ appItems.length }} {{ t('个应用') }}</span>
        </div>
        <div class="picker-head-actions">
          <button class="picker-done" :disabled="!appSelected.length || busy" @click="confirmApps">
            {{ appSelected.length ? t('发送 ({count})', { count: String(appSelected.length) }) : t('发送') }}
          </button>
          <button class="picker-more" :aria-label="t('筛选')" @click="appMenuOpen = !appMenuOpen">
            <van-icon name="ellipsis" size="18" />
          </button>
        </div>
        <div v-if="appMenuOpen" class="app-menu" @click.stop>
          <label class="app-menu-row">
            <span>{{ t('排除系统应用') }}</span>
            <van-switch size="18" :model-value="appExcludeSystem" @update:model-value="(v: boolean) => toggleAppFilter('system', v)" />
          </label>
          <label class="app-menu-row">
            <span>{{ t('排除无法启动的应用') }}</span>
            <van-switch size="18" :model-value="appExcludeNonLaunchable" @update:model-value="(v: boolean) => toggleAppFilter('nonlaunch', v)" />
          </label>
        </div>
      </div>

      <div class="picker-filter app-search">
        <div class="search-box">
          <van-icon name="search" size="15" />
          <input
            v-model="appQuery"
            type="search"
            :placeholder="t('搜索应用名称')"
            class="search-input"
          />
          <button v-if="appQuery" class="search-clear" :aria-label="t('清除')" @click="appQuery = ''">
            <van-icon name="cross" size="12" />
          </button>
        </div>
      </div>

      <div class="picker-scroll app-scroll" @scroll="appMenuOpen = false">
        <div v-if="appLoading" class="picker-hint">
          <van-loading size="20" />
          <span>{{ t('正在读取应用列表') }}</span>
        </div>
        <div v-else-if="!appItems.length" class="picker-hint">{{ t('没有匹配的应用') }}</div>
        <div v-else-if="!appVisible.length" class="picker-hint">{{ t('没有匹配的应用') }}</div>
        <div v-else class="app-grid">
          <button
            v-for="a in appVisible"
            :key="a.package"
            class="app-cell"
            :class="{ on: appSelected.includes(a.package) }"
            @click="toggleApp(a.package)"
          >
            <span class="app-icon">
              <img v-if="a.icon" :src="a.icon" :alt="a.label" decoding="async" />
              <van-icon v-else name="apps-o" size="24" />
              <span v-if="appSelected.includes(a.package)" class="app-badge">
                {{ appSelected.indexOf(a.package) + 1 }}
              </span>
            </span>
            <span class="app-label">{{ a.label }}</span>
            <span class="app-size">{{ fmtSize(a.size) }}</span>
          </button>
        </div>
      </div>
    </van-popup>

    <!-- 接收请求 -->
    <van-popup v-model:show="requestShow" :lazy-render="false" teleport="body" position="bottom" round :close-on-click-overlay="false" class="sheet">
      <div class="sheet-body">
        <div class="sheet-title">{{ t('收到传输请求') }}</div>
        <div class="status-line"><span class="muted">{{ t('发送方') }}</span><strong>{{ requestView.from }}</strong></div>
        <div class="status-line"><span class="muted">{{ t('类型') }}</span><strong>{{ kindLabel(requestView.kind) }}</strong></div>
        <div class="status-line"><span class="muted">{{ t('发送文件数量') }}</span><strong>{{ requestView.count }}</strong></div>
        <div class="sheet-actions three">
          <van-button plain @click="requestDialog && rejectReceive(requestDialog)">{{ t('拒绝') }}</van-button>
          <van-button plain type="primary" @click="openIncoming()">{{ t('查看清单') }}</van-button>
          <van-button type="primary" @click="requestDialog && approveReceive(requestDialog)">{{ t('接收') }}</van-button>
        </div>
      </div>
    </van-popup>

    <!-- 指纹确认 -->
    <van-popup v-model:show="fpShow" :lazy-render="false" teleport="body" position="bottom" round :close-on-click-overlay="false" class="sheet">
      <div v-if="fpDialog" class="sheet-body">
        <div class="sheet-title">{{ t('确认设备身份') }}</div>
        <p class="fp-hint">{{ t('首次连接，请核对对方显示的指纹是否一致，确认后建立信任') }}</p>
        <div class="status-line"><span class="muted">{{ t('设备') }}</span><strong>{{ fpDialog.from }}</strong></div>
        <div class="status-line"><span class="muted">{{ t('指纹') }}</span><strong class="mono fp-code">{{ shortFp(fpDialog.fingerprint) }}</strong></div>
        <div class="sheet-actions">
          <van-button plain type="danger" @click="rejectReceive(fpDialog)">{{ t('拒绝') }}</van-button>
          <van-button type="primary" @click="approveFingerprint">{{ t('信任并接收') }}</van-button>
        </div>
      </div>
    </van-popup>

    <!-- 修改设备名：居中对话框，键盘弹出时随视口收缩居中上移 -->
    <van-popup
      v-model:show="editingName"
      :lazy-render="false"
      position="center"
      round
      :lock-scroll="true"
      teleport="body"
      class="m-dialog"
    >
      <div class="dialog-body">
        <div class="dialog-title">{{ t('修改设备名称') }}</div>
        <van-field v-model="nameDraft" :label="t('设备名称')" :placeholder="t('输入设备名称')" class="dialog-field" />
        <div class="dialog-actions">
          <van-button plain @click="closeNameDialog">{{ t('取消') }}</van-button>
          <van-button type="primary" @click="saveNameAndClose">{{ t('保存') }}</van-button>
        </div>
      </div>
    </van-popup>

    <!-- 手动添加设备：居中对话框，输入 IP 后直接连接 -->
    <van-popup
      v-model:show="pairDialogOpen"
      :lazy-render="false"
      position="center"
      round
      :lock-scroll="true"
      teleport="body"
      class="m-dialog"
    >
      <div class="dialog-body">
        <div class="dialog-title">{{ t('手动添加设备') }}</div>
        <van-field
          v-model="pairIp"
          :label="t('对方 IP')"
          placeholder="192.168.1.10"
          clearable
          class="dialog-field"
        />
        <p class="dialog-hint">{{ t('确保对方已打开数据同步页，两机在同一局域网内') }}</p>
        <div class="dialog-actions">
          <van-button plain @click="closePairDialog">{{ t('取消') }}</van-button>
          <van-button type="primary" :disabled="busy || !pairIp.trim()" @click="pairAndClose">{{ t('连接设备') }}</van-button>
        </div>
      </div>
    </van-popup>

    <!-- 主题 / 语言下拉选择 -->
    <van-popup v-model:show="themePickerShow" position="bottom" round class="sheet">
      <van-picker
        :title="t('界面主题')"
        :columns="themeColumns"
        :model-value="[themeValue]"
        @confirm="onThemeConfirm"
        @cancel="themePickerShow = false"
      />
    </van-popup>
    <van-popup v-model:show="langPickerShow" position="bottom" round class="sheet">
      <van-picker
        :title="t('语言')"
        :columns="langColumns"
        :model-value="[langValue]"
        @confirm="onLangConfirm"
        @cancel="langPickerShow = false"
      />
    </van-popup>
  </div>
</template>

<style scoped>
.mobile-sync { height: 100%; min-height: 100%; background: var(--bg-0); display: flex; flex-direction: column; }
/* 标题艺术字：渐变 + 斜体字标 */
.brand-nav :deep(.van-nav-bar__title) { max-width: 90%; }
.brand-title { display: inline-flex; align-items: baseline; gap: 6px; white-space: nowrap; line-height: 1; }
.brand-mark {
  font-family: 'Trebuchet MS', 'Segoe UI', system-ui, sans-serif;
  font-size: 17px;
  font-weight: 800;
  font-style: italic;
  letter-spacing: .3px;
  background: linear-gradient(105deg, var(--accent), var(--accent-hover) 70%);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}
.brand-sub { font-size: 15px; font-weight: 600; color: var(--text-1); letter-spacing: .5px; }

.view-body { flex: 1; min-height: 0; display: flex; flex-direction: column; overflow: hidden; padding-bottom: 10px; }
.zone-head { display: flex; align-items: center; justify-content: space-between; padding: 10px 16px 8px; font-size: 13px; font-weight: 500; color: var(--text-1); flex-shrink: 0; }
.zone-head.spaced { margin-top: 8px; }
.zone-sub { font-size: 12px; font-weight: 400; color: var(--text-2); }
.target-hint { font-size: 11px; font-weight: 400; color: var(--accent); }
.target-hint.warn { color: var(--text-2); }

/* 发送：设备区占 2/3，内容区占 1/3（比上一版矮约 1/4），六宫格按比例自适应 */
.device-zone { flex: 2; min-height: 0; display: flex; flex-direction: column; position: relative; }
.device-scroll { flex: 1; min-height: 0; overflow-y: auto; overscroll-behavior: contain; padding: 0 12px 10px; }
.device-list { transition: transform .22s cubic-bezier(.22, .61, .36, 1); }
.device-list.dragging { transition: none; }
.hint-empty { padding: 26px 12px; text-align: center; font-size: 12px; line-height: 1.7; color: var(--text-2); }

/* 设备卡片：更明显的卡片 + 选中描边高亮 */
.device-card {
  display: flex;
  align-items: center;
  gap: 10px;
  min-height: 62px;
  padding: 10px 12px;
  margin-bottom: 8px;
  border-radius: 14px;
  background: var(--bg-1);
  border: 1px solid var(--border-soft);
  box-shadow: 0 1px 2px rgba(0, 0, 0, .04);
  transition: border-color .18s, box-shadow .18s, background .18s;
  cursor: pointer;
}
.device-card.selected {
  border-color: var(--accent);
  background: var(--bg-2);
  box-shadow: 0 0 0 1px var(--accent), 0 6px 16px var(--accent-soft);
}
.device-card.offline { opacity: .62; }
.dot { width: 9px; height: 9px; border-radius: 50%; background: var(--text-2); flex-shrink: 0; box-shadow: 0 0 0 3px var(--bg-2); }
.dot.on { background: var(--success); }
.device-info { flex: 1; min-width: 0; }
.dev-name { font-size: 14px; font-weight: 600; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.device-sub { display: flex; flex-direction: column; gap: 3px; margin-top: 4px; font-size: 11px; }
.muted { color: var(--text-2); }
.tiny { font-size: 10px; }
.row-actions { display: flex; align-items: center; gap: 2px; flex-shrink: 0; }
.star-btn, .trash-btn { width: 34px; height: 34px; display: inline-flex; align-items: center; justify-content: center; border: none; border-radius: 9px; background: transparent; color: var(--text-2); cursor: pointer; }
.star-btn svg, .trash-btn svg { width: 21px; height: 21px; }
.star-btn.on { color: var(--warn); }
.trash-btn:active { color: var(--danger); background: rgba(193, 92, 86, .12); }

/* 刷新中：设备区正中央的转圈胶囊 */
.refresh-pill {
  position: absolute;
  left: 50%;
  top: 50%;
  transform: translate(-50%, -50%);
  display: inline-flex;
  align-items: center;
  gap: 8px;
  padding: 9px 16px;
  border-radius: 20px;
  background: var(--bg-1);
  border: 1px solid var(--border-soft);
  box-shadow: 0 8px 22px rgba(0, 0, 0, .12);
  font-size: 12px;
  color: var(--text-2);
  pointer-events: none;
  z-index: 3;
}

.content-zone { flex: 1; min-height: 168px; display: flex; flex-direction: column; background: var(--bg-1); border-top-left-radius: 18px; border-top-right-radius: 18px; box-shadow: 0 -6px 18px rgba(0, 0, 0, .05); }
.action-grid { flex: 1; min-height: 0; display: grid; grid-template-columns: repeat(3, 1fr); grid-template-rows: repeat(2, 1fr); gap: 8px; padding: 2px 12px 14px; }
.action-grid button { display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; border: 1px solid var(--border-soft); border-radius: 12px; background: var(--bg-2); color: var(--accent); font-size: 12px; cursor: pointer; min-height: 62px; }
.action-grid button:active:not(:disabled) { background: var(--accent-soft); }
.action-grid button:disabled { opacity: .42; }
.action-grid button span { color: var(--text-1); }
.grid-icon { width: 22px; height: 22px; flex-shrink: 0; }

/* 接收：摘要 + 独立文件列表块（铺满剩余空间） */
.receive-summary { flex-shrink: 0; }
.receive-summary :deep(.van-empty) { padding: 34px 0; }
.receive-card { margin: 0 12px 10px; padding: 12px 14px; border-radius: 14px; background: var(--bg-1); border: 1px solid var(--border-soft); }
.receive-card.rejected { opacity: .6; }
.receive-card.expired { opacity: .75; border-color: var(--border); }
.receive-head { display: flex; align-items: flex-start; justify-content: space-between; gap: 10px; }
.receive-head strong { font-size: 14px; }
.kind-chip { font-size: 11px; padding: 2px 8px; border-radius: 9px; background: var(--accent-soft); color: var(--accent); }
.stage { font-size: 11px; padding: 2px 8px; border-radius: 10px; background: var(--bg-3); color: var(--text-1); }
.stage.done { color: var(--success); }
.stage.rejected { color: var(--danger); }
.stage.receiving { color: var(--accent); }
.receive-progress { margin-top: 12px; }

.file-panel { flex: 1; min-height: 0; display: flex; flex-direction: column; margin: 0 12px; border-radius: 16px; background: var(--bg-1); border: 1px solid var(--border-soft); overflow: hidden; }
.file-head { display: flex; align-items: center; justify-content: space-between; padding: 11px 14px; font-size: 12px; font-weight: 600; color: var(--text-1); background: var(--bg-2); border-bottom: 1px solid var(--border-soft); flex-shrink: 0; }
.file-count { font-weight: 400; font-size: 11px; color: var(--text-2); }
.file-scroll { flex: 1; min-height: 0; overflow-y: auto; padding: 8px 10px 10px; }
.file-row {
  display: flex;
  align-items: center;
  gap: 10px;
  padding: 9px 11px;
  margin-bottom: 6px;
  border-radius: 11px;
  background: var(--bg-0);
  border: 1px solid var(--border-soft);
}
.file-row:last-child { margin-bottom: 0; }
/* 状态指示：待接收=空心圆点，接收中=转圈，完成=对勾（之前误显示为「很小很小的点」） */
.file-state {
  width: 24px;
  height: 24px;
  flex-shrink: 0;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  border-radius: 50%;
  background: var(--bg-2);
  color: var(--text-2);
}
.state-ring { width: 8px; height: 8px; border-radius: 50%; border: 1.5px solid var(--text-2); opacity: .7; }
.file-state.done { background: var(--accent-soft); color: var(--success); }
.file-state.receiving { background: var(--accent-soft); color: var(--accent); }
.file-main { flex: 1; min-width: 0; }
.file-name { font-size: 13px; color: var(--text-0); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.file-size { margin-top: 2px; font-size: 11px; color: var(--text-2); }
.file-tag { flex-shrink: 0; font-size: 10px; padding: 2px 7px; border-radius: 8px; background: var(--bg-2); color: var(--text-2); }
.file-tag.done { color: var(--success); background: var(--accent-soft); }
.file-tag.receiving { color: var(--accent); background: var(--accent-soft); }
.file-tag.expired { color: var(--text-2); background: var(--bg-2); }

/* 按钮贴住页面底部：上方留白自动撑开 */
.receive-actions { display: grid; grid-template-columns: repeat(2, 1fr); gap: 9px; padding: 12px 12px 4px; margin-top: auto; flex-shrink: 0; }

/* 设置 */
.settings-scroll { flex: 1; min-height: 0; overflow-y: auto; padding-bottom: 10px; }
.fp-cell :deep(.van-cell__value) { color: var(--accent); letter-spacing: 1px; }
.edit-icon { width: 24px; height: 24px; line-height: 0; vertical-align: middle; display: inline-flex; align-items: center; justify-content: center; border: none; background: transparent; color: var(--accent); cursor: pointer; }
.edit-icon svg { width: 20px; height: 20px; }
.settings-actions { padding: 10px 16px 8px; flex-shrink: 0; }

/* 底部页签：与页面/手势区同色，去掉下边框与硬投影，底部不再出现割裂的横线 */
.bottom-tabbar {
  flex-shrink: 0;
  margin-top: 0;
  /* Vant 默认固定 50px 高，这里改为按内容自适应，否则加大的内边距会被裁掉 */
  height: auto;
  background: var(--bg-0);
  box-shadow: 0 -1px 0 var(--border-soft);
  border-top: none;
}
/* 加高、加大内边距，图标与文字不再贴边 */
.bottom-tabbar :deep(.van-tabbar-item) { height: auto; padding: 11px 0 14px; font-size: 12px; }
.bottom-tabbar :deep(.van-tabbar-item__icon) { font-size: 22px; margin-bottom: 4px; }
.bottom-tabbar :deep(.van-tabbar-item__text) { line-height: 1.25; }

.sheet { background: var(--bg-1) !important; }

/* ---------- 居中对话框（文本 / 设备名 / 对方 IP） ----------
 * 浮在页面之上（overlay + fixed），不参与页面布局，也不改变主页面滚动位置。
 * 键盘不再压缩 WebView 视口（见 MainActivity），所以这里按 --kb-height 把
 * 对话框整体上移半个键盘高度：视觉上仍是「键盘上方那块区域的居中」，
 * 而底部页签留在原地被键盘盖住，不会跟着往上跳。 */
.van-popup--center.m-dialog {
  width: min(88vw, 360px);
  max-width: 360px;
  top: calc(50% - var(--kb-height, 0px) / 2);
  max-height: calc(100vh - var(--kb-height, 0px) - 96px);
  padding: 0;
  border-radius: 16px;
  background: var(--bg-1) !important;
  overflow: hidden;
  display: flex;
  flex-direction: column;
  transition: top .18s cubic-bezier(.22, .61, .36, 1);
}
.dialog-body { padding: 20px 18px 16px; display: flex; flex-direction: column; min-height: 0; overflow-y: auto; }
.dialog-title { font-size: 16px; font-weight: 600; color: var(--text-0); margin-bottom: 14px; text-align: center; }
.dialog-field { border-radius: 10px; background: var(--bg-2); }
.dialog-hint { margin: 10px 2px 0; font-size: 11px; line-height: 1.6; color: var(--text-2); }
.dialog-actions { display: flex; gap: 10px; margin-top: 16px; }
.dialog-actions .van-button { flex: 1; }
.text-field { border-radius: 10px; background: var(--bg-2); }

/* ---------- 全屏选择器（相册 / 最近文件 / 应用） ---------- */
.full-picker {
  height: 100%;
  display: flex;
  flex-direction: column;
  background: var(--bg-0) !important;
  border-radius: 0;
}
.picker-head {
  flex-shrink: 0;
  position: relative;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 10px 12px;
  background: var(--bg-0);
  border-bottom: 1px solid var(--border-soft);
}
.picker-back {
  width: 34px; height: 34px; flex-shrink: 0;
  display: inline-flex; align-items: center; justify-content: center;
  border: none; border-radius: 50%; background: var(--bg-2); color: var(--text-1); cursor: pointer;
}
.picker-title { flex: 1; min-width: 0; display: flex; flex-direction: column; }
.picker-title strong { font-size: 15px; font-weight: 600; color: var(--text-0); }
.picker-sub { margin-top: 1px; font-size: 11px; color: var(--text-2); }
.picker-done {
  flex-shrink: 0; padding: 7px 14px; border: none; border-radius: 16px;
  background: var(--accent); color: #fff; font-size: 13px; font-weight: 500; cursor: pointer;
}
.picker-done:disabled { background: var(--bg-3); color: var(--text-2); }
.picker-head-actions { display: flex; align-items: center; gap: 4px; }

/* 选择器内筛选条：相册（全部/照片/视频）、应用（搜索框） */
.picker-filter {
  flex-shrink: 0;
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 8px 12px;
  background: var(--bg-0);
  border-bottom: 1px solid var(--border-soft);
  overflow-x: auto;
  -webkit-overflow-scrolling: touch;
}
.picker-filter::-webkit-scrollbar { display: none; }
.filter-chip {
  flex-shrink: 0;
  display: inline-flex; align-items: center; gap: 5px;
  padding: 6px 13px; border: 1px solid var(--border-soft); border-radius: 16px;
  background: var(--bg-1); color: var(--text-1); font-size: 13px; cursor: pointer;
  transition: background .14s, color .14s, border-color .14s;
}
.filter-chip.on { background: var(--accent); border-color: var(--accent); color: #fff; }
.chip-count { font-size: 11px; opacity: .7; }
.app-search { padding: 8px 12px; overflow: visible; }
.search-box {
  flex: 1; display: flex; align-items: center; gap: 7px;
  padding: 0 12px; height: 36px;
  border-radius: 18px; background: var(--bg-2); color: var(--text-2);
}
.search-input {
  flex: 1; min-width: 0; height: 100%;
  border: none; outline: none; background: transparent;
  color: var(--text-0); font-size: 13px;
}
.search-input::-webkit-search-cancel-button { display: none; }
.search-clear {
  width: 20px; height: 20px; flex-shrink: 0;
  display: inline-flex; align-items: center; justify-content: center;
  border: none; border-radius: 50%; background: var(--bg-3); color: var(--text-1); cursor: pointer;
}
.picker-more {
  width: 32px; height: 32px; display: inline-flex; align-items: center; justify-content: center;
  border: none; border-radius: 50%; background: transparent; color: var(--text-1); cursor: pointer;
}
.app-menu {
  position: absolute; right: 10px; top: 52px; z-index: 5;
  min-width: 212px; padding: 4px 12px;
  border-radius: 12px; background: var(--bg-1);
  border: 1px solid var(--border-soft);
  box-shadow: 0 10px 28px rgba(0, 0, 0, .16);
}
.app-menu-row { display: flex; align-items: center; justify-content: space-between; gap: 14px; padding: 10px 0; font-size: 13px; color: var(--text-1); }
.app-menu-row + .app-menu-row { border-top: 1px solid var(--border-soft); }

.picker-scroll {
  flex: 1;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  -webkit-overflow-scrolling: touch;
  padding: 0 8px 12px;
}
.picker-hint { display: flex; align-items: center; justify-content: center; gap: 8px; padding: 70px 0; font-size: 13px; color: var(--text-2); }
.picker-foot { flex-shrink: 0; padding: 10px 14px 14px; border-top: 1px solid var(--border-soft); background: var(--bg-0); }

.section-label {
  position: sticky; top: 0; z-index: 2;
  padding: 12px 6px 8px;
  font-size: 12px; font-weight: 600; color: var(--text-1);
  background: linear-gradient(var(--bg-0) 72%, transparent);
}

/* 相册网格：3 列正方形，间距收紧到系统相册的观感 */
.media-section { padding-bottom: 2px; }
.media-grid { display: grid; grid-template-columns: repeat(3, 1fr); gap: 3px; }
.media-cell {
  position: relative;
  aspect-ratio: 1 / 1;
  padding: 0;
  border: none;
  border-radius: 4px;
  overflow: hidden;
  background: var(--bg-2);
  cursor: pointer;
}
.media-cell img { width: 100%; height: 100%; object-fit: cover; display: block; }
.media-cell.on::after {
  content: '';
  position: absolute;
  inset: 0;
  border: 2.5px solid var(--accent);
  border-radius: 4px;
  box-sizing: border-box;
  pointer-events: none;
}
.media-fallback { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; color: var(--text-2); }
.media-video {
  position: absolute; right: 5px; bottom: 5px; width: 18px; height: 18px; border-radius: 50%;
  background: rgba(0, 0, 0, .5); color: #fff; display: inline-flex; align-items: center; justify-content: center;
}
.media-check {
  position: absolute; left: 5px; top: 5px;
  min-width: 20px; height: 20px; padding: 0 5px;
  border-radius: 10px; border: 1.5px solid rgba(255, 255, 255, .9);
  background: rgba(0, 0, 0, .22);
  color: transparent; font-size: 11px; line-height: 18px; text-align: center;
  transition: background .12s, color .12s;
}
.media-check.show { background: var(--accent); border-color: var(--accent); color: #fff; }

/* 最近文件列表 */
.file-section { padding-bottom: 4px; }
.file-item {
  display: flex; align-items: center; gap: 10px; width: 100%;
  padding: 9px 10px; margin-bottom: 5px;
  border: 1px solid transparent; border-radius: 12px;
  background: var(--bg-1); color: inherit; text-align: left; cursor: pointer;
}
.file-item.on { border-color: var(--accent); background: var(--accent-soft); }
.file-ico {
  width: 34px; height: 34px; flex-shrink: 0;
  display: inline-flex; align-items: center; justify-content: center;
  border-radius: 9px; background: var(--bg-2); color: var(--text-1);
}
.file-ico.media { color: var(--accent); }
.file-ico.app { color: var(--warn); }
.file-ico.zip { color: var(--danger); }
.file-ico.audio { color: var(--success); }
.file-meta { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
.file-nm { font-size: 13px; color: var(--text-0); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.file-sub { font-size: 11px; color: var(--text-2); }
.file-check {
  width: 20px; height: 20px; flex-shrink: 0;
  display: inline-flex; align-items: center; justify-content: center;
  border-radius: 50%; border: 1.5px solid var(--border); color: #fff;
}
.file-check.show { background: var(--accent); border-color: var(--accent); }

/* 应用网格：4 列图标 + 名称 */
.app-scroll { padding: 8px 8px 16px; }
.app-grid { display: grid; grid-template-columns: repeat(4, 1fr); gap: 6px 2px; }
.app-cell {
  display: flex; flex-direction: column; align-items: center; gap: 5px;
  padding: 8px 4px; border: none; border-radius: 12px; background: transparent; cursor: pointer;
}
.app-cell.on { background: var(--accent-soft); }
.app-icon { position: relative; width: 46px; height: 46px; display: inline-flex; align-items: center; justify-content: center; }
.app-icon img { width: 46px; height: 46px; border-radius: 12px; display: block; }
.app-badge {
  position: absolute; right: -3px; top: -3px;
  min-width: 17px; height: 17px; padding: 0 4px;
  border-radius: 9px; border: 1.5px solid var(--bg-0);
  background: var(--accent); color: #fff; font-size: 10px; line-height: 14px; text-align: center;
}
.app-label { width: 100%; font-size: 11px; color: var(--text-1); text-align: center; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.app-size { font-size: 10px; color: var(--text-2); }

.name-dialog { width: 86%; max-width: 340px; border-radius: 14px; background: var(--bg-1) !important; }
.sheet-body { padding: 18px 18px 20px; }
.sheet-title { font-size: 16px; font-weight: 500; margin-bottom: 14px; }
.text-field { border-radius: 10px; background: var(--bg-2); }
.status-line { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 7px 0; font-size: 13px; }
.sheet-actions { display: flex; gap: 10px; margin-top: 16px; }
.sheet-actions .van-button { flex: 1; }
.fp-hint { font-size: 12px; color: var(--text-2); line-height: 1.6; margin-bottom: 10px; }
.fp-code { color: var(--accent); word-break: break-all; }
.mono { font-family: 'Cascadia Code', 'Consolas', monospace; }
.send-progress { margin: 14px 0; }
.spin { animation: spin .8s linear infinite; }
@keyframes spin { to { transform: rotate(360deg); } }
</style>

<!-- 非 scoped：需要命中 Vant 弹层根节点（弹层被 teleport 到 body） -->
<style>
/* 底部弹层里若有输入框（目前只有展示类弹层，留作兜底）：跟着键盘上移整段高度 */
.van-popup--bottom.keyboard-sheet {
  padding-bottom: var(--kb-height, 0px);
  transition: padding-bottom .2s cubic-bezier(.22, .61, .36, 1);
}

/* 弹层过渡：时长收紧 + 提前提升合成层，减少「弹出第一帧」的顿挫感 */
.van-popup-slide-bottom-enter-active,
.van-popup-slide-bottom-leave-active {
  transition-duration: .2s;
  transition-timing-function: cubic-bezier(.22, .61, .36, 1);
}
.van-popup { will-change: transform; backface-visibility: hidden; }
.van-overlay { will-change: opacity; transition-duration: .16s; }
</style>
