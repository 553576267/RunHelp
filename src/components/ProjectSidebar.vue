<script setup lang="ts">
import { ref, inject, onMounted } from 'vue';
import { api } from '../lib/api';
import { t } from '../lib/i18n';
import type { Project } from '../types';

defineProps<{ projects: Project[]; selected: Project | null }>();
const emit = defineEmits<{
  (e: 'select', p: Project): void;
  (e: 'created', p: Project): void;
  (e: 'deleted', id: number): void;
  (e: 'updated', p: Project): void;
  (e: 'restored'): void;
}>();

const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);
const showModal = ref(false);
const editing = ref<Project | null>(null);
// 类型放在最前：先选类型，图片压缩项目无需路径
const form = ref({ name: '', type: 'dev', path: '', description: '' });

const showRecycle = ref(false);
const deleted = ref<Project[]>([]);

onMounted(() => {
  api.listDeletedProjects().then((list) => (deleted.value = list)).catch(() => {});
});

function openCreate() {
  editing.value = null;
  form.value = { name: '', type: 'dev', path: '', description: '' };
  showModal.value = true;
}

function openEdit(p: Project) {
  editing.value = p;
  form.value = { name: p.name, type: p.type, path: p.path, description: p.description };
  showModal.value = true;
}

async function pickFolder() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const dir = await open({ directory: true, multiple: false, title: t('选择项目目录') });
    if (dir && typeof dir === 'string') form.value.path = dir;
  } catch {
    notify(t('目录选择需要 dialog 插件，请直接在输入框粘贴路径'));
  }
}

async function save() {
  if (!form.value.name.trim()) {
    notify(t('请填写项目名称'));
    return;
  }
  // 图片压缩项目允许不填路径；开发项目必须有路径
  if (form.value.type === 'dev' && !form.value.path.trim()) {
    notify(t('开发项目需要填写路径'));
    return;
  }
  try {
    const path = form.value.type === 'image' ? '' : form.value.path.trim();
    if (editing.value) {
      await api.updateProject(editing.value.id, form.value.name, path, form.value.type, form.value.description);
      emit('updated', {
        ...editing.value,
        name: form.value.name,
        path,
        type: form.value.type as 'dev' | 'image',
        description: form.value.description,
      });
      notify(t('项目已更新'));
    } else {
      const id = await api.createProject(form.value.name, path, form.value.type, form.value.description);
      emit('created', {
        id,
        name: form.value.name,
        path,
        type: form.value.type as 'dev' | 'image',
        description: form.value.description,
        created_at: '',
        updated_at: '',
      });
      notify(t('项目已创建'));
    }
    showModal.value = false;
  } catch (e) {
    notify(t('保存失败: {error}', { error: String(e) }));
  }
}

async function remove(p: Project) {
  if (!(await confirm(t('确定把项目「{name}」移到回收站？可在左下角回收站还原。', { name: p.name }), { title: t('删除项目'), danger: true, confirmText: t('移到回收站') }))) return;
  try {
    await api.deleteProject(p.id);
    emit('deleted', p.id);
    showModal.value = false;
    notify(t('已移到回收站'));
  } catch (e) {
    notify(t('删除失败: {error}', { error: String(e) }));
  }
}

async function loadDeleted() {
  deleted.value = await api.listDeletedProjects();
}

async function openRecycle() {
  await loadDeleted();
  showRecycle.value = true;
}

async function restore(p: Project) {
  await api.restoreProject(p.id);
  await loadDeleted();
  emit('restored');
  notify(t('已还原「{name}」', { name: p.name }));
}

async function permanentDelete(p: Project) {
  if (!(await confirm(t('彻底删除「{name}」？此操作不可恢复，相关命令和配置将一并删除。', { name: p.name }), { title: t('彻底删除'), danger: true, confirmText: t('彻底删除') }))) return;
  await api.permanentDeleteProject(p.id);
  await loadDeleted();
  notify(t('已彻底删除'));
}

