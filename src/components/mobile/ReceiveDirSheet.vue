<script setup lang="ts">
/**
 * 应用内「接收目录」弹层（移动端）。
 *
 * 为什么要有这个：恢复默认接收位置后目录是「下载/RunHelp」，之前是靠拼一个
 * document URI 让系统文件管理器去开，鸿蒙/EMUI 上会白屏卡死。这里直接把目录内容
 * 列出来（SAF 目录查子项 / 默认目录查 MediaStore），点条目就用带授权的 URI 打开文件，
 * 全程不依赖文件管理器。
 */
import { computed } from 'vue';
import { Popup as VanPopup, Loading as VanLoading, Empty as VanEmpty, Icon as VanIcon } from 'vant';
import { useSyncContext } from '../../lib/useSync';

const {
  t, receiveDirOpen, receiveDirLoading, receiveDirItems, receiveDirLabel,
  loadSavedFiles, openSavedFile, openInFileManager, fmtSize, receiveDestLabel,
} = useSyncContext();

const items = computed(() => receiveDirItems.value);

function when(ts: number) {
  if (!ts) return '';
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return '';
  return `${d.getMonth() + 1}/${d.getDate()}`;
}
</script>

<template>
  <van-popup
    v-model:show="receiveDirOpen"
    teleport="body"
    position="bottom"
    round
    class="sheet dir-sheet"
  >
    <div class="dir-head">
      <div class="dir-title">
        <strong>{{ receiveDirLabel || receiveDestLabel() }}</strong>
        <span class="muted tiny">{{ t('已接收的文件') }} · {{ items.length }}</span>
      </div>
      <div class="dir-actions">
        <button type="button" class="dir-link" @click="loadSavedFiles">{{ t('刷新') }}</button>
        <button type="button" class="dir-link" @click="openInFileManager">{{ t('文件管理器') }}</button>
      </div>
    </div>

    <div class="dir-scroll">
      <div v-if="receiveDirLoading" class="dir-empty"><van-loading size="18" /></div>
      <van-empty v-else-if="!items.length" :description="t('这里还没有收到的文件')" />
      <button
        v-for="(f, i) in items"
        :key="`${f.uri}-${i}`"
        type="button"
        class="dir-row"
        @click="openSavedFile(f)"
      >
        <van-icon name="description" size="17" class="dir-icon" />
        <span class="dir-name">{{ f.name }}</span>
        <span class="dir-meta">{{ fmtSize(f.size) }}<template v-if="when(f.date)"> · {{ when(f.date) }}</template></span>
      </button>
    </div>

    <div class="sheet-actions dir-foot">
      <van-button block @click="receiveDirOpen = false">{{ t('关闭') }}</van-button>
    </div>
  </van-popup>
</template>
