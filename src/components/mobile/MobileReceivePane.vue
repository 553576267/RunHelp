<script setup lang="ts">
/**
 * 接收页（移动端）。
 *
 * 单独成组件的原因：`activeReceive` 在传输过程中每 200ms 就会被进度事件改写，
 * 如果它被写在一个 1300 行的大组件模板里，每次都会把发送页、设置页、六个弹层
 * 一起 diff 一遍——这正是「接收/发送弹层卡顿」的主因。这里只读自己需要的字段，
 * 重绘范围收敛到摘要卡 + 文件列表这两块。
 */
import { onMounted, watch } from 'vue';
import {
  Button as VanButton, Icon as VanIcon, Progress as VanProgress,
  Empty as VanEmpty, Loading as VanLoading,
} from 'vant';
import { useSyncContext } from '../../lib/useSync';

const {
  t, activeReceive, receiveTaskPercent, incomingViewReq,
  receiveStageLabel, platformLabel, shortFp, kindLabel, fmtSize,
  refreshIncoming, openSavedDir, rejectReceive, receiveAllRequests, isActionActive,
  loadReceiveItems,
} = useSyncContext();

// 明细按需拉：接收页第一次挂载/切换到新的待接收任务时才拉，避免请求弹层被几百项清单拖慢
onMounted(() => { void loadReceiveItems(); });
watch(() => activeReceive.value?.transferId, () => { void loadReceiveItems(); });

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
</script>

<template>
  <div class="m-body receive-view">
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
        <span class="file-count">{{ activeReceive.itemCount }} {{ t('个文件') }} · {{ fmtSize(activeReceive.total) }}</span>
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
        <div v-if="!activeReceive.itemsLoaded" class="file-row placeholder">
          <van-loading size="14" />
          <div class="file-main"><div class="file-name">{{ t('正在读取文件清单…') }}</div></div>
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
</template>
