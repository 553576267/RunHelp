<script setup lang="ts">
import { ref, watch, onMounted, onUnmounted, inject } from 'vue';
import { api } from '../lib/api';
import { t } from '../lib/i18n';
import { defaultCompressionParams, type Project, type CompressionProfile, type CompressionParams, type Script, type LogEvent } from '../types';
import { listen } from '@tauri-apps/api/event';

const props = defineProps<{ project: Project | null }>();
const emit = defineEmits<{ (e: 'run', id: string): void }>();
const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);

const profiles = ref<CompressionProfile[]>([]);
const scripts = ref<Script[]>([]);
const selectedScriptId = ref<number | null>(null);
const activeProfile = ref<CompressionProfile | null>(null);
const params = ref<CompressionParams>({ ...defaultCompressionParams });
const profileName = ref('');
const inputPaths = ref<string[]>([]);

// 常用目录：存在本地，点 ☆ 收藏后下次可直接点 chip 加入
const FAV_KEY = 'runhelp.compress.favorite_dirs';
const favoriteDirs = ref<string[]>(loadFavoriteDirs());
function loadFavoriteDirs(): string[] {
  try {
    const raw = localStorage.getItem(FAV_KEY);
    const arr = raw ? JSON.parse(raw) : [];
    return Array.isArray(arr) ? arr.filter((x) => typeof x === 'string') : [];
  } catch {
    return [];
  }
}
function persistFavoriteDirs() {
  try {
    localStorage.setItem(FAV_KEY, JSON.stringify(favoriteDirs.value));
  } catch {
    /* 忽略存储不可用 */
  }
}
function isFavorite(p: string) {
  return favoriteDirs.value.includes(p);
}
function toggleFavorite(p: string) {
  if (isFavorite(p)) {
    favoriteDirs.value = favoriteDirs.value.filter((x) => x !== p);
    notify(t('已取消常用目录'));
  } else {
    if (favoriteDirs.value.length >= 12) {
      notify(t('常用目录最多 12 个，请先移除一个'));
      return;
    }
    favoriteDirs.value.push(p);
    notify(t('已加入常用目录'));
  }
  persistFavoriteDirs();
}
function removeFavorite(p: string) {
  favoriteDirs.value = favoriteDirs.value.filter((x) => x !== p);
  persistFavoriteDirs();
}
function addInputPath(p: string) {
  if (inputPaths.value.includes(p)) {
    notify(t('该路径已在列表中'));
    return;
  }
  inputPaths.value.push(p);
}
function dirName(p: string) {
  const parts = p.split(/[\\/]/).filter(Boolean);
  return parts.length ? parts[parts.length - 1] : p;
}

const running = ref(false);
const currentRun = ref('');
const progress = ref({ current: 0, total: 0, percent: 0 });
const results = ref<{ ok: boolean; input: string; output: string; before: number; after: number }[]>([]);
const summary = ref<{ total: number; ok: number; failed: number; before: number; after: number } | null>(null);

watch(
  () => props.project?.id,
  () => load(),
  { immediate: true }
);

let unlisten: (() => void) | null = null;

onMounted(async () => {
  unlisten = await listen<LogEvent>('run://log', (e) => {
    if (e.payload.run_id !== currentRun.value) return;
    try {
      const data = JSON.parse(e.payload.line);
      if (data.type === 'progress') {
        progress.value = {
          current: data.current,
          total: data.total,
          percent: data.total ? Math.round((data.current / data.total) * 100) : 0,
        };
        running.value = data.current < data.total;
      } else if (data.type === 'result') {
        results.value.push(data);
      } else if (data.type === 'summary') {
        summary.value = data;
        running.value = false;
      } else if (data.type === 'error') {
        notify(data.message);
      }
    } catch {
      // 非 JSON 行，忽略（完整输出在控制台）
    }
  });
});

onUnmounted(() => {
  if (unlisten) unlisten();
});

