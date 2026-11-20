<script setup lang="ts">
import { ref, watch, inject } from 'vue';
import { api } from '../lib/api';
import type { Project, Command } from '../types';
import PipelineEditor from './PipelineEditor.vue';

const props = defineProps<{ project: Project | null }>();
const emit = defineEmits<{ (e: 'run', id: string): void; (e: 'clear-logs'): void }>();
const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);

const commands = ref<Command[]>([]);
const showCommandModal = ref(false);
const showPipeline = ref(false);
const showAdhoc = ref(false);
const editing = ref<Command | null>(null);
const pipelineRef = ref<InstanceType<typeof PipelineEditor> | null>(null);

// 临时命令：不保存，直接运行（适合快速测试 python server.py 之类）
const adhoc = ref({ cmd: '', shell: 'powershell' as 'powershell' | 'cmd' | 'bash', cwd: '' });

const form = ref({
  name: '',
  cmd: '',
  cwd: '',
  shell: 'powershell',
  type: 'custom',
  timeout_ms: 0,
});

watch(
  () => props.project?.id,
  () => load(),
  { immediate: true }
);

async function load() {
  if (!props.project) {
    commands.value = [];
    return;
  }
  commands.value = await api.listCommands(props.project.id);
}

function openCreate() {
  editing.value = null;
  form.value = { name: '', cmd: '', cwd: '', shell: 'powershell', type: 'custom', timeout_ms: 0 };
  showCommandModal.value = true;
}

function openEdit(c: Command) {
  editing.value = c;
  form.value = {
    name: c.name,
    cmd: c.cmd,
    cwd: c.cwd,
    shell: c.shell,
    type: c.type,
    timeout_ms: c.timeout_ms,
  };
  showCommandModal.value = true;
}

async function saveCommand() {
  if (!props.project || !form.value.name.trim() || !form.value.cmd.trim()) {
    notify('请填写命令名称和内容');
    return;
  }
  try {
    await api.saveCommand({
      ...form.value,
      shell: form.value.shell as 'powershell' | 'cmd' | 'bash',
      type: form.value.type as 'install' | 'build' | 'run' | 'custom',
      id: editing.value?.id,
      project_id: props.project.id,
    });
    await load();
    showCommandModal.value = false;
    notify('命令已保存');
  } catch (e) {
    notify(`保存失败: ${e}`);
  }
}

async function removeCommand(c: Command) {
  if (!(await confirm(`删除命令「${c.name}」？`, { title: '删除命令', danger: true, confirmText: '删除' }))) return;
  await api.deleteCommand(c.id);
  await load();
}

async function run(c: Command) {
  // 先清空日志再启动，避免后端线程早期输出被随后到达的 clear 清掉
  emit('clear-logs');
  try {
    const id = await api.runCommand(c.id);
    emit('run', id);
    notify(`已启动: ${c.name}，可在「全局任务」页终止`);
  } catch (e) {
    notify(`启动失败: ${e}`);
  }
}

async function runAdhoc() {
  const cmd = adhoc.value.cmd.trim();
  if (!cmd) {
    notify('请输入命令');
    return;
  }
  // 工作目录：填了就用（相对路径拼到项目根），没填用项目根
  let cwd = '';
  if (props.project) {
    cwd = adhoc.value.cwd.trim();
    if (cwd && !/^[A-Za-z]:[\\/]/.test(cwd)) {
      cwd = `${props.project.path.replace(/[\\/]+$/, '')}\\${cwd}`;
    } else if (!cwd) {
      cwd = props.project.path;
    }
  }
  emit('clear-logs');
  try {
    const id = await api.runAdhoc(cmd, cwd, adhoc.value.shell);
    showAdhoc.value = false;
    emit('run', id);
    notify('临时命令已启动，可在「全局任务」页终止');
  } catch (e) {
    notify(`启动失败: ${e}`);
  }
}

function onPipelineRun(id: string) {
  emit('run', id);
  notify('流水线已启动，可在「全局任务」页终止');
}

async function openPipeline() {
  showPipeline.value = true;
  setTimeout(() => pipelineRef.value?.loadPipelines(), 0);
}

</script>

