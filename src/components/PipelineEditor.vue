<script setup lang="ts">
import { ref, inject } from 'vue';
import { api } from '../lib/api';
import type { Pipeline, PipelineStep, Command } from '../types';

const props = defineProps<{
  projectId: number;
  commands: Command[];
  pipeline: Pipeline | null; // null = 新建，传入具体编排 = 编辑
}>();
const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'run', id: string): void;
  (e: 'saved'): void;
}>();
const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);

const activeId = ref<number | null>(props.pipeline?.id ?? null);
const form = ref({ name: props.pipeline?.name ?? '', description: props.pipeline?.description ?? '' });
const steps = ref<PipelineStep[]>([]);
const loading = ref(false);

// 编辑模式加载已有步骤；新建模式从空列表开始
if (activeId.value != null) {
  api.listPipelineSteps(activeId.value).then((list) => (steps.value = list));
}

function addCommandStep() {
  if (!props.commands.length) {
    notify('请先添加命令');
    return;
  }
  steps.value.push({
    id: Date.now(),
    command_id: props.commands[0].id,
    command_name: props.commands[0].name,
    step_type: 'cmd',
    delay_ms: 0,
    on_error: 'stop',
    sort_order: steps.value.length,
  });
}

function addDelayStep() {
  steps.value.push({
    id: Date.now(),
    command_id: null,
    command_name: '等待',
    step_type: 'delay',
    delay_ms: 1000,
    on_error: 'stop',
    sort_order: steps.value.length,
  });
}

function removeStep(i: number) {
  steps.value.splice(i, 1);
}

function move(i: number, dir: -1 | 1) {
  const j = i + dir;
  if (j < 0 || j >= steps.value.length) return;
  const tmp = steps.value[i];
  steps.value[i] = steps.value[j];
  steps.value[j] = tmp;
}

async function save(): Promise<number | null> {
  if (!form.value.name.trim()) {
    notify('请填写流水线名称');
    return null;
  }
  loading.value = true;
  try {
    // 名称与描述随步骤一起落库：新建拿 id，编辑走 UPDATE（后端 save_pipeline 已支持改名）
    const id = await api.savePipeline(
      activeId.value,
      props.projectId,
      form.value.name.trim(),
      form.value.description.trim()
    );
    await api.savePipelineSteps(
      id,
      steps.value.map((s) => ({
        command_id: s.command_id,
        step_type: s.step_type,
        delay_ms: s.delay_ms,
        on_error: s.on_error,
      }))
    );
    activeId.value = id; // 新建保存后转为编辑态
    emit('saved');
    notify('流水线已保存');
    return id;
  } catch (e) {
    notify(`保存失败: ${e}`);
    return null;
  } finally {
    loading.value = false;
  }
}

async function remove() {
  if (activeId.value == null) return;
  if (!(await confirm(`删除流水线「${form.value.name}」？`, { title: '删除流水线', danger: true, confirmText: '删除' }))) return;
  await api.deletePipeline(activeId.value);
  emit('saved');
  emit('close');
  notify('流水线已删除');
}

async function run() {
  const id = await save();
  if (id == null) return;
  const runId = await api.runPipeline(id);
  emit('run', runId);
  notify('流水线已启动');
}
</script>

<template>
  <div class="modal-mask" @click.self="emit('close')">
    <div class="modal editor" role="dialog" aria-modal="true">
      <h3>{{ activeId == null ? '新建流水线' : '编辑流水线' }}</h3>

      <div class="pipe-form">
        <div class="field">
          <label>流水线名称</label>
          <input v-model="form.name" class="input" placeholder="例如：重启服务" />
        </div>
        <div class="field">
          <label>描述（可选）</label>
          <input v-model="form.description" class="input" placeholder="简单说明这条流水线的用途" />
        </div>
      </div>

      <div class="steps-area">
        <div class="steps-toolbar">
          <span class="muted">步骤（按顺序执行）</span>
          <div class="row" style="gap: 6px">
            <button class="btn sm" @click="addCommandStep">+ 命令</button>
            <button class="btn sm" @click="addDelayStep">+ 等待</button>
          </div>
        </div>
        <div class="steps-list">
          <div v-for="(s, i) in steps" :key="s.id" class="step-row">
            <span class="step-no">{{ i + 1 }}</span>
            <template v-if="s.step_type === 'delay'">
              <span class="tag">等待</span>
              <input v-model.number="s.delay_ms" type="number" class="input sm" style="width: 110px" />
              <span class="muted">毫秒</span>
            </template>
            <template v-else>
              <select v-model.number="s.command_id" class="select sm" style="flex: 1">
                <option v-for="c in commands" :key="c.id" :value="c.id" :title="c.cmd">{{ c.name }}</option>
              </select>
              <select v-model="s.on_error" class="select sm" style="width: 96px">
                <option value="stop">失败即停</option>
                <option value="continue">继续</option>
              </select>
            </template>
            <div class="step-actions">
              <button class="icon-sm" title="上移" @click="move(i, -1)">↑</button>
              <button class="icon-sm" title="下移" @click="move(i, 1)">↓</button>
              <button class="icon-sm danger" title="删除" @click="removeStep(i)">×</button>
            </div>
          </div>
          <div v-if="!steps.length" class="empty-small muted">还没有步骤，点击上方按钮添加</div>
        </div>
      </div>

      <div class="modal-actions">
        <button v-if="activeId != null" class="btn danger" @click="remove">删除流水线</button>
        <button class="btn" @click="emit('close')">关闭</button>
        <button class="btn" :disabled="loading" @click="save">保存</button>
        <button class="btn primary" :disabled="loading || !steps.length" @click="run">运行</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.editor {
  width: 720px;
  max-height: 80vh;
  overflow: hidden; /* 外框不滚动，只让步骤列表滚 */
  display: flex;
  flex-direction: column;
}
.editor h3 { flex-shrink: 0; margin-bottom: 14px; }
.pipe-form { flex-shrink: 0; display: flex; gap: 12px; padding-bottom: 14px; border-bottom: 1px solid var(--border); }
.pipe-form .field { flex: 1; }
.steps-area { flex: 1; min-height: 120px; display: flex; flex-direction: column; margin-top: 14px; }
.steps-toolbar { display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px; font-size: 12px; flex-shrink: 0; }
.steps-list { flex: 1; min-height: 0; overflow: auto; display: flex; flex-direction: column; gap: 7px; padding-right: 2px; }
.step-row { display: flex; align-items: center; gap: 9px; padding: 8px 10px; background: var(--bg-2); border: 1px solid var(--border-soft); border-radius: 7px; flex-shrink: 0; }
.step-no { width: 20px; height: 20px; flex-shrink: 0; border-radius: 5px; background: var(--accent-soft); color: var(--accent); display: flex; align-items: center; justify-content: center; font-size: 11px; }
.step-actions { display: flex; gap: 2px; }
.icon-sm { width: 22px; height: 22px; border: none; background: transparent; color: var(--text-2); cursor: pointer; border-radius: 4px; font-size: 13px; }
.icon-sm:hover { background: var(--bg-3); color: var(--text-0); }
.icon-sm.danger:hover { color: var(--danger); }
.empty-small { text-align: center; font-size: 12px; padding: 18px 0; }
.modal-actions { flex-shrink: 0; }
</style>
