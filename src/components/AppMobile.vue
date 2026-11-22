<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { invoke } from '@tauri-apps/api/core';
import { isPermissionGranted, requestPermission } from '@tauri-apps/plugin-notification';
import SyncView from './SyncView.vue';
import { setLocale } from '../lib/i18n';

const ready = ref(false);

onMounted(async () => {
  // 移动端跟随系统语言：中文环境用中文，其余用英文（与桌面端设置页独立）。
  try {
    const navLang = navigator.language || 'zh-CN';
    setLocale(navLang.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en');
  } catch {
    /* ignore */
  }
  // Android 13+ 需运行时申请通知权限，否则收到传输请求时系统通知发不出来。
  try {
    if (!(await isPermissionGranted())) {
      await requestPermission();
    }
  } catch {
    /* ignore */
  }
  ready.value = true;
});

// 预留：未来若要移动端也调用后端能力，统一从这里 invoke。
void invoke;
</script>

<template>
  <div class="mobile-app">
    <SyncView />
  </div>
</template>

<style>
/* 全局（非 scoped）：覆盖 SyncView 内部桌面双栏布局，适配手机窄屏 */
html, body, #app { overscroll-behavior: none; }

.mobile-app {
  height: 100%;
  width: 100%;
  padding-top: env(safe-area-inset-top);
  padding-bottom: env(safe-area-inset-bottom);
  background: var(--bg-0);
}

.mobile-app .sync-page { padding: 12px; }
.mobile-app .page-head h2 { font-size: 17px; }

/* 发送页：桌面左右双栏 → 手机上下单栏，各自高度自适应、整体可滚动 */
.mobile-app .view-body { min-height: 0; flex: 1 1 auto; overflow-y: auto; display: block; }
.mobile-app .send-layout {
  display: flex;
  flex-direction: column;
  gap: 12px;
  min-height: 0;
}
.mobile-app .device-panel,
.mobile-app .compose-panel,
.mobile-app .receive-panel { flex: 0 0 auto; min-height: 0; }
.mobile-app .device-list { max-height: 260px; }
.mobile-app .action-grid { grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 6px; }
.mobile-app .action-card { height: 72px; font-size: 11px; }
.mobile-app .textarea { min-height: 120px; }

/* 设置页：桌面双列 → 手机单列 */
.mobile-app .settings-grid { grid-template-columns: 1fr; overflow-y: auto; }

/* 弹窗：桌面固定宽度 → 手机贴近屏宽 */
.mobile-app .modal-mask { padding: 16px; }
.mobile-app .modal { max-width: 100%; max-height: 90vh; overflow-y: auto; }
.mobile-app .send-modal,
.mobile-app .request-modal,
.mobile-app .confirm-modal { width: 100%; }

/* 页脚小字可适当缩小 */
.mobile-app .entry-card { height: 46px; }
</style>