<template>
  <div class="command-center">
    <template v-if="project">
      <div class="page-head">
        <div>
          <h2>命令中心</h2>
          <p class="muted">{{ project.name }} · {{ project.path }}</p>
        </div>
        <div class="row" style="gap: 8px">
          <button class="btn" @click="showAdhoc = true">临时命令</button>
          <button class="btn" @click="openPipeline">流水线编排</button>
          <button class="btn primary" @click="openCreate">+ 新建命令</button>
        </div>
      </div>

      <div class="cmd-grid">
        <div v-for="c in commands" :key="c.id" class="card cmd-card">
          <div class="cmd-title">
            <span class="tag" :class="c.type">{{ c.type === 'install' ? '安装' : c.type === 'build' ? '构建' : c.type === 'run' ? '运行' : '自定义' }}</span>
            <strong :title="c.name">{{ c.name }}</strong>
          </div>
          <div class="cmd-line mono" :title="c.cmd">{{ c.cmd }}</div>
          <div class="cmd-meta muted">
            <span>Shell: {{ c.shell }}</span>
            <span v-if="c.cwd" :title="c.cwd">目录: {{ c.cwd }}</span>
            <span v-if="c.timeout_ms">超时: {{ c.timeout_ms / 1000 }}s</span>
          </div>
          <div class="cmd-actions">
            <button class="act act-edit" title="编辑" @click="openEdit(c)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M12 20h9" /><path d="M16.5 3.5a2.12 2.12 0 0 1 3 3L7 19l-4 1 1-4Z" />
              </svg>
            </button>
            <button class="act act-del" title="删除" @click="removeCommand(c)">
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
                <path d="M3 6h18" /><path d="M8 6V4h8v2" /><path d="M19 6l-1 14H6L5 6" /><path d="M10 11v6M14 11v6" />
              </svg>
            </button>
            <button class="act act-run" title="运行" @click="run(c)">
              <svg viewBox="0 0 24 24" fill="currentColor"><path d="M8 5v14l11-7z" /></svg>
            </button>
          </div>
        </div>
        <button class="card add-card" @click="openCreate">+ 添加命令</button>
      </div>
    </template>

    <div v-else class="empty">
      <div class="big">▸</div>
      <div>请先在左侧选择或新建一个开发项目</div>
    </div>

    <!-- 临时命令弹窗（替代原先常驻的顶部面板） -->
    <div v-if="showAdhoc" class="modal-mask" @click.self="showAdhoc = false">
      <div class="modal" role="dialog" aria-modal="true">
        <h3>临时命令</h3>
        <p class="muted adhoc-tip">直接输入命令运行，不保存。例如：python server.py</p>
        <div class="field">
          <label>命令内容</label>
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
              <option value="powershell">PowerShell</option>
              <option value="cmd">CMD</option>
              <option value="bash">Git Bash</option>
            </select>
          </div>
          <div class="field">
            <label>工作目录（相对项目路径，留空用项目根）</label>
            <input
              v-model="adhoc.cwd"
              class="input mono"
              placeholder="留空或 frontend"
              title="相对项目路径的子目录，留空用项目根"
            />
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="showAdhoc = false">取消</button>
          <button class="btn primary" @click="runAdhoc">运行</button>
        </div>
      </div>
    </div>

    <div v-if="showCommandModal" class="modal-mask" @click.self="showCommandModal = false">
      <div class="modal">
        <h3>{{ editing ? '编辑命令' : '新建命令' }}</h3>
        <div class="field">
          <label>命令名称</label>
          <input v-model="form.name" class="input" placeholder="例如：安装依赖" />
        </div>
        <div class="field" style="margin-top: 14px">
          <label>命令内容</label>
          <textarea v-model="form.cmd" class="textarea" rows="3" placeholder="npm install"></textarea>
        </div>
        <div class="row" style="margin-top: 14px">
          <div class="field">
            <label>类型</label>
            <select v-model="form.type" class="select">
              <option value="install">安装</option>
              <option value="build">构建</option>
              <option value="run">运行</option>
              <option value="custom">自定义</option>
            </select>
          </div>
          <div class="field">
            <label>Shell</label>
            <select v-model="form.shell" class="select">
              <option value="powershell">PowerShell</option>
              <option value="cmd">CMD</option>
              <option value="bash">Git Bash</option>
            </select>
          </div>
        </div>
        <div class="row" style="margin-top: 14px">
          <div class="field">
            <label>工作目录（相对项目路径，留空用项目根）</label>
            <input v-model="form.cwd" class="input mono" placeholder="留空或 frontend" />
          </div>
          <div class="field" style="max-width: 150px">
            <label>超时（毫秒，0=不限）</label>
            <input v-model.number="form.timeout_ms" type="number" class="input" />
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="showCommandModal = false">取消</button>
          <button class="btn primary" @click="saveCommand">保存</button>
        </div>
      </div>
    </div>

    <PipelineEditor
      v-if="showPipeline && project"
      ref="pipelineRef"
      :project-id="project.id"
      :commands="commands"
      @close="showPipeline = false"
      @run="onPipelineRun"
      @changed="load"
    />
  </div>
</template>

<style scoped>
.page-head { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 18px; }
.adhoc-tip { font-size: 11px; margin: -4px 0 14px; }
.page-head h2 { font-size: 18px; font-weight: 600; }
.page-head p { font-size: 12px; margin-top: 4px; }
.cmd-grid { display: grid; grid-template-columns: repeat(auto-fill, 200px); gap: 12px; }
.cmd-card { width: 200px; padding: 12px; display: flex; flex-direction: column; gap: 9px; }
.cmd-title { display: flex; align-items: center; gap: 6px; min-width: 0; }
.cmd-title strong { font-size: 12px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.cmd-line { font-size: 11px; color: var(--text-1); background: var(--bg-0); padding: 7px 8px; border-radius: 6px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.cmd-meta { display: flex; gap: 10px; font-size: 10px; flex-wrap: wrap; min-height: 14px; }
.cmd-meta span { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.cmd-actions { display: flex; gap: 6px; justify-content: flex-end; margin-top: auto; }
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
.add-card { width: 200px; min-height: 120px; border-style: dashed; background: transparent; color: var(--text-2); cursor: pointer; font-size: 14px; }
.add-card:hover { color: var(--accent); border-color: var(--accent); }
</style>
