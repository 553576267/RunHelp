<script setup lang="ts">
import { useSync } from '../lib/useSync';

const {
  t, view, status, devices, selectedId, textInput, pairIp, editingName, nameDraft, busy, isMobile,
  activeAction, isActionActive, activeReceive, fpDialog, requestDialog,
  sendModal, sendCanceling, deviceList, selectedDevice, incomingCount, displayName, receiveTaskPercent,
  refreshDevices, refreshIncoming, saveName, setReceiveDir, openReceiveDir,
  doPair, toggleFavorite, removeDevice, selectDevice, sendText, pickFiles, cancelSend,
  approveReceive, approveFingerprint, rejectReceive, receiveAllRequests,
  incomingViewReq, openIncoming, fmtSize, kindLabel, platformLabel, shortFp, receiveStageLabel, stageLabel,
} = useSync();
</script>

<template>
  <div class="sync-page">
    <div class="page-head">
      <div>
        <h2>{{ t('数据同步') }}</h2>
        <p class="muted">{{ t('同一局域网内互传文件、文件夹、文字与剪贴板，无需联网') }}</p>
      </div>
    </div>

    <div class="entry-row">
      <button class="entry-card" :class="{ active: view === 'send' }" @click="view = 'send'">
        <svg class="entry-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21.5 2.5 10.8 13.2" /><path d="M21.5 2.5 15 21l-4.2-7.8L3 9l18.5-6.5Z" />
        </svg>
        <span class="entry-label">{{ t('发送') }}</span>
      </button>
      <button class="entry-card" :class="{ active: view === 'receive' }" @click="view = 'receive'">
        <svg class="entry-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M12 3v12" /><path d="m7 10 5 5 5-5" /><path d="M4 20h16" />
        </svg>
        <span class="entry-label">{{ t('接收') }}</span>
        <span v-if="incomingCount" class="entry-badge">{{ incomingCount }}</span>
      </button>
      <button class="entry-card" :class="{ active: view === 'settings' }" @click="view = 'settings'">
        <svg class="entry-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <circle cx="12" cy="12" r="3" />
          <path d="M19.4 15a1.7 1.7 0 0 0 .34 1.87l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.7 1.7 0 0 0-1.87-.34 1.7 1.7 0 0 0-1.03 1.56V21a2 2 0 1 1-4 0v-.08a1.7 1.7 0 0 0-1.11-1.56 1.7 1.7 0 0 0-1.87.34l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.7 1.7 0 0 0 .34-1.87 1.7 1.7 0 0 0-1.56-1.03H3a2 2 0 1 1 0-4h.08a1.7 1.7 0 0 0 1.56-1.11 1.7 1.7 0 0 0-.34-1.87l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.7 1.7 0 0 0 1.87.34H9a1.7 1.7 0 0 0 1.03-1.56V3a2 2 0 1 1 4 0v.08a1.7 1.7 0 0 0 1.03 1.56 1.7 1.7 0 0 0 1.87-.34l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.7 1.7 0 0 0-.34 1.87V9a1.7 1.7 0 0 0 1.56 1.03H21a2 2 0 1 1 0 4h-.08a1.7 1.7 0 0 0-1.52 1.03Z" />
        </svg>
        <span class="entry-label">{{ t('设置') }}</span>
      </button>
    </div>

    <!-- 发送视图：左设备，右内容 -->
    <div v-show="view === 'send'" class="view-body">
      <div class="send-layout">
        <section class="card pad device-panel">
          <div class="section-title row-between">
            <span>{{ t('附近设备') }}</span>
            <button class="btn sm refresh-btn" :disabled="isActionActive('refresh-devices')" @click="refreshDevices">
              <svg class="refresh-icon" :class="{ spin: isActionActive('refresh-devices') }" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              {{ t('刷新') }}
            </button>
          </div>
          <div class="device-list">
            <div v-if="!deviceList.length" class="muted empty-hint">
              {{ t('未发现设备，请确认对方也打开了数据同步页，或在设置里手动添加') }}
            </div>
            <div
              v-for="d in deviceList"
              :key="d.id"
              class="device-card"
              :class="{ selected: selectedId === d.id, offline: !d.online, fav: d.favorite }"
              @click="selectDevice(d)"
            >
              <div class="device-main">
                <span class="dot" :class="{ on: d.online }"></span>
                <strong>{{ displayName(d) }}</strong>
                <span class="muted tiny mono">{{ d.online ? d.ip : (t('离线') + ' · ' + d.last_ip) }}</span>
                <div class="device-actions">
                  <button
                    class="icon-btn favorite-btn"
                    :class="{ active: d.favorite }"
                    :title="d.favorite ? t('取消收藏') : t('收藏')"
                    :aria-label="d.favorite ? t('取消收藏') : t('收藏')"
                    @click.stop="toggleFavorite(d)"
                  >
                    <svg viewBox="0 0 24 24" :fill="d.favorite ? 'currentColor' : 'none'" stroke="currentColor" stroke-width="1.7" stroke-linejoin="round">
                      <path d="m12 2.6 2.9 5.9 6.5.95-4.7 4.6 1.1 6.45L12 17.45l-5.8 3.05 1.1-6.45-4.7-4.6 6.5-.95L12 2.6Z" />
                    </svg>
                  </button>
                  <button class="icon-btn remove-btn" :title="t('删除设备')" :aria-label="t('删除设备')" @click.stop="removeDevice(d)">
                    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.7" stroke-linecap="round" stroke-linejoin="round">
                      <path d="M4 7h16M10 4h4M6 7l1 13h10l1-13M10 11v5M14 11v5" />
                    </svg>
                  </button>
                </div>
              </div>
              <div class="device-sub">
                <span class="muted tiny mono">{{ platformLabel(d.platform) }} · {{ shortFp(d.fingerprint) }}</span>
              </div>
            </div>
          </div>
        </section>

        <section class="card pad compose-panel">
          <div class="section-title">{{ t('发送内容') }}</div>
          <div class="action-grid">
            <button class="action-card" :disabled="!selectedDevice || busy" @click="pickFiles('file')">
              <svg v-if="busy && activeAction === 'file'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">📄</span>
              <span>{{ t('文件') }}</span>
            </button>
            <button class="action-card" :disabled="!selectedDevice || busy" @click="pickFiles('folder')">
              <svg v-if="busy && activeAction === 'folder'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">📁</span>
              <span>{{ t('文件夹') }}</span>
            </button>
            <button class="action-card" :disabled="!selectedDevice || busy" @click="sendText('clipboard')">
              <svg v-if="busy && activeAction === 'clipboard'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">📋</span>
              <span>{{ t('剪贴板') }}</span>
            </button>
            <button class="action-card" :disabled="!selectedDevice || busy" @click="sendText('text')">
              <svg v-if="busy && activeAction === 'text'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              <span v-else class="action-icon">✎</span>
              <span>{{ t('文字') }}</span>
            </button>
          </div>
          <div class="text-block">
            <label>{{ t('文字内容') }}</label>
            <textarea v-model="textInput" class="input textarea" :placeholder="t('输入要发送的文字')"></textarea>
          </div>
        </section>
      </div>
    </div>

    <!-- 接收视图：只展示最近一次接收任务的清单与进度 -->
    <div v-show="view === 'receive'" class="view-body">
      <section class="card pad receive-panel">
        <div class="section-title row-between">
          <span>{{ t('最近接收') }}</span>
          <div class="refresh-wrap">
            <button class="btn sm" :title="t('打开接收目录')" @click="openReceiveDir">{{ t('打开接收目录') }}</button>
            <button class="btn sm refresh-btn" :disabled="isActionActive('refresh-incoming')" @click="refreshIncoming">
              <svg class="refresh-icon" :class="{ spin: isActionActive('refresh-incoming') }" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
              </svg>
              {{ t('刷新') }}
            </button>
            <button class="btn sm primary" v-if="incomingViewReq" @click="receiveAllRequests">
              {{ t('全部接收') }}
            </button>
          </div>
        </div>

        <div v-if="!activeReceive" class="muted empty-hint">{{ t('暂无待接收的内容') }}</div>

        <div v-else class="receive-task" :class="{ rejected: activeReceive.stage === 'rejected' }">
          <div class="request-head">
            <div class="request-from">
              <strong>{{ activeReceive.from }}</strong>
              <span class="muted tiny mono">{{ platformLabel(activeReceive.fromPlatform) }} · {{ shortFp(activeReceive.fingerprint) }}</span>
            </div>
            <span class="tag" :class="activeReceive.trusted ? 'ok' : 'warn'">
              {{ activeReceive.trusted ? t('可信设备') : t('首次连接') }}
            </span>
          </div>
          <div class="request-meta">
            <span>{{ kindLabel(activeReceive.kind) }}</span>
            <span class="muted">{{ fmtSize(activeReceive.total) }}</span>
            <span class="muted">{{ activeReceive.itemCount }} {{ t('个文件') }}</span>
            <span class="tag" :class="{
              ok: activeReceive.stage === 'done',
              warn: activeReceive.stage === 'rejected',
              accent: activeReceive.stage === 'receiving',
            }">{{ receiveStageLabel(activeReceive.stage) }}</span>
          </div>
          <!-- 文件清单：自适应卡片剩余高度，超出时自身滚动 -->
          <div class="item-list">
            <div v-for="(item, index) in activeReceive.items" :key="`${activeReceive.id}-${index}`" class="item-row">
              <span class="item-status" :class="item.status">
                {{ item.status === 'done' ? '✓' : (item.status === 'receiving' ? '⇩' : '·') }}
              </span>
              <span class="item-name">{{ item.name }}</span>
              <span class="muted tiny">{{ fmtSize(item.size) }}</span>
            </div>
          </div>
          <!-- 整体进度条：待接收/已拒绝不显示 -->
          <div v-if="activeReceive.stage === 'receiving' || activeReceive.stage === 'done'" class="progress-bar receive-progress">
            <div class="progress-fill" :class="{ full: receiveTaskPercent >= 100 }" :style="{ width: receiveTaskPercent + '%' }"></div>
          </div>
          <div v-if="incomingViewReq" class="request-actions">
            <button class="btn sm" @click="rejectReceive(incomingViewReq)">{{ t('拒绝') }}</button>
          </div>
        </div>
      </section>
    </div>

    <!-- 设置视图 -->
    <div v-show="view === 'settings'" class="view-body">
      <div class="settings-grid">
        <div class="card pad">
          <div class="section-title">{{ t('本机信息') }}</div>
          <div class="field">
            <label>{{ t('设备名称（仅用于同步功能标识设备）') }}</label>
            <div v-if="editingName" class="row">
              <input v-model="nameDraft" class="input" @keyup.enter="saveName" />
              <button class="btn sm primary" @click="saveName">{{ t('保存') }}</button>
            </div>
            <div v-else class="row name-row">
              <strong>{{ status?.device_name || '—' }}</strong>
              <button class="btn sm" @click="editingName = true">{{ t('修改') }}</button>
            </div>
          </div>
          <div class="field" style="margin-top: 14px">
            <label>{{ t('本机指纹（首次连接对方确认时用）') }}</label>
            <div class="fingerprint-box mono">{{ status?.fingerprint_short || '—' }}</div>
          </div>
          <div class="field" style="margin-top: 14px">
            <label>{{ t('接收目录') }}</label>
            <div class="row">
              <input :value="status?.receive_dir || ''" class="input mono" readonly />
              <button v-if="!isMobile" class="btn sm" @click="setReceiveDir">{{ t('更改') }}</button>
            </div>
          </div>
        </div>

        <div class="card pad">
          <div class="section-title">{{ t('手动添加') }}</div>
          <p class="muted tiny">{{ t('发现不到设备时，输入对方 IP 添加；传输时核对指纹即可建立信任') }}</p>
          <div class="field" style="margin-top: 10px">
            <label>{{ t('对方 IP') }}</label>
            <input v-model="pairIp" class="input mono" placeholder="192.168.1.10" />
          </div>
          <button class="btn primary full" style="margin-top: 12px" :disabled="busy" @click="doPair">
            <svg v-if="busy && activeAction === 'pair'" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
              <path d="M20 12a8 8 0 1 1-2.3-5.7M20 4v5h-5" />
            </svg>
            {{ t('添加设备') }}
          </button>
        </div>
      </div>
    </div>

    <!-- 发送状态弹窗：等待接受 / 已接受文件列表 -->
    <div v-if="sendModal" class="modal-mask">
      <div class="modal send-modal">
        <h3>{{ sendModal.stage === 'accepted' || sendModal.stage === 'transferring' ? t('发送文件列表') : t('发送状态') }}</h3>
        <div class="modal-info">
          <div class="row-between"><span class="muted">{{ t('发送方') }}</span><strong>{{ status?.device_name || '—' }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('接收方') }}</span><strong>{{ sendModal.targetName }}</strong></div>
          <div class="row-between">
            <span class="muted">{{ t('状态') }}</span>
            <strong :class="{
              ok: sendModal.stage === 'accepted' || sendModal.stage === 'transferring' || sendModal.stage === 'done',
              warn: sendModal.stage === 'waiting',
              danger: sendModal.stage === 'error' || sendModal.stage === 'cancelled',
            }">
              {{ stageLabel(sendModal) }}
            </strong>
          </div>
        </div>
        <div v-if="sendModal.stage === 'accepted' || sendModal.stage === 'transferring'" class="modal-file-list">
          <div v-for="(item, index) in sendModal.items" :key="index" class="modal-file-row">
            <span>{{ item.name }}</span>
            <span class="muted tiny">{{ item.size ? fmtSize(item.size) : t('待发送') }}</span>
          </div>
        </div>
        <div v-if="sendModal.stage === 'transferring'" class="progress-bar">
          <div class="progress-fill" :style="{ width: (sendModal.total ? sendModal.sent / sendModal.total * 100 : 0) + '%' }"></div>
        </div>
        <div class="modal-actions">
          <button
            class="btn danger"
            :disabled="sendCanceling || sendModal.stage === 'done' || sendModal.stage === 'cancelled' || sendModal.stage === 'error'"
            @click="cancelSend"
          >{{ sendCanceling ? t('撤销中') : t('取消发送') }}</button>
          <button class="btn primary" @click="sendModal = null">{{ t('关闭') }}</button>
        </div>
      </div>
    </div>

    <!-- 接收请求摘要弹窗 -->
    <div v-if="requestDialog" class="modal-mask">
      <div class="modal request-modal">
        <h3>{{ t('收到传输请求') }}</h3>
        <div class="modal-info">
          <div class="row-between"><span class="muted">{{ t('发送方') }}</span><strong>{{ requestDialog.from }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('设备类型') }}</span><strong>{{ platformLabel(requestDialog.from_platform) }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('发送文件数量') }}</span><strong>{{ requestDialog.count }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('类型') }}</span><strong>{{ kindLabel(requestDialog.kind) }}</strong></div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="rejectReceive(requestDialog)">{{ t('拒绝') }}</button>
          <button class="btn" @click="openIncoming()">{{ t('查看清单') }}</button>
          <button class="btn primary" @click="approveReceive(requestDialog)">{{ t('接收') }}</button>
        </div>
      </div>
    </div>

    <!-- 指纹确认弹窗（TOFU） -->
    <div v-if="fpDialog" class="modal-mask">
      <div class="modal confirm-modal">
        <h3>{{ t('确认设备身份') }}</h3>
        <p>{{ t('首次连接，请核对对方显示的指纹是否一致，确认后建立信任') }}</p>
        <div class="fp-target">
          <div class="row-between"><span class="muted">{{ t('设备') }}</span><strong>{{ fpDialog.from }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('设备类型') }}</span><strong>{{ platformLabel(fpDialog.from_platform) }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('发送文件数量') }}</span><strong>{{ fpDialog.count }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('指纹') }}</span><strong class="mono">{{ shortFp(fpDialog.fingerprint) }}</strong></div>
          <div class="row-between"><span class="muted">{{ t('类型') }}</span><strong>{{ kindLabel(fpDialog.kind) }} · {{ fmtSize(fpDialog.size) }}</strong></div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="rejectReceive(fpDialog)">{{ t('拒绝') }}</button>
          <button class="btn primary" @click="approveFingerprint">{{ t('信任并接收') }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
/* 高度完全跟随 workspace 可用空间；不设固定下限，避免日志面板展开时撑破容器产生外层滚动条 */
.sync-page { height: 100%; min-height: 0; overflow-y: auto; display: flex; flex-direction: column; }
.page-head { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 8px; flex-shrink: 0; }
.page-head h2 { font-size: 18px; font-weight: 600; }
.page-head p { font-size: 12px; margin-top: 4px; }
.entry-row { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; flex-shrink: 0; margin-bottom: 10px; }
.entry-card { position: relative; height: 50px; display: flex; align-items: center; justify-content: center; gap: 9px; border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1); border-radius: 10px; cursor: pointer; font-size: 13px; }
.entry-card:hover { border-color: var(--accent); }
.entry-card.active { border-color: var(--accent); background: var(--bg-3); color: var(--accent); }
.entry-icon { width: 20px; height: 20px; flex-shrink: 0; }
.entry-badge { position: absolute; top: 6px; right: 10px; min-width: 16px; height: 16px; padding: 0 4px; border-radius: 8px; background: var(--danger); color: #fff; font-size: 10px; line-height: 16px; }

/* 内容区优先吃满剩余高度；日志面板展开导致空间不足时，保留可读的最小高度，
   超出部分由 .sync-page 自身出页面级滚动条（不是把卡片压扁，也不是 workspace 外层滚动） */
.view-body { flex: 1; min-height: 300px; display: flex; overflow: hidden; }
.send-layout { flex: 1; min-width: 0; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 14px; }
.card.pad { padding: 16px; }
.device-panel, .compose-panel, .receive-panel { position: relative; min-width: 0; min-height: 0; display: flex; flex-direction: column; }
.section-title { font-size: 12px; color: var(--text-1); font-weight: 600; margin-bottom: 12px; }
.row-between { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
.field label, .text-block label { display: block; font-size: 11px; color: var(--text-2); margin-bottom: 5px; }
.hint { font-size: 11px; margin-top: 5px; display: block; }
.tiny { font-size: 10px; }
.row { display: flex; gap: 8px; align-items: center; }
.name-row strong { flex: 1; font-size: 14px; }
.full { width: 100%; }
.empty-hint { text-align: center; padding: 22px 8px; font-size: 12px; line-height: 1.6; }

.refresh-wrap { display: flex; align-items: center; gap: 8px; }
/* 冷却期保持普通箭头光标，与命令中心运行按钮一致（全局 .btn:disabled 是红色禁止） */
.btn.refresh-btn:disabled { cursor: default; opacity: .8; }
/* 刷新图标恒定占位：静止/旋转都用同一 SVG，避免切换文本↻导致按钮被撑大 */
.refresh-icon { width: 14px; height: 14px; flex-shrink: 0; }
.device-list { flex: 1; min-height: 0; overflow-y: auto; padding-right: 2px; }
.device-card { padding: 10px 12px; border: 1px solid var(--border); border-radius: 9px; margin-bottom: 8px; cursor: pointer; background: var(--bg-2); }
.device-card:hover { border-color: var(--accent); }
.device-card.selected { border-color: var(--accent); background: var(--bg-3); }
.device-card.fav { border-left: 3px solid var(--accent); }
.device-card.offline { opacity: .6; }
.device-main { position: relative; display: flex; align-items: center; gap: 8px; min-width: 0; padding-right: 62px; }
.device-main strong { font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.device-main .mono { margin-left: auto; white-space: nowrap; }
.device-sub { display: flex; align-items: center; gap: 10px; margin-top: 7px; }
.dot { width: 7px; height: 7px; border-radius: 50%; background: var(--text-2); flex-shrink: 0; }
.dot.on { background: var(--success); }
.tag { font-size: 10px; padding: 2px 7px; border-radius: 9px; background: var(--bg-3); color: var(--text-2); border: none; }
.tag.ok { color: var(--success); }
.tag.warn { color: var(--danger); }
/* 设备操作图标：容器始终可见，靠子按钮各自控制显隐；
   已收藏的实心星标常亮（不受 hover 影响），删除图标仍只 hover 显示。
   注意不能给容器设 opacity:0 —— 父级透明会让子元素的 opacity:1 失效。 */
.device-actions { position: absolute; top: 50%; right: 0; transform: translateY(-50%); display: flex; gap: 4px; }
.icon-btn { width: 26px; height: 26px; display: inline-flex; align-items: center; justify-content: center; padding: 3px; border: none; border-radius: 6px; background: transparent; color: var(--text-2); cursor: pointer; opacity: 0; transition: opacity .15s; }
.device-card:hover .icon-btn { opacity: 1; }
.icon-btn:hover { background: var(--bg-3); color: var(--text-0); }
.icon-btn svg { width: 16px; height: 16px; }
/* 收藏实心星：常态常亮；hover 时即使父条件不满足也强制显示 */
.icon-btn.favorite-btn.active { color: var(--warn); opacity: 1; }
.device-card.fav .icon-btn.favorite-btn { opacity: 1; }
.icon-btn.remove-btn:hover { color: var(--danger); background: rgba(192,92,86,.12); }

.action-grid { display: grid; grid-template-columns: repeat(4, minmax(0, 1fr)); gap: 10px; justify-items: center; }
.action-card { width: 100%; height: 82px; border: 1px solid var(--border); border-radius: 10px; background: var(--bg-2); color: var(--text-1); display: flex; flex-direction: column; align-items: center; justify-content: center; gap: 6px; cursor: pointer; font-size: 12px; }
.action-card:hover:not(:disabled) { border-color: var(--accent); color: var(--accent); }
.action-card:disabled { opacity: .45; cursor: not-allowed; }
.action-icon { font-size: 24px; line-height: 1; }
.text-block { flex: 1; min-height: 0; display: flex; flex-direction: column; margin-top: 14px; }
.textarea { flex: 1; min-height: 78px; resize: none; }

.receive-panel { width: 100%; }
.count-pill { display: inline-block; margin-left: 7px; padding: 1px 7px; border-radius: 9px; background: var(--danger); color: #fff; font-size: 10px; }
/* 最近一次接收任务：撑满卡片高度，文件清单自适应剩余空间并自身滚动 */
.receive-task { flex: 1; min-height: 0; display: flex; flex-direction: column; padding: 12px; border: 1px solid var(--border); border-radius: 9px; background: var(--bg-2); }
.request-head { display: flex; justify-content: space-between; align-items: center; }
.request-from { display: flex; flex-direction: column; gap: 3px; }
.request-from strong { font-size: 13px; }
.request-meta { display: flex; flex-wrap: wrap; align-items: center; gap: 12px; margin-top: 8px; font-size: 12px; }
.item-list { flex: 1; min-height: 0; margin-top: 10px; overflow-y: auto; border: 1px solid var(--border-soft); border-radius: 8px; background: var(--bg-1); }
.item-row { display: flex; align-items: center; gap: 8px; padding: 7px 10px; border-bottom: 1px solid var(--border-soft); font-size: 12px; }
.item-row:last-child { border-bottom: none; }
.item-status { width: 14px; text-align: center; color: var(--text-2); flex-shrink: 0; }
.item-status.done { color: var(--success); }
.item-status.receiving { color: var(--accent); }
.item-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.progress-bar { height: 8px; margin-top: 10px; background: var(--bg-3); border-radius: 5px; overflow: hidden; flex-shrink: 0; }
.progress-fill { height: 100%; background: var(--accent); transition: width .2s; }
/* 接收进度条更细；到达 100% 时改为青色 */
.receive-progress { height: 4px; }
.receive-progress .progress-fill.full { background: #19b8b3; }
.tag.accent { color: var(--accent); }
.receive-task.rejected { opacity: .6; }
.request-actions { display: flex; justify-content: flex-end; flex-wrap: wrap; gap: 8px; margin-top: 12px; flex-shrink: 0; }

.settings-grid { flex: 1; min-width: 0; min-height: 0; display: grid; grid-template-columns: minmax(0, 1fr) minmax(0, 1fr); gap: 14px; overflow-y: auto; }
.fingerprint-box { padding: 10px 12px; border: 1px solid var(--accent); border-radius: 8px; background: var(--bg-2); color: var(--accent); font-size: 15px; letter-spacing: 2px; }
.btn.danger { color: var(--danger); border-color: var(--danger); }

.modal-mask { position: fixed; inset: 0; background: rgba(0,0,0,.55); display: flex; align-items: center; justify-content: center; z-index: 100; }
.modal { background: var(--bg-1); border: 1px solid var(--border); border-radius: 12px; padding: 22px; box-shadow: 0 18px 48px rgba(0,0,0,.35); }
.send-modal, .request-modal { width: 460px; }
.confirm-modal { width: 440px; }
.modal h3 { font-size: 16px; margin-bottom: 14px; }
.modal p { font-size: 13px; margin-bottom: 12px; line-height: 1.5; }
.modal-info { background: var(--bg-2); border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 9px; }
.modal-info > div { font-size: 12px; }
.modal-info .ok { color: var(--success); }
.modal-info .warn { color: var(--accent); }
.modal-info .danger { color: var(--danger); }
.modal-file-list { margin-top: 12px; max-height: 220px; overflow-y: auto; border: 1px solid var(--border); border-radius: 8px; }
.modal-file-row { display: flex; justify-content: space-between; gap: 10px; padding: 8px 10px; border-bottom: 1px solid var(--border-soft); font-size: 12px; }
.modal-file-row:last-child { border-bottom: none; }
.fp-target { background: var(--bg-2); border-radius: 8px; padding: 12px; display: flex; flex-direction: column; gap: 9px; }
.fp-target > div { font-size: 12px; }
.modal-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 18px; }

.spin { width: 14px; height: 14px; animation: btn-spin .7s linear infinite; }
.action-card .spin { width: 24px; height: 24px; }
@keyframes btn-spin { to { transform: rotate(360deg); } }
</style>
