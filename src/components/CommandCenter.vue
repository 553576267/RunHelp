<script setup lang="ts">
import { ref, watch, inject, computed, onMounted, onUnmounted } from 'vue';
import { api } from '../lib/api';
import { t } from '../lib/i18n';
import { useActionFeedback } from '../lib/useActionFeedback';
import type { Project, Command, RunningInfo, Pipeline } from '../types';
import PipelineEditor from './PipelineEditor.vue';

const props = defineProps<{ project: Project | null; running: RunningInfo[] }>();
const emit = defineEmits<{ (e: 'run', id: string): void; (e: 'clear-logs'): void }>();
const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);

const commands = ref<Command[]>([]);
const pipelines = ref<Pipeline[]>([]);
// Shell 选项与默认值都由后端按平台提供，前端不再写死 PowerShell/CMD
const shellOptions = ref<{ value: string; label: string }[]>([]);
const defaultShell = ref<string>('');
// 正在启动中的项（统一按钮反馈）：key 用 "c:id"/"p:id" 区分命令与流水线，避免 id 撞号。
// 冷却 600ms、图标原地旋转、期间重复点击忽略，规则由 useActionFeedback 统一定义，全平台一致。
const { activeKeys: startingIds, isActive: isStarting, trigger: markStarting, clear: clearStarting } = useActionFeedback({ cooldownMs: 600 });
// 每个命令正在运行的实例数（来自全局 running 列表，按 command_id 精确匹配，用于左下角角标）
const runningCount = computed(() => {
  const m = new Map<number, number>();
  for (const r of props.running) {
    if (r.command_id != null) m.set(r.command_id, (m.get(r.command_id) ?? 0) + 1);
  }
  return m;
});
// 每条流水线正在运行的实例数（按 pipeline_id 精确匹配，用于流水线卡片左下角角标）
const pipelineRunningCount = computed(() => {
  const m = new Map<number, number>();
  for (const r of props.running) {
    if (r.pipeline_id != null) m.set(r.pipeline_id, (m.get(r.pipeline_id) ?? 0) + 1);
  }
  return m;
});

const showCommandModal = ref(false);
const showPipeline = ref(false);
const showAdhoc = ref(false);
const editing = ref<Command | null>(null);
// 当前正在编辑器中修改的流水线；null 表示新建模式
const editingPipeline = ref<Pipeline | null>(null);

// 临时命令：不保存，直接运行（适合快速测试 python server.py 之类）
const adhoc = ref({ cmd: '', shell: '', cwd: '' });

const form = ref({
  name: '',
  cmd: '',
  cwd: '',
  shell: '',
  type: 'custom',
  timeout_ms: 0,
});

onMounted(async () => {
  try {
    shellOptions.value = await api.getShellOptions();
    const s = await api.getDefaultShell();
    defaultShell.value = s;
    // 同步到两个表单的当前值（仅在用户还没输入时，避免覆盖正在编辑的内容）
    form.value.shell = s;
    adhoc.value.shell = s;
  } catch { /* 后端异常时保持空值，platform 层仍会按平台回落 */ }
});

watch(
  () => props.project?.id,
  () => load(),
  { immediate: true }
);

async function load() {
  if (!props.project) {
    commands.value = [];
    pipelines.value = [];
    return;
  }
  commands.value = await api.listCommands(props.project.id);
  pipelines.value = await api.listPipelines(props.project.id);
}

function openCreate() {
  editing.value = null;
  form.value = { name: '', cmd: '', cwd: '', shell: defaultShell.value, type: 'custom', timeout_ms: 0 };
  showCommandModal.value = true;
}

// 双击卡片打开编辑时，浏览器默认会选中双击处的文字；弹窗挂载后选区会扩展到整个弹窗，
// 导致所有文字呈蓝色选中态。打开编辑前主动清除选区。
function clearTextSelection() {
  window.getSelection()?.removeAllRanges();
}

// 点击页头项目地址，用系统资源管理器打开对应本地目录
async function openProjectFolder() {
  if (!props.project?.path) return;
  try {
    await api.openFolder(props.project.path);
  } catch (e) {
    notify(t('打开目录失败: {error}', { error: String(e) }));
  }
}

