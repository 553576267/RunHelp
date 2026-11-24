<script setup lang="ts">
/**
 * 接收请求 / 指纹确认弹层（移动端）。
 *
 * 单独成组件的理由和前几个一样：这两个弹层原来写在 SyncViewMobile 里，
 * `requestDialog` 一变就会让「发送页 + 设置页 + 六个弹层」整棵模板重新 diff 一遍，
 * 而请求到达的瞬间正好是弹层入场动画，两边抢同一帧就表现为「弹出时卡一下」。
 * 移出来后只有这两个弹层参与重绘。
 */
import { computed } from 'vue';
import { Button as VanButton, Popup as VanPopup } from 'vant';
import { useSyncContext } from '../../lib/useSync';

const {
  t, requestDialog, fpDialog,
  kindLabel, shortFp, rejectReceive, approveFingerprint, approveReceive, openIncoming,
} = useSyncContext();

const show = computed(() => !!requestDialog.value);
const fpShow = computed(() => !!fpDialog.value);

/* 内容常驻：无数据时用占位对象，避免首次弹出时临时建 DOM */
const view = computed(() => requestDialog.value ?? {
  id: '', transfer_id: '', from: '—', from_platform: '', fingerprint: '', trusted: true,
  kind: 'file', name: '', size: 0, count: 0, items: [] as { name: string; size: number }[],
});
</script>

<template>
  <van-popup
    v-model:show="show"
    :lazy-render="false"
    teleport="body"
    position="bottom"
    round
    :close-on-click-overlay="false"
    class="sheet"
  >
    <div class="sheet-body">
      <div class="sheet-title">{{ t('收到传输请求') }}</div>
      <div class="status-line"><span class="muted">{{ t('发送方') }}</span><strong>{{ view.from }}</strong></div>
      <div class="status-line"><span class="muted">{{ t('类型') }}</span><strong>{{ kindLabel(view.kind) }}</strong></div>
      <div class="status-line"><span class="muted">{{ t('发送文件数量') }}</span><strong>{{ view.count }}</strong></div>
      <div class="sheet-actions three">
        <van-button plain @click="requestDialog && rejectReceive(requestDialog)">{{ t('拒绝') }}</van-button>
        <van-button plain type="primary" @click="openIncoming()">{{ t('查看清单') }}</van-button>
        <van-button type="primary" @click="requestDialog && approveReceive(requestDialog)">{{ t('接收') }}</van-button>
      </div>
    </div>
  </van-popup>

  <van-popup
    v-model:show="fpShow"
    :lazy-render="false"
    teleport="body"
    position="bottom"
    round
    :close-on-click-overlay="false"
    class="sheet"
  >
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
</template>