async function emptyRecycle() {
  if (!deleted.value.length) return;
  if (!(await confirm(t('清空回收站？将彻底删除 {count} 个项目，不可恢复。', { count: deleted.value.length }), { title: t('清空回收站'), danger: true, confirmText: t('全部删除') }))) return;
  const n = await api.emptyRecycleBin();
  await loadDeleted();
  notify(t('已彻底删除 {count} 个项目', { count: n }));
}
</script>

<template>
  <div class="sidebar-body">
    <div class="list">
      <div class="list-head">
        <span>{{ t('项目') }}</span>
        <button class="icon-btn" :title="t('新建项目')" @click="openCreate">+</button>
      </div>
      <div
        v-for="p in projects"
        :key="p.id"
        class="project-item"
        :class="{ active: selected?.id === p.id }"
        @click="emit('select', p)"
      >
        <span class="item-icon" :class="p.type">{{ p.type === 'image' ? '▣' : '▸' }}</span>
        <div class="item-info">
          <div class="item-name">{{ p.name }}</div>
          <div class="item-path mono" :title="p.path">{{ p.path || t('图片压缩项目') }}</div>
        </div>
        <button class="edit-btn" :title="t('编辑')" @click.stop="openEdit(p)">⋯</button>
      </div>
      <button class="add-btn" @click="openCreate">+ {{ t('新建项目') }}</button>
    </div>

    <div class="sidebar-foot">
      <button class="recycle-btn" @click="openRecycle">
        <span>♲</span> {{ t('回收站') }}
        <span v-if="deleted.length" class="badge">{{ deleted.length }}</span>
      </button>
    </div>

    <div v-if="showModal" class="modal-mask" @click.self="showModal = false">
      <div class="modal">
        <h3>{{ editing ? t('编辑项目') : t('新建项目') }}</h3>
        <div class="field">
          <label>{{ t('项目类型（先选类型）') }}</label>
          <select v-model="form.type" class="select">
            <option value="dev">{{ t('开发项目（命令中心）') }}</option>
            <option value="image">{{ t('图片压缩项目') }}</option>
          </select>
        </div>
        <div class="field" style="margin-top: 14px">
          <label>{{ t('项目名称') }}</label>
          <input v-model="form.name" class="input" :placeholder="t('例如：我的前端项目')" />
        </div>
        <div v-if="form.type === 'dev'" class="field" style="margin-top: 14px">
          <label>{{ t('项目路径') }}</label>
          <div class="row">
            <input v-model="form.path" class="input mono" placeholder="D:\code\my-project" />
            <button class="btn" @click="pickFolder">{{ t('浏览') }}</button>
          </div>
          <span class="muted hint">{{ t('开发项目必填，命令以此为工作目录') }}</span>
        </div>
        <div v-else class="field" style="margin-top: 14px">
          <span class="muted hint">{{ t('图片压缩项目无需路径，压缩时直接选择图片或目录即可') }}</span>
        </div>
        <div class="field" style="margin-top: 14px">
          <label>{{ t('备注') }}</label>
          <input v-model="form.description" class="input" :placeholder="t('可选')" />
        </div>
        <div class="modal-actions">
          <button v-if="editing" class="btn danger" @click="remove(editing)">{{ t('删除') }}</button>
          <button class="btn" @click="showModal = false">{{ t('取消') }}</button>
          <button class="btn primary" @click="save">{{ t('保存') }}</button>
        </div>
      </div>
    </div>

    <div v-if="showRecycle" class="modal-mask" @click.self="showRecycle = false">
      <div class="modal recycle-modal">
        <div class="section-head">
          <h3>{{ t('回收站') }}</h3>
          <button v-if="deleted.length" class="btn sm danger" @click="emptyRecycle">{{ t('清空回收站') }}</button>
        </div>
        <p class="muted hint">{{ t('删除的项目暂存在这里，可还原；彻底删除后不可恢复。') }}</p>
        <div v-if="!deleted.length" class="recycle-empty muted">{{ t('回收站为空') }}</div>
        <div v-for="p in deleted" :key="p.id" class="recycle-row">
          <div class="recycle-info">
            <strong>{{ p.name }}</strong>
            <span class="muted">{{ p.type === 'image' ? t('图片压缩') : t('开发项目') }} · {{ p.path || t('无路径') }}</span>
          </div>
          <div class="row" style="gap: 6px">
            <button class="btn sm" @click="restore(p)">{{ t('还原') }}</button>
            <button class="btn sm danger" @click="permanentDelete(p)">{{ t('彻底删除') }}</button>
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn primary" @click="showRecycle = false">{{ t('关闭') }}</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.sidebar-body { flex: 1; min-height: 0; display: flex; flex-direction: column; }
.list { flex: 1; overflow: auto; padding: 10px 10px 16px; }
.list-head {
  display: flex; align-items: center; justify-content: space-between;
  padding: 4px 8px 10px; color: var(--text-2); font-size: 11px;
  text-transform: uppercase; letter-spacing: .06em;
}
.icon-btn {
  width: 22px; height: 22px; border: 1px solid var(--border); background: var(--bg-2);
  color: var(--text-1); border-radius: 5px; cursor: pointer; font-size: 15px; line-height: 1;
}
.icon-btn:hover { color: var(--accent); border-color: var(--accent); }
.project-item {
  display: flex; align-items: center; gap: 9px; padding: 9px 8px;
  border-radius: 7px; cursor: pointer; margin-bottom: 2px; position: relative;
}
.project-item:hover { background: var(--bg-2); }
.project-item.active { background: var(--accent-soft); }
.item-icon {
  width: 22px; height: 22px; flex-shrink: 0; border-radius: 6px;
  background: var(--bg-3); display: flex; align-items: center; justify-content: center;
  font-size: 12px; color: var(--accent);
}
.item-icon.image { color: var(--warn); }
.item-info { min-width: 0; flex: 1; }
.item-name { font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.item-path { font-size: 10px; color: var(--text-2); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.edit-btn {
  opacity: 0; border: none; background: transparent; color: var(--text-2);
  cursor: pointer; font-size: 16px; padding: 0 3px;
}
.project-item:hover .edit-btn { opacity: 1; }
.edit-btn:hover { color: var(--text-0); }
.add-btn {
  width: 100%; margin-top: 8px; height: 32px; border: 1px dashed var(--border);
  background: transparent; color: var(--text-2); border-radius: 7px; cursor: pointer;
}
.add-btn:hover { color: var(--accent); border-color: var(--accent); }
.hint { font-size: 11px; margin-top: 5px; display: block; }
.sidebar-foot { flex-shrink: 0; padding: 8px 10px 12px; border-top: 1px solid var(--border-soft); }
.recycle-btn {
  width: 100%; height: 32px; display: flex; align-items: center; justify-content: center; gap: 7px;
  border: 1px solid var(--border); background: var(--bg-2); color: var(--text-1);
  border-radius: 7px; cursor: pointer; font-size: 12px;
}
.recycle-btn:hover { color: var(--accent); border-color: var(--accent); }
.badge { background: var(--accent); color: #fff; border-radius: 9px; padding: 1px 6px; font-size: 10px; }
.recycle-modal { width: 560px; }
.section-head { display: flex; justify-content: space-between; align-items: center; }
.section-head h3 { font-size: 15px; }
.recycle-empty { text-align: center; padding: 30px 0; font-size: 12px; }
.recycle-row {
  display: flex; justify-content: space-between; align-items: center; gap: 10px;
  padding: 10px 0; border-bottom: 1px solid var(--border-soft); font-size: 12px;
}
.recycle-info { display: flex; flex-direction: column; gap: 3px; min-width: 0; }
.recycle-info strong { font-size: 13px; }
</style>