function openEdit(c: Command) {
  clearTextSelection();
  editing.value = c;
  form.value = {
    name: c.name,
    cmd: c.cmd,
    cwd: c.cwd,
    // 老数据里的 shell 若不在当前平台选项中，回落到平台默认值
    shell: shellOptions.value.some((o) => o.value === c.shell) ? c.shell : defaultShell.value,
    type: c.type,
    timeout_ms: c.timeout_ms,
  };
  showCommandModal.value = true;
}

async function saveCommand() {
  if (!props.project || !form.value.name.trim() || !form.value.cmd.trim()) {
    notify(t('请填写命令名称和内容'));
    return;
  }
  try {
    await api.saveCommand({
      ...form.value,
      shell: form.value.shell,
      type: form.value.type as 'install' | 'build' | 'run' | 'custom',
      id: editing.value?.id,
      project_id: props.project.id,
    });
    await load();
    showCommandModal.value = false;
    notify(t('命令已保存'));
  } catch (e) {
    notify(t('保存失败: {error}', { error: String(e) }));
  }
}

async function removeCommand(c: Command) {
  if (!(await confirm(t('删除命令「{name}」？', { name: c.name }), { title: t('删除命令'), danger: true, confirmText: t('删除') }))) return;
  await api.deleteCommand(c.id);
  await load();
}

async function run(c: Command) {
  // 防抖：点击后固定冷却 600ms，期间重复点击直接忽略，按钮有明确的启动反馈
  if (!markStarting(`c:${c.id}`)) return;
  // 先清空日志再启动，避免后端线程早期输出被随后到达的 clear 清掉
  emit('clear-logs');
  try {
    const id = await api.runCommand(c.id);
    emit('run', id);
    notify(t('已启动: {name}，可在「全局任务」页终止', { name: c.name }));
  } catch (e) {
    notify(t('启动失败: {error}', { error: String(e) }));
  }
}

onUnmounted(() => {
  clearStarting();
});

async function runAdhoc() {
  const cmd = adhoc.value.cmd.trim();
  if (!cmd) {
    notify(t('请输入命令'));
    return;
  }
  // 工作目录：相对路径拼到项目根、留空用项目根，拼接统一交给后端 PathBuf
  let cwd = adhoc.value.cwd.trim();
  if (props.project) {
    cwd = await api.resolveProjectPath(props.project.path, cwd);
  }
  emit('clear-logs');
  try {
    const id = await api.runAdhoc(cmd, cwd, adhoc.value.shell);
    showAdhoc.value = false;
    emit('run', id);
    notify(t('临时命令已启动，可在「全局任务」页终止'));
  } catch (e) {
    notify(t('启动失败: {error}', { error: String(e) }));
  }
}

function onPipelineRun(id: string) {
  emit('run', id);
  notify(t('流水线已启动，可在「全局任务」页终止'));
}

// 新建：以空编辑器打开
function openNewPipeline() {
  editingPipeline.value = null;
  showPipeline.value = true;
}

// 编辑：加载指定流水线到编辑器
function openEditPipeline(p: Pipeline) {
  clearTextSelection();
  editingPipeline.value = p;
  showPipeline.value = true;
}

// 卡片上直接运行（用已保存的步骤）
async function runPipeline(p: Pipeline) {
  // 与命令卡片一致的防抖：冷却期内忽略重复点击
  if (!markStarting(`p:${p.id}`)) return;
  try {
    const id = await api.runPipeline(p.id);
    onPipelineRun(id);
  } catch (e) {
    notify(t('启动失败: {error}', { error: String(e) }));
  }
}

async function removePipeline(p: Pipeline) {
  if (!(await confirm(t('删除流水线「{name}」？', { name: p.name }), { title: t('删除流水线'), danger: true, confirmText: t('删除') }))) return;
  await api.deletePipeline(p.id);
  await load();
  notify(t('流水线已删除'));
}

</script>

