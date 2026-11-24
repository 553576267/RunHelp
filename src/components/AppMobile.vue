<script setup lang="ts">
import { onMounted, provide, ref } from 'vue';
import { isPermissionGranted, requestPermission } from '@tauri-apps/plugin-notification';
import { showToast, showConfirmDialog } from 'vant';
import { api } from '../lib/api';
import SyncViewMobile from './SyncViewMobile.vue';
import { setLocale, t } from '../lib/i18n';
import { useKeyboardAvoid } from '../lib/useKeyboardAvoid';

const ready = ref(false);

// 键盘不再压缩原生窗口（底部页签原地不动），需要避让的输入弹层靠 --kb-height 上移
useKeyboardAvoid();

// 移动端同步逻辑通过 inject 拿 notify / confirm，这里提供 Vant 实现
provide('notify', (msg: string) => { showToast(msg); });
provide('confirm', async (msg: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => {
  try {
    await showConfirmDialog({
      title: o?.title || t('确认'),
      message: msg,
      confirmButtonText: o?.confirmText || t('确认'),
      cancelButtonText: t('取消'),
      confirmButtonColor: o?.danger ? 'var(--danger)' : 'var(--accent)',
    });
    return true;
  } catch {
    return false;
  }
});

onMounted(async () => {
  // 语言：优先用用户在设置页保存的 language，否则跟随系统。
  let lang = '';
  let theme = 'soft-light';
  try {
    const settings = await api.getSettings();
    if (settings.language) lang = settings.language;
    if (settings.mobile_theme) theme = settings.mobile_theme;
  } catch { /* ignore */ }
  if (!lang) {
    const navLang = navigator.language || 'zh-CN';
    lang = navLang.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en-US';
  }
  setLocale(lang);
  // 柔光浅：参考色 #E8F0E9，移动端默认护眼配色，避免深色页面与系统浅色导航栏产生白边。
  document.documentElement.setAttribute('data-theme', theme);
  try { await api.mobileSetTheme(theme); } catch { /* ignore */ }
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
</script>

<template>
  <div class="mobile-app">
    <SyncViewMobile v-if="ready" />
  </div>
</template>

<style>
/* 移动端基础样式：Vant 组件样式由 SyncViewMobile.vue 按需引入 */
html, body, #app { overscroll-behavior: none; height: 100%; }

/* 系统栏/键盘避让交给原生 decorView 内边距处理，这里不再重复加 safe-area 内边距，
 * 否则底部会出现两层空白。 */
.mobile-app {
  height: 100%;
  width: 100%;
  background: var(--bg-0);
  overflow: hidden;
}
</style>
