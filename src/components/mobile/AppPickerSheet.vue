<script setup lang="ts">
/**
 * 已安装应用列表（发 APK），默认排除系统应用 / 无法启动的应用（与 LocalSend 一致）。
 * 同样单独成组件：图标是 base64，几十上百个格子不该跟着整页一起 diff。
 */
import { computed } from 'vue';
import { Icon as VanIcon, Loading as VanLoading, Popup as VanPopup, Switch as VanSwitch } from 'vant';
import { useSyncContext } from '../../lib/useSync';

const props = defineProps<{ query: string; menuOpen: boolean }>();
const emit = defineEmits<{
  (e: 'update:query', v: string): void;
  (e: 'update:menuOpen', v: boolean): void;
}>();

const {
  t, appPickerOpen, appLoading, appItems, appSelected, busy,
  appExcludeSystem, appExcludeNonLaunchable, loadApps, toggleApp, confirmApps, fmtSize,
} = useSyncContext();

const query = computed({
  get: () => props.query,
  set: (v: string) => emit('update:query', v),
});
const menuOpen = computed({
  get: () => props.menuOpen,
  set: (v: boolean) => emit('update:menuOpen', v),
});

/** 名称/包名即时过滤 */
const visible = computed(() => {
  const q = props.query.trim().toLowerCase();
  if (!q) return appItems.value;
  return appItems.value.filter((a) => a.label.toLowerCase().includes(q) || a.package.toLowerCase().includes(q));
});

async function toggleFilter(kind: 'system' | 'nonlaunch', value: boolean) {
  if (kind === 'system') appExcludeSystem.value = value;
  else appExcludeNonLaunchable.value = value;
  await loadApps();
}
</script>

<template>
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
        <button class="picker-more" :aria-label="t('筛选')" @click="menuOpen = !menuOpen">
          <van-icon name="ellipsis" size="18" />
        </button>
      </div>
      <div v-if="menuOpen" class="app-menu" @click.stop>
        <label class="app-menu-row">
          <span>{{ t('排除系统应用') }}</span>
          <van-switch size="18" :model-value="appExcludeSystem" @update:model-value="(v: boolean) => toggleFilter('system', v)" />
        </label>
        <label class="app-menu-row">
          <span>{{ t('排除无法启动的应用') }}</span>
          <van-switch size="18" :model-value="appExcludeNonLaunchable" @update:model-value="(v: boolean) => toggleFilter('nonlaunch', v)" />
        </label>
      </div>
    </div>

    <div class="picker-filter app-search">
      <div class="search-box">
        <van-icon name="search" size="15" />
        <input
          v-model="query"
          type="search"
          :placeholder="t('搜索应用名称')"
          class="search-input"
        />
        <button v-if="query" class="search-clear" :aria-label="t('清除')" @click="query = ''">
          <van-icon name="cross" size="12" />
        </button>
      </div>
    </div>

    <div class="picker-scroll app-scroll" @scroll="menuOpen = false">
      <div v-if="appLoading" class="picker-hint">
        <van-loading size="20" />
        <span>{{ t('正在读取应用列表') }}</span>
      </div>
      <div v-else-if="!appItems.length" class="picker-hint">{{ t('没有匹配的应用') }}</div>
      <div v-else-if="!visible.length" class="picker-hint">{{ t('没有匹配的应用') }}</div>
      <div v-else class="app-grid">
        <button
          v-for="a in visible"
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
</template>