<template>
  <div class="command-center">
    <template v-if="project">
      <div class="page-head">
        <div>
          <h2>{{ t('命令中心') }}</h2>
          <p class="muted">
            {{ project.name }} ·
            <span
              v-if="project.path"
              class="project-path-link"
              :title="t('点击打开项目目录')"
              @click="openProjectFolder"
            >{{ project.path }}</span>
            <span v-else>{{ t('无路径') }}</span>
          </p>
        </div>
        <div class="row" style="gap: 8px">
          <button class="btn" @click="showAdhoc = true">{{ t('临时命令') }}</button>
          <button class="btn" @click="openNewPipeline">{{ t('新建流水线') }}</button>
          <button class="btn btn-create" @click="openCreate">{{ t('新建命令') }}</button>
        </div>
      </div>

      <div class="cmd-grid">
        <div v-for="c in commands" :key="c.id" class="card cmd-card" :title="t('双击编辑命令')" @dblclick.prevent="openEdit(c)">
          <div class="cmd-title">
            <span class="tag" :class="c.type">{{ c.type === 'install' ? t('安装') : c.type === 'build' ? t('构建') : c.type === 'run' ? t('运行') : t('自定义') }}</span>
            <strong :title="c.name">{{ c.name }}</strong>
          </div>
          <div class="cmd-line mono" :title="c.cmd">{{ c.cmd }}</div>
          <div class="cmd-meta muted">
            <span>Shell: {{ c.shell }}</span>
            <span v-if="c.cwd" :title="c.cwd">{{ t('目录') }}: {{ c.cwd }}</span>
            <span v-if="c.timeout_ms">{{ t('超时') }}: {{ c.timeout_ms / 1000 }}s</span>
          </div>
          <div class="cmd-actions">
            <span
              v-if="(runningCount.get(c.id) ?? 0) > 0"
              class="run-badge"
              :title="t('{count} 个实例运行中，可在「全局任务」终止', { count: runningCount.get(c.id) ?? 0 })"
            >
              <span class="run-dot"></span>
              <span class="run-num">{{ runningCount.get(c.id) }}</span>
            </span>
            <button class="act act-edit" :title="t('编辑')" @click="openEdit(c)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
              </svg>
            </button>
            <button class="act act-del" :title="t('删除')" @click="removeCommand(c)" @dblclick.stop>
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 6h18" /><path d="M8 6V4h8v2" /><path d="M19 6l-1 14H6L5 6" /><path d="M10 11v6M14 11v6" />
              </svg>
            </button>
            <button class="act act-run" :disabled="startingIds.has(`c:${c.id}`)" :title="startingIds.has(`c:${c.id}`) ? t('启动中…') : t('运行')" @click="run(c)" @dblclick.stop>
              <svg v-if="startingIds.has(`c:${c.id}`)" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                <path d="M12 3a9 9 0 1 0 9 9" />
              </svg>
              <svg v-else viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z" /></svg>
            </button>
          </div>
        </div>
      </div>

      <div v-if="pipelines.length" class="pipe-section">
        <h3 class="section-title">{{ t('流水线') }} <span class="muted">{{ pipelines.length }}</span></h3>
        <div class="cmd-grid">
          <div v-for="p in pipelines" :key="p.id" class="card cmd-card pipe-card" :title="t('双击编辑流水线')" @dblclick.prevent="openEditPipeline(p)">
            <div class="cmd-title">
              <span class="tag pipe-tag">{{ t('流水线') }}</span>
              <strong :title="p.name">{{ p.name }}</strong>
            </div>
            <div class="pipe-desc" :title="p.description || t('暂无描述')">{{ p.description || t('暂无描述') }}</div>
            <div class="cmd-actions">
              <span
                v-if="(pipelineRunningCount.get(p.id) ?? 0) > 0"
                class="run-badge"
                :title="t('{count} 个实例运行中，可在「全局任务」终止', { count: pipelineRunningCount.get(p.id) ?? 0 })"
              >
                <span class="run-dot"></span>
                <span class="run-num">{{ pipelineRunningCount.get(p.id) }}</span>
              </span>
              <button class="act act-edit" :title="t('编辑')" @click="openEditPipeline(p)">
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
                </svg>
              </button>
              <button class="act act-del" :title="t('删除')" @click="removePipeline(p)" @dblclick.stop>
                <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                  <path d="M3 6h18" /><path d="M8 6V4h8v2" /><path d="M19 6l-1 14H6L5 6" /><path d="M10 11v6M14 11v6" />
                </svg>
              </button>
              <button class="act act-run" :disabled="startingIds.has(`p:${p.id}`)" :title="startingIds.has(`p:${p.id}`) ? t('启动中…') : t('运行')" @click="runPipeline(p)" @dblclick.stop>
                <svg v-if="startingIds.has(`p:${p.id}`)" class="spin" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.4" stroke-linecap="round">
                  <path d="M12 3a9 9 0 1 0 9 9" />
                </svg>
                <svg v-else viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z" /></svg>
              </button>
            </div>
          </div>
        </div>
      </div>
    </template>

    <div v-else class="empty">
      <div class="big">▸</div>
      <div>{{ t('请先在左侧选择或新建一个开发项目') }}</div>
    </div>

    <!-- 临时命令弹窗（替代原先常驻的顶部面板） -->
    <div v-if="showAdhoc" class="modal-mask" @click.self="showAdhoc = false">
      <div class="modal" role="dialog" aria-modal="true">
        <h3>{{ t('临时命令') }}</h3>
        <p class="muted adhoc-tip">{{ t('直接输入命令运行，不保存。例如：python server.py') }}</p>
        <div class="field">
          <label>{{ t('命令内容') }}</label>
          <input
            v-model="adhoc.cmd"
            class="input mono"
            placeholder="python server.py"
            @keyup.enter="runAdhoc"
          />
        </div>
        <div class="row" style="margin-top: 14px">
          <div class="field">
            <label>Shell</label>
            <select v-model="adhoc.shell" class="select">
              <option v-for="opt in shellOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
            </select>
          </div>
          <div class="field">
            <label>{{ t('工作目录（相对项目路径，留空用项目根）') }}</label>
            <input
              v-model="adhoc.cwd"
              class="input mono"
              :placeholder="t('留空或 frontend')"
              :title="t('相对项目路径的子目录，留空用项目根')"
            />
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="showAdhoc = false">{{ t('取消') }}</button>
          <button class="btn primary" @click="runAdhoc">{{ t('运行') }}</button>
        </div>
      </div>
    </div>

    <div v-if="showCommandModal" class="modal-mask" @click.self="showCommandModal = false">
      <div class="modal">
        <h3>{{ editing ? t('编辑命令') : t('新建命令') }}</h3>
        <div class="field">
          <label>{{ t('命令名称') }}</label>
          <input v-model="form.name" class="input" :placeholder="t('例如：安装依赖')" />
        </div>
        <div class="field" style="margin-top: 14px">
          <label>{{ t('命令内容') }}</label>
          <textarea v-model="form.cmd" class="textarea" rows="3" placeholder="npm install"></textarea>
        </div>
        <div class="row" style="margin-top: 14px">
          <div class="field">
            <label>{{ t('类型') }}</label>
            <select v-model="form.type" class="select">
              <option value="install">{{ t('安装') }}</option>
              <option value="build">{{ t('构建') }}</option>
              <option value="run">{{ t('运行') }}</option>
              <option value="custom">{{ t('自定义') }}</option>
            </select>
          </div>
          <div class="field">
            <label>Shell</label>
            <select v-model="form.shell" class="select">
              <option v-for="opt in shellOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
            </select>
          </div>
        </div>
        <div class="row" style="margin-top: 14px">
          <div class="field">
            <label>{{ t('工作目录（相对项目路径，留空用项目根）') }}</label>
            <input v-model="form.cwd" class="input mono" :placeholder="t('留空或 frontend')" />
          </div>
          <div class="field" style="max-width: 150px">
            <label>{{ t('超时（毫秒，0=不限）') }}</label>
            <input v-model.number="form.timeout_ms" type="number" class="input" />
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="showCommandModal = false">{{ t('取消') }}</button>
          <button class="btn primary" @click="saveCommand">{{ t('保存') }}</button>
        </div>
      </div>
    </div>

    <PipelineEditor
      v-if="showPipeline && project"
      :key="editingPipeline?.id ?? 'new'"
      :project-id="project.id"
      :commands="commands"
      :pipeline="editingPipeline"
      @close="showPipeline = false"
      @run="onPipelineRun"
      @saved="load"
    />
  </div>
