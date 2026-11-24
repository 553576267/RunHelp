<script setup lang="ts">
/**
 * 应用内相册（图片 + 视频混选），全屏、按拍摄日期分组。
 *
 * 200 张缩略图的网格单独成组件：以前它长在 1300 行的页面组件里，
 * 任何一次进度事件都会把这 200 个格子重新 diff 一遍。现在只有选中态
 * 变化时它自己重绘，页面其余部分完全不参与。
 */
import { computed, ref, watch } from 'vue';
import { Icon as VanIcon, Loading as VanLoading, Popup as VanPopup } from 'vant';
import { useSyncContext } from '../../lib/useSync';

type MediaFilter = 'all' | 'photo' | 'video';

const props = defineProps<{ filter: MediaFilter }>();
const emit = defineEmits<{ (e: 'update:filter', v: MediaFilter): void }>();

const { t, mediaPickerOpen, mediaLoading, mediaItems, mediaSelected, busy, toggleMedia, confirmMedia } = useSyncContext();

const filter = computed({
  get: () => props.filter,
  set: (v: MediaFilter) => emit('update:filter', v),
});

/* ---------- 日期分组 ---------- */
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

/** 按日期切片（列表本身已按时间倒序） */
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

/* 筛选：全部 / 只看照片 / 只看视频（本地过滤，不再查一次 MediaStore） */
const filtered = computed(() => mediaItems.value.filter((m) => {
  if (filter.value === 'photo') return !m.video;
  if (filter.value === 'video') return m.video;
  return true;
}));

/* 渐进渲染：首帧只铺 60 个格子，滚到接近底部再追加。
 * 200 张 base64 缩略图一次性建 DOM 是「点开相册卡一下」的主因，
 * 分批后首帧工作量降到 1/3，剩下的在滚动过程中自然补齐（用户感知不到）。 */
const PAGE = 60;
const limit = ref(PAGE);
const visible = computed(() => filtered.value.slice(0, limit.value));
const visibleGroups = computed(() => groupByDay(visible.value));

// 换筛选 / 每次重新打开都从第一批开始
watch([filter, mediaPickerOpen], () => { limit.value = PAGE; });
watch(mediaItems, () => { limit.value = PAGE; });

function onScroll(e: Event) {
  const el = e.target as HTMLElement;
  if (el.scrollTop + el.clientHeight < el.scrollHeight - 240) return;
  if (limit.value >= filtered.value.length) return;
  limit.value = Math.min(filtered.value.length, limit.value + PAGE);
}

const filterOptions = computed(() => [
  { key: 'all' as const, label: t('全部'), count: mediaItems.value.length },
  { key: 'photo' as const, label: t('照片'), count: mediaItems.value.filter((m) => !m.video).length },
  { key: 'video' as const, label: t('视频'), count: mediaItems.value.filter((m) => m.video).length },
]);
</script>

<template>
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
        v-for="f in filterOptions"
        :key="f.key"
        class="filter-chip"
        :class="{ on: filter === f.key }"
        @click="filter = f.key"
      >{{ f.label }}<span class="chip-count">{{ f.count }}</span></button>
    </div>

    <div class="picker-scroll" @scroll.passive="onScroll">
      <div v-if="mediaLoading" class="picker-hint">
        <van-loading size="20" />
        <span>{{ t('正在读取相册') }}</span>
      </div>
      <div v-else-if="!mediaItems.length" class="picker-hint">{{ t('相册里没有可发送的图片或视频') }}</div>
      <div v-else-if="!visible.length" class="picker-hint">{{ t('该类内容为空') }}</div>
      <template v-else>
        <section v-for="g in visibleGroups" :key="g.label" class="media-section">
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
</template>
