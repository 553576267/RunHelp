<script setup lang="ts">
/**
 * 发送状态页（移动端）。
 *
 * 形态参考 LocalSend 的 ProgressPage：**整页进度视图**，不是压在设备列表上的弹层。
 * 弹层要用 transform 做入场动画，动画那几帧里只要有数据写入（进度事件、清单构建）
 * 就会掉帧；整页视图没有入场位移，只做一次淡入，天然不抢帧。
 * 另外它单独成组件，`sendModal` 更新只重绘这一页，不牵动发送页/接收页/六个选择器。
 */
import { computed } from 'vue';
import { Button as VanButton, Icon as VanIcon, Progress as VanProgress } from 'vant';
import { useSyncContext } from '../../lib/useSync';

const { t, sendModal, sendVisible, sendCanceling, cancelSend, stageLabel, fmtSize } = useSyncContext();

/* 内容常驻（无数据时用占位对象）：首帧不再临时建 DOM */
const view = computed(() => sendModal.value ?? {
  transferId: '', ip: '', targetName: '—', kind: 'file',
  items: [] as { name: string; size: number }[],
  stage: 'waiting' as const, detail: '', sent: 0, total: 0,
});
const percent = computed(() => {
  const m = sendModal.value;
  return m && m.total ? Math.min(100, Math.round(m.sent / m.total * 100)) : 0;
});
const finished = computed(() => ['done', 'cancelled', 'error'].includes(view.value.stage));
/** 传输中显示「已发送 / 总量」，其余状态显示条目数 */
const meta = computed(() => {
  const m = view.value;
  if (m.stage === 'transferring') return `${fmtSize(m.sent)} / ${fmtSize(m.total)}`;
  if (m.kind === 'text' || m.kind === 'clipboard') return t('文字');
  const n = m.items.length;
  return n > 1 ? t('{count} 个文件', { count: String(n) }) : (m.items[0]?.name ?? '');
});
</script>

<template>
  <div v-if="sendVisible" class="send-page">
    <div class="send-page-head">
      <span class="send-page-title">{{ t('发送状态') }}</span>
      <button class="send-page-close" type="button" :aria-label="t('关闭')" @click="sendVisible = false">
        <van-icon name="cross" size="18" />
      </button>
    </div>

    <div class="send-page-body">
      <div class="send-page-device">{{ view.targetName }}</div>
      <div class="send-page-state" :class="view.stage">{{ stageLabel(view) }}</div>
      <div class="send-page-percent" v-if="!finished">{{ percent }}<span>%</span></div>
      <van-progress
        v-if="view.stage === 'transferring'"
        :percentage="percent"
        :show-pivot="false"
        stroke-width="6"
        class="send-page-bar"
      />
      <div class="send-page-meta">{{ meta }}</div>
      <div v-if="view.detail" class="send-page-detail">{{ view.detail }}</div>
    </div>

    <div class="send-page-actions">
      <van-button
        v-if="!finished"
        plain
        type="danger"
        block
        :disabled="sendCanceling"
        @click="cancelSend"
      >{{ sendCanceling ? t('撤销中') : t('取消发送') }}</van-button>
      <van-button type="primary" block @click="sendVisible = false">{{ t('关闭') }}</van-button>
    </div>
  </div>
</template>