</template>

<style scoped>
.page-head { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 18px; }
.adhoc-tip { font-size: 11px; margin: -4px 0 14px; }
.page-head h2 { font-size: 18px; font-weight: 600; }
.page-head p { font-size: 12px; margin-top: 4px; }
.project-path-link { color: var(--accent); cursor: pointer; border-bottom: 1px dashed transparent; }
.project-path-link:hover { border-bottom-color: var(--accent); }
.cmd-grid { display: grid; grid-template-columns: repeat(auto-fill, 200px); gap: 12px; }
.cmd-card { width: 200px; padding: 12px; display: flex; flex-direction: column; gap: 9px; user-select: none; }
.cmd-card input, .cmd-card textarea { user-select: text; }
.cmd-title { display: flex; align-items: center; gap: 6px; min-width: 0; }
.cmd-title strong { font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.cmd-line { font-size: 11px; color: var(--text-1); background: var(--bg-0); padding: 7px 8px; border-radius: 6px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.cmd-meta { display: flex; gap: 10px; font-size: 10px; flex-wrap: wrap; min-height: 14px; }
.cmd-meta span { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cmd-actions { display: flex; gap: 6px; justify-content: flex-end; align-items: center; margin-top: auto; }
.act {
  width: 28px; height: 26px; display: inline-flex; align-items: center; justify-content: center;
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 6px; cursor: pointer; transition: opacity .15s, background .15s, border-color .15s, color .15s;
}
.act svg { width: 14px; height: 14px; }
.act:hover { background: var(--bg-3); color: var(--text-0); }
.act-edit, .act-del { opacity: 0; pointer-events: none; }
.cmd-card:hover .act-edit, .cmd-card:hover .act-del { opacity: 1; pointer-events: auto; }
.act-del:hover { color: var(--danger); border-color: var(--danger); }
.act-run { color: #fff; background: var(--accent); border-color: var(--accent); }
.act-run:hover { background: var(--accent-hover); color: #fff; }
.act-run:disabled { opacity: .8; cursor: default; }
/* 防抖期间：图标原地旋转，鼠标指针保持正常，不再像卡死 */
.act-run .spin { animation: btn-spin .7s linear infinite; }
@keyframes btn-spin { to { transform: rotate(360deg); } }
/* 左下角运行中角标：绿点 + 实例数，margin-right:auto 推到最左，与右侧按钮两端对齐 */
.run-badge {
  margin-right: auto; display: inline-flex; align-items: center; gap: 4px;
  height: 20px; padding: 0 7px; border-radius: 10px;
  background: rgba(34, 197, 94, .12); border: 1px solid rgba(34, 197, 94, .3);
  color: #22c55e;
}
.run-dot { width: 6px; height: 6px; border-radius: 50%; background: #22c55e; animation: dot-pulse 1.2s ease-in-out infinite; }
.run-num { font-size: 11px; font-weight: 600; line-height: 1; }
@keyframes dot-pulse { 0%, 100% { opacity: 1; } 50% { opacity: .35; } }
/* 新建命令按钮：默认中性灰融入顶栏，hover 才亮蓝边强调（方案 C） */
.btn-create { transition: background .15s, border-color .15s, box-shadow .15s, transform .15s; }
.btn-create:hover {
  border-color: var(--accent);
  box-shadow: 0 4px 14px rgba(0, 0, 0, .28);
  transform: translateY(-1px);
}
.btn-create:active { transform: translateY(0); box-shadow: none; }
/* 流水线分区 */
.pipe-section { margin-top: 24px; }
.section-title { font-size: 13px; font-weight: 600; margin-bottom: 10px; display: flex; align-items: center; gap: 8px; }
.section-title .muted { font-size: 11px; font-weight: 400; }
.pipe-tag { background: rgba(168, 85, 247, .14); color: #c084fc; border-color: rgba(168, 85, 247, .35); }
.pipe-desc { font-size: 11px; color: var(--text-2); background: var(--bg-0); padding: 7px 8px; border-radius: 6px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
</style>