async function load() {
  // 脚本列表与项目无关，始终加载（供预设选择用）
  scripts.value = await api.listScripts();
  if (!props.project) {
    profiles.value = [];
    return;
  }
  profiles.value = await api.listProfiles(props.project.id);
  if (profiles.value.length) await selectProfile(profiles.value[0]);
  else {
    activeProfile.value = null;
    params.value = { ...defaultCompressionParams };
    inputPaths.value = [];
  }
}

function selectProfile(p: CompressionProfile) {
  activeProfile.value = p;
  selectedScriptId.value = p.script_id;
  try {
    params.value = { ...defaultCompressionParams, ...JSON.parse(p.params_json) };
  } catch {
    params.value = { ...defaultCompressionParams };
  }
}

function onProfileChange(e: Event) {
  const id = Number((e.target as HTMLSelectElement).value);
  if (id === -1) {
    newProfile();
    return;
  }
  const found = profiles.value.find((p) => p.id === id);
  if (found) selectProfile(found);
}

function newProfile() {
  activeProfile.value = null;
  params.value = { ...defaultCompressionParams };
  profileName.value = '';
  selectedScriptId.value = null;
}

async function saveProfile() {
  if (!props.project) return;
  if (!profileName.value.trim() && !activeProfile.value) {
    notify(t('请填写预设名称'));
    return;
  }
  const name = profileName.value.trim() || activeProfile.value!.name;
  const id = await api.saveProfile(
    activeProfile.value?.id ?? null,
    props.project.id,
    name,
    JSON.stringify(params.value),
    selectedScriptId.value
  );
  profileName.value = '';
  await load();
  const found = profiles.value.find((p) => p.id === id);
  if (found) selectProfile(found);
  notify(t('预设已保存'));
}

async function deleteProfile() {
  if (!activeProfile.value || !(await confirm(t('删除预设「{name}」？', { name: activeProfile.value.name }), { title: t('删除预设'), danger: true, confirmText: t('删除') }))) return;
  await api.deleteProfile(activeProfile.value.id);
  newProfile();
  await load();
}

async function pickInputs() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const result = await open({ directory: true, multiple: true, title: t('选择图片文件或目录') });
    if (!result) return;
    const arr = Array.isArray(result) ? result : [result];
    inputPaths.value = [...new Set([...inputPaths.value, ...arr.filter((x) => typeof x === 'string')])] as string[];
  } catch {
    notify(t('目录选择需要 dialog 插件，请在下方手动输入路径'));
  }
}

function removeInput(i: number) {
  inputPaths.value.splice(i, 1);
}

async function start() {
  if (!props.project) return;
  if (!inputPaths.value.length) {
    notify(t('请先选择要压缩的图片或目录'));
    return;
  }
  await saveProfile();
  progress.value = { current: 0, total: 0, percent: 0 };
  results.value = [];
  summary.value = null;
  running.value = true;
  try {
    const id = await api.runCompression(activeProfile.value!.id, inputPaths.value);
    currentRun.value = id;
    emit('run', id);
    notify(t('压缩任务已启动'));
  } catch (e) {
    notify(t('启动失败: {error}', { error: String(e) }));
    running.value = false;
  }
}

async function stop() {
  if (!currentRun.value) return;
  await api.stopCompression(currentRun.value);
  running.value = false;
  notify(t('已发送终止信号'));
}

function fmtSize(b: number) {
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  return `${(b / 1024 / 1024).toFixed(2)} MB`;
}

function savedPercent() {
  if (!summary.value || !summary.value.before) return 0;
  return Math.round((1 - summary.value.after / summary.value.before) * 100);
}
</script>

