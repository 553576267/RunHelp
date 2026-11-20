<script setup lang="ts">
import { ref, computed, inject } from 'vue';
import { api } from '../lib/api';
import type { Pipeline, PipelineStep, Command } from '../types';

const props = defineProps<{ projectId: number; commands: Command[] }>();
const emit = defineEmits<{
  (e: 'close'): void;
  (e: 'run', id: string): void;
  (e: 'changed'): void;
}>();
const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);

const pipelines = ref<Pipeline[]>([]);
const active = ref<Pipeline | null>(null);
const steps = ref<PipelineStep[]>([]);
const newName = ref('');
const loading = ref(false);

async function loadPipelines() {
  pipelines.value = await api.listPipelines(props.projectId);
  if (!active.value && pipelines.value.length) await select(pipelines.value[0]);
  else if (active.value) {
    const found = pipelines.value.find((p) => p.id === active.value!.id);
    if (found) await select(found);
  }
}

async function select(p: Pipeline) {
  active.value = p;
  steps.value = await api.listPipelineSteps(p.id);
}

async function createPipeline() {
  if (!newName.value.trim()) return;
  const id = await api.savePipeline(null, props.projectId, newName.value.trim(), '');
  newName.value = '';
  await loadPipelines();
  const found = pipelines.value.find((p) => p.id === id);
  if (found) await select(found);
  emit('changed');
  notify('流水线已创建');
}

async function deletePipeline() {
  if (!active.value || !(await confirm(`删除流水线「${active.value.name}」？`, { title: '删除流水线', danger: true, confirmText: '删除' }))) return;
  await api.deletePipeline(active.value.id);
  active.value = null;
  steps.value = [];
  await loadPipelines();
  emit('changed');
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

async function saveSteps() {
  if (!active.value) return;
  loading.value = true;
  try {
    await api.savePipelineSteps(
      active.value.id,
      steps.value.map((s) => ({
        command_id: s.command_id,
        step_type: s.step_type,
        delay_ms: s.delay_ms,
        on_error: s.on_error,
      }))
    );
    notify('流水线已保存');
    emit('changed');
  } catch (e) {
    notify(`保存失败: ${e}`);
  } finally {
    loading.value = false;
  }
}

async function run() {
  if (!active.value) return;
  await saveSteps();
  const id = await api.runPipeline(active.value.id);
  emit('run', id);
  notify('流水线已启动');
}

defineExpose({ loadPipelines });
</script>

<template>
  <div class="modal-mask" @click.self="emit('close')">
    <div class="modal editor">
      <h3>流水线编排</h3>
      <div class="pipe-list">
        <button
          v-for="p in pipelines"
          :key="p.id"
          class="pipe-tab"
          :class="{ active: active?.id === p.id }"
          @click="select(p)"
        >{{ p.name }}</button>
        <div class="new-pipe">
          <input v-model="newName" class="input sm" placeholder="新流水线名称" @keyup.enter="createPipeline" />
          <button class="btn sm" @click="createPipeline">+</button>
        </div>
      </div>

      <div v-if="active" class="steps-area">
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
                <option v-for="c in commands" :key="c.id" :value="c.id">{{ c.name }} — {{ c.cmd }}</option>
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
        <div class="modal-actions">
          <button class="btn danger" @click="deletePipeline">删除流水线</button>
          <button class="btn" @click="emit('close')">关闭</button>
          <button class="btn" :disabled="loading" @click="saveSteps">保存</button>
          <button class="btn primary" :disabled="loading || !steps.length" @click="run">运行</button>
        </div>
      </div>
      <div v-else class="empty-small muted" style="padding: 30px">创建一个流水线，把多个命令串成一键操作</div>
    </div>
  </div>
</template>

<style scoped>
.editor { width: 720px; }
.pipe-list { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; border-bottom: 1px solid var(--border); padding-bottom: 12px; }
.pipe-tab { height: 28px; padding: 0 12px; border: 1px solid var(--border); background: var(--bg-2); border-radius: 6px; cursor: pointer; font-size: 12px; }
.pipe-tab.active { background: var(--accent-soft); border-color: var(--accent); color: var(--accent); }
.new-pipe { display: flex; gap: 5px; margin-left: auto; }
.new-pipe .input { width: 160px; }
.input.sm, .select.sm { height: 26px; padding: 0 8px; font-size: 12px; }
.steps-area { margin-top: 16px; }
.steps-toolbar { display: flex; justify-content: space-between; align-items: center; margin-bottom: 10px; font-size: 12px; }
.steps-list { display: flex; flex-direction: column; gap: 7px; max-height: 340px; overflow: auto; }
.step-row { display: flex; align-items: center; gap: 9px; padding: 8px 10px; background: var(--bg-2); border: 1px solid var(--border-soft); border-radius: 7px; }
.step-no { width: 20px; height: 20px; flex-shrink: 0; border-radius: 5px; background: var(--accent-soft); color: var(--accent); display: flex; align-items: center; justify-content: center; font-size: 11px; }
.step-actions { display: flex; gap: 2px; }
.icon-sm { width: 22px; height: 22px; border: none; background: transparent; color: var(--text-2); cursor: pointer; border-radius: 4px; font-size: 13px; }
.icon-sm:hover { background: var(--bg-3); color: var(--text-0); }
.icon-sm.danger:hover { color: var(--danger); }
.empty-small { text-align: center; font-size: 12px; }
</style>
