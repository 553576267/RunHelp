<script setup lang="ts">
import { computed, onBeforeUnmount, ref, watch } from 'vue';
import {
  NavBar as VanNavBar, Tabbar as VanTabbar, TabbarItem as VanTabbarItem,
  Button as VanButton, Icon as VanIcon, CellGroup as VanCellGroup, Cell as VanCell,
  Field as VanField, Popup as VanPopup, Switch as VanSwitch, Loading as VanLoading, Picker as VanPicker,
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
import { useSync, provideSyncContext } from '../lib/useSync';
import { locale } from '../lib/i18n';
import MobileReceivePane from './mobile/MobileReceivePane.vue';
import SendStatusSheet from './mobile/SendStatusSheet.vue';
import MediaPickerSheet from './mobile/MediaPickerSheet.vue';
import AppPickerSheet from './mobile/AppPickerSheet.vue';
import RequestSheets from './mobile/RequestSheets.vue';
import ReceiveDirSheet from './mobile/ReceiveDirSheet.vue';
// 与拆出去的子组件共用的一套样式（非 scoped，集中维护）
import './mobile/mobile.css';

/* 状态只在这里创建一次，子组件通过 useSyncContext() 拿到同一份，
 * 这样进度事件只会重绘弹层自己，不会把整页模板 diff 一遍。 */
const sync = useSync();
provideSyncContext(sync);

const {
  t, view, status, textInput, pairIp, editingName, nameDraft, busy, settings,
  activeAction, isActionActive, fpDialog, requestDialog,
  sendVisible, sendCanceling, deviceList, selectedId, selectedDevice, displayName,
  refreshDevices, refreshIncoming, saveName, openReceiveDir,
  changeTheme, changeKeepAlive, changeLanguage, changeReceiveDir, resetReceiveDir,
  openSavedDir, receiveDirOpen,
  doPair, toggleFavorite, removeDevice, selectDevice, sendText, pickFiles,
  mediaPickerOpen, pickMediaEntry,
  browseFiles,
  appPickerOpen, pickAppEntry,
  approveReceive, approveFingerprint, rejectReceive, receiveAllRequests, openIncoming,
  incomingViewReq, kindLabel, platformLabel, shortFp,
} = sync;

/* 键盘避让由原生 decorView 内边距统一处理（见 MainActivity），
 * 这里不再叠加 visualViewport 计算，否则弹层会被抬高两次。 */
// 文本弹层：等弹出动画结束再聚焦，键盘与弹层同步上升，避免动画中途抖动
const textFieldRef = ref<{ focus: () => void } | null>(null);
function focusTextField() {
  textFieldRef.value?.focus();
}


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

/* ---------- 选择器界面状态（列表数据本身在子组件里读，这里只保留跨组件的那几个） ---------- */
// 相册筛选：全部 / 只看照片 / 只看视频；点「媒体」时回到「全部」
const mediaFilter = ref<'all' | 'photo' | 'video'>('all');
// 应用搜索：一堆图标里翻名字太慢，顶部给个即时过滤框
const appQuery = ref('');
// 应用列表右上角筛选菜单（返回手势要能关它，所以状态留在父组件）
const appMenuOpen = ref(false);

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
  { open: () => receiveDirOpen.value, close: () => (receiveDirOpen.value = false) },
  {
    open: () => !!fpDialog.value,
    close: () => {
      if (fpDialog.value) void rejectReceive(fpDialog.value);
    },
  },
  {
    open: () => !!requestDialog.value,
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
    <div v-show="view === 'send'" class="m-body send-view">
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

    <!-- 接收（独立组件：进度事件只重绘它自己） -->
    <MobileReceivePane v-if="view === 'receive'" />

    <!-- 设置 -->
    <div v-if="view === 'settings'" class="m-body settings-view">
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

    <!-- 发送状态（独立组件：发送进度只重绘它自己） -->
    <SendStatusSheet />

    <!-- 应用内相册（独立组件：200 个缩略图不参与整页 diff） -->
    <MediaPickerSheet v-model:filter="mediaFilter" />

    <!-- 已安装应用（独立组件） -->
    <AppPickerSheet v-model:query="appQuery" v-model:menu-open="appMenuOpen" />

    <!-- 接收请求 / 指纹确认（独立组件：请求到达不再牵动整页重绘） -->
    <RequestSheets />

    <!-- 应用内接收目录（替代「让文件管理器去开拼出来的目录 URI」，那条路会白屏） -->
    <ReceiveDirSheet />

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

.name-dialog { width: 86%; max-width: 340px; border-radius: 14px; background: var(--bg-1) !important; }
</style>

<!-- 非 scoped：需要命中 Vant 弹层根节点（弹层被 teleport 到 body） -->
<style>
/* 底部弹层里若有输入框（目前只有展示类弹层，留作兜底）：跟着键盘上移整段高度 */
.van-popup--bottom.keyboard-sheet {
  padding-bottom: var(--kb-height, 0px);
  transition: padding-bottom .2s cubic-bezier(.22, .61, .36, 1);
}

/* 弹层过渡：时长收紧 + **只在动画期间**提升合成层。
 * 之前把 `will-change: transform` 常驻在所有 .van-popup 上：相册弹层里有 200 张缩略图，
 * 常驻合成层会占掉大块 GPU 显存，反而让「弹出第一帧」掉帧；改成只在 enter/leave
 * 的 active 阶段声明，动画结束自动释放。 */
.van-popup-slide-bottom-enter-active,
.van-popup-slide-bottom-leave-active {
  transition-duration: .2s;
  transition-timing-function: cubic-bezier(.22, .61, .36, 1);
  will-change: transform;
  backface-visibility: hidden;
}
.van-overlay { transition-duration: .16s; }
.van-overlay-enter-active,
.van-overlay-leave-active { will-change: opacity; }
</style>