<template>
  <div class="compress-page">
    <template v-if="project">
      <div class="page-head">
        <div>
          <h2>{{ t('图片压缩') }}</h2>
          <p class="muted">{{ project.name }} · {{ t('配置参数后一键压缩，无需手动运行命令') }}</p>
        </div>
      </div>

      <div class="layout">
        <div class="left card">
          <div class="section-title">{{ t('参数预设') }}</div>
          <div class="profile-row">
            <select class="select" :value="activeProfile?.id ?? -1" @change="onProfileChange">
              <option v-for="p in profiles" :key="p.id" :value="p.id">{{ p.name }}</option>
              <option :value="-1">+ {{ t('新建预设') }}</option>
            </select>
            <button class="btn sm" @click="newProfile">{{ t('新建') }}</button>
          </div>
          <div class="field" style="margin-top: 10px">
            <label>{{ t('预设名称') }}</label>
            <input v-model="profileName" class="input" :placeholder="activeProfile?.name ?? t('例如：网页图片质量80')" />
          </div>
          <div class="field" style="margin-top: 10px">
            <label>{{ t('使用脚本') }}</label>
            <select v-model="selectedScriptId" class="select">
              <option :value="null">{{ t('内置图片压缩脚本（默认）') }}</option>
              <option v-for="s in scripts" :key="s.id" :value="s.id">{{ s.name }}（{{ s.source_type === 'builtin' ? t('数据库') : t('外链') }}）</option>
            </select>
            <span class="muted hint">{{ t('自定义脚本需遵循内置脚本的参数协议（读 --config 的 JSON，输出 JSON 行）。脚本在「全局设置 → 脚本管理」新增。') }}</span>
          </div>

          <div class="section-title" style="margin-top: 20px">{{ t('压缩参数') }}</div>
          <div class="row">
            <div class="field">
              <label>{{ t('质量 (1-95)') }}</label>
              <input v-model.number="params.quality" type="number" min="1" max="95" class="input" />
            </div>
            <div class="field">
              <label>{{ t('输出格式') }}</label>
              <select v-model="params.format" class="select">
                <option value="keep">{{ t('保持原格式') }}</option>
                <option value="jpg">JPG</option>
                <option value="png">PNG</option>
                <option value="webp">WebP</option>
              </select>
            </div>
          </div>
          <div class="row" style="margin-top: 12px">
            <div class="field">
              <label>{{ t('最大宽度 (0=不限)') }}</label>
              <input v-model.number="params.max_width" type="number" class="input" />
            </div>
            <div class="field">
              <label>{{ t('最大高度 (0=不限)') }}</label>
              <input v-model.number="params.max_height" type="number" class="input" />
            </div>
          </div>
          <div class="row" style="margin-top: 12px">
            <div class="field">
              <label>{{ t('命名规则') }}</label>
              <select v-model="params.naming" class="select">
                <option value="keep">{{ t('保持原名') }}</option>
                <option value="prefix">{{ t('加前缀') }}</option>
                <option value="suffix">{{ t('加后缀') }}</option>
              </select>
            </div>
            <div class="field">
              <label>{{ params.naming === 'prefix' ? t('前缀') : t('后缀') }}</label>
              <input v-if="params.naming === 'prefix'" v-model="params.name_prefix" class="input" />
              <input v-else-if="params.naming === 'suffix'" v-model="params.name_suffix" class="input" />
              <input v-else class="input" disabled :placeholder="t('无需设置')" />
            </div>
          </div>
          <div class="field" style="margin-top: 12px">
            <label>{{ t('输出目录（留空则输出到原目录下的 _compressed 文件夹）') }}</label>
            <input v-model="params.output_dir" class="input mono" :placeholder="t('留空使用默认')" />
          </div>
          <div class="checks">
            <label><input v-model="params.strip_metadata" type="checkbox" /> {{ t('去除元数据') }}</label>
            <label><input v-model="params.recursive" type="checkbox" /> {{ t('递归子目录') }}</label>
            <label><input v-model="params.overwrite" type="checkbox" /> {{ t('覆盖同名文件') }}</label>
          </div>
          <div class="row" style="margin-top: 12px">
            <div class="field">
              <label>{{ t('跳过小于 (KB)') }}</label>
              <input v-model.number="params.min_size_kb" type="number" class="input" />
            </div>
          </div>
          <div class="profile-actions">
            <button v-if="activeProfile" class="btn sm danger" @click="deleteProfile">{{ t('删除预设') }}</button>
            <button class="btn sm" @click="saveProfile">{{ t('保存预设') }}</button>
          </div>
        </div>

        <div class="right">
          <div class="card input-card">
            <div class="section-title">{{ t('选择图片 / 目录') }}</div>
            <div class="row">
              <button class="btn" @click="pickInputs">+ {{ t('添加路径') }}</button>
              <span class="muted">{{ t('支持图片文件或包含图片的目录') }}</span>
            </div>
            <div class="fav-row">
              <span class="fav-label">{{ t('常用目录') }}</span>
              <div class="fav-chips">
                <button
                  v-for="d in favoriteDirs"
                  :key="d"
                  class="fav-chip"
                  :title="t('点击加入：{name}', { name: d })"
                  @click="addInputPath(d)"
                >
                  <span class="fav-name">▣ {{ dirName(d) }}</span>
                  <span class="fav-remove" :title="t('从常用目录移除')" @click.stop="removeFavorite(d)">×</span>
                </button>
                <span v-if="!favoriteDirs.length" class="muted fav-empty">
                  {{ t('在下方路径点 ☆ 收藏，下次可直接点这里选择') }}
                </span>
              </div>
            </div>
            <div class="path-list">
              <div v-for="(p, i) in inputPaths" :key="i" class="path-item">
                <span class="mono">{{ p }}</span>
                <button
                  class="star-btn"
                  :class="{ on: isFavorite(p) }"
                  :title="isFavorite(p) ? t('取消常用目录') : t('设为常用目录')"
                  @click="toggleFavorite(p)"
                >{{ isFavorite(p) ? '★' : '☆' }}</button>
                <button class="icon-sm" :title="t('移除路径')" @click="removeInput(i)">×</button>
              </div>
              <div v-if="!inputPaths.length" class="muted empty-path">{{ t('尚未选择任何路径') }}</div>
            </div>
          </div>

          <div class="card run-card">
            <div class="run-head">
              <div class="section-title">{{ t('压缩进度') }}</div>
              <div class="row" style="gap: 8px">
                <button v-if="running" class="btn sm danger" @click="stop">{{ t('终止') }}</button>
                <button class="btn primary" :disabled="running" @click="start">{{ t('开始压缩') }}</button>
              </div>
            </div>
            <div class="progress-bar"><div class="progress-fill" :style="{ width: progress.percent + '%' }"></div></div>
            <div class="progress-text muted">{{ progress.current }} / {{ progress.total }} · {{ progress.percent }}%</div>

            <div v-if="summary" class="summary">
              <div class="metric"><span class="muted">{{ t('总数') }}</span><strong>{{ summary.total }}</strong></div>
              <div class="metric"><span class="muted">{{ t('成功') }}</span><strong style="color: var(--success)">{{ summary.ok }}</strong></div>
              <div class="metric"><span class="muted">{{ t('失败') }}</span><strong style="color: var(--danger)">{{ summary.failed }}</strong></div>
              <div class="metric"><span class="muted">{{ t('节省空间') }}</span><strong style="color: var(--accent)">{{ savedPercent() }}%</strong></div>
            </div>
            <div class="result-list">
              <div v-for="(r, i) in results.slice(-8)" :key="i" class="result-row">
                <span class="mono" :title="r.input">{{ r.input.split(/[\\/]/).pop() }}</span>
                <span class="muted">{{ fmtSize(r.before) }} → {{ fmtSize(r.after) }}</span>
              </div>
            </div>
          </div>
        </div>
      </div>
    </template>
    <div v-else class="empty">
      <div class="big">▣</div>
      <div>{{ t('请先在左侧选择或新建一个图片压缩项目') }}</div>
    </div>
  </div>
</template>

<style scoped>
.page-head { margin-bottom: 18px; }
.page-head h2 { font-size: 18px; font-weight: 600; }
.page-head p { font-size: 12px; margin-top: 4px; }
/* 左右两栏等高拉伸：右侧进度块自动填满，底部与左侧参数预设块对齐 */
.layout { display: grid; grid-template-columns: 380px minmax(0, 1fr); gap: 14px; align-items: stretch; }
.left { padding: 16px; }
.hint { font-size: 11px; margin-top: 5px; display: block; }
.section-title { font-size: 12px; color: var(--text-1); font-weight: 600; margin-bottom: 10px; }
.profile-row { display: flex; gap: 8px; }
.checks { display: flex; flex-wrap: wrap; gap: 14px; margin-top: 14px; font-size: 12px; color: var(--text-1); }
.checks label { display: flex; align-items: center; gap: 6px; cursor: pointer; }
.checks input { accent-color: var(--accent); }
.profile-actions { display: flex; justify-content: flex-end; gap: 8px; margin-top: 18px; padding-top: 14px; border-top: 1px solid var(--border-soft); }
.right { display: flex; flex-direction: column; gap: 14px; min-width: 0; }
.input-card { padding: 16px; }
.fav-row { display: flex; align-items: flex-start; gap: 10px; margin-top: 12px; }
.fav-label { flex-shrink: 0; font-size: 11px; color: var(--text-2); padding-top: 6px; white-space: nowrap; }
.fav-chips { display: flex; flex-wrap: wrap; gap: 6px; flex: 1; min-width: 0; max-height: 54px; overflow: auto; }
.fav-chip {
  display: inline-flex; align-items: center; gap: 4px; max-width: 160px;
  height: 24px; padding: 0 5px 0 9px; border: 1px solid var(--border);
  background: var(--bg-2); color: var(--text-1); border-radius: 12px;
  font-size: 11px; cursor: pointer; transition: border-color .15s, color .15s;
}
.fav-chip:hover { border-color: var(--accent); color: var(--text-0); }
.fav-name { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.fav-remove { color: var(--text-2); font-size: 13px; line-height: 1; padding: 0 2px; }
.fav-remove:hover { color: var(--danger); }
.fav-empty { font-size: 11px; padding-top: 6px; line-height: 1.5; }
.path-list { margin-top: 12px; max-height: 150px; overflow: auto; display: flex; flex-direction: column; gap: 4px; }
.path-item { display: flex; align-items: center; gap: 8px; padding: 7px 10px; background: var(--bg-2); border-radius: 6px; font-size: 12px; }
.path-item > span { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.star-btn { border: none; background: transparent; color: var(--text-2); cursor: pointer; font-size: 14px; line-height: 1; padding: 0 2px; }
.star-btn:hover, .star-btn.on { color: var(--warn); }
.icon-sm { border: none; background: transparent; color: var(--text-2); cursor: pointer; font-size: 15px; }
.icon-sm:hover { color: var(--danger); }
.empty-path { padding: 12px; text-align: center; font-size: 12px; }
.run-card { padding: 16px; flex: 1; min-height: 260px; display: flex; flex-direction: column; }
.run-head { display: flex; justify-content: space-between; align-items: center; }
.run-head .section-title { margin: 0; }
.progress-bar { height: 8px; background: var(--bg-3); border-radius: 5px; overflow: hidden; margin-top: 16px; }
.progress-fill { height: 100%; background: var(--accent); border-radius: 5px; transition: width .25s; }
.progress-text { font-size: 11px; margin-top: 7px; }
.summary { display: grid; grid-template-columns: repeat(4, 1fr); gap: 10px; margin-top: 16px; }
.metric { background: var(--bg-2); border-radius: 7px; padding: 10px; display: flex; flex-direction: column; gap: 4px; }
.metric span { font-size: 11px; }
.metric strong { font-size: 18px; font-weight: 600; }
/* 结果列表吃掉剩余高度：窗口越高可见越多，底边与左侧预设块对齐 */
.result-list { margin-top: 14px; flex: 1; min-height: 120px; overflow: auto; }
.result-row { display: flex; justify-content: space-between; gap: 12px; padding: 7px 2px; border-bottom: 1px solid var(--border-soft); font-size: 12px; }
.result-row span:first-child { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
</style>
