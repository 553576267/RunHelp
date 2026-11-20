<script setup lang="ts">
import { ref, onMounted, inject } from 'vue';
import { api } from '../lib/api';
import type { Script, BackupInfo } from '../types';

const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);
const settings = ref<Record<string, string>>({});
const scripts = ref<Script[]>([]);
const dataDir = ref('');
const dbLocation = ref<{ current: string } | null>(null);
const showScriptModal = ref(false);
const editingScript = ref<Script | null>(null);
const scriptForm = ref({ name: '', source_type: 'builtin', content: '', file_path: '' });

// 备份与恢复
const backups = ref<BackupInfo[]>([]);
const backupDirInput = ref('');
const defaultBackupDir = ref('');
const backingUp = ref(false);

onMounted(async () => {
  await load();
});

async function load() {
  settings.value = await api.getSettings();
  scripts.value = await api.listScripts();
  dataDir.value = await api.dataDir();
  dbLocation.value = await api.getDbLocation();
  await loadBackups();
}

async function loadBackups() {
  try {
    defaultBackupDir.value = await api.getDefaultBackupDir();
    backupDirInput.value = settings.value.backup_dir || defaultBackupDir.value;
    backups.value = await api.listBackups();
  } catch (e) {
    notify(`读取备份列表失败: ${e}`);
  }
}

async function saveBackupDir() {
  const dir = backupDirInput.value.trim();
  if (!dir) {
    notify('请填写备份目录');
    return;
  }
  settings.value.backup_dir = dir;
  await api.setSetting('backup_dir', dir);
  await loadBackups();
  notify('备份目录已保存（备份脚本与恢复都会用这个目录）');
}

async function runBackup() {
  if (backingUp.value) return;
  backingUp.value = true;
  try {
    await api.runBackup();
    notify('备份已启动，进度可在「运行日志」查看');
    // 备份是异步任务，稍后刷新列表让用户看到新生成的备份文件
    setTimeout(loadBackups, 2500);
  } catch (e) {
    notify(`备份启动失败: ${e}`);
  } finally {
    backingUp.value = false;
  }
}

async function restoreBackup(b: BackupInfo) {
  const ok = await confirm(
    `从备份恢复？\n\n备份文件：${b.name}\n时间：${b.modified}\n\n当前数据库会先自动另存一份保命，然后用该备份覆盖。恢复后需重启软件生效。`,
    { title: '从备份恢复', danger: true, confirmText: '确认恢复' }
  );
  if (!ok) return;
  try {
    const msg = await api.restoreBackup(b.name);
    notify(msg);
    await loadBackups();
  } catch (e) {
    notify(`恢复失败: ${e}`);
  }
}

async function saveSetting(key: string) {
  await api.setSetting(key, settings.value[key]);
  if (key === 'theme') {
    document.documentElement.setAttribute('data-theme', settings.value[key]);
    // 同步原生标题栏明暗与窗口背景色
    try { await api.setWindowTheme(settings.value[key]); } catch { /* 忽略 */ }
  }
  notify('设置已保存');
}

function openScript(s: Script | null) {
  editingScript.value = s;
  scriptForm.value = s
    ? { name: s.name, source_type: s.source_type, content: s.content, file_path: s.file_path }
    : { name: '', source_type: 'builtin', content: '', file_path: '' };
  showScriptModal.value = true;
}

async function saveScript() {
  if (!scriptForm.value.name.trim()) {
    notify('请填写脚本名称');
    return;
  }
  try {
    await api.saveScript({
      ...scriptForm.value,
      source_type: scriptForm.value.source_type as 'builtin' | 'file',
      id: editingScript.value?.id,
    });
    showScriptModal.value = false;
    await load();
    notify('脚本已保存到数据库');
  } catch (e) {
    notify(`保存失败: ${e}`);
  }
}

async function deleteScript(s: Script) {
  if (!(await confirm(`删除脚本「${s.name}」？`, { title: '删除脚本', danger: true, confirmText: '删除' }))) return;
  await api.deleteScript(s.id);
  await load();
}

async function pickScriptFile() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const f = await open({ filters: [{ name: 'Python', extensions: ['py'] }], title: '选择 Python 脚本' });
    if (f && typeof f === 'string') scriptForm.value.file_path = f;
  } catch {
    notify('请手动输入脚本路径');
  }
}

async function pickPython() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const f = await open({ filters: [{ name: 'Python', extensions: ['exe'] }], title: '选择 python.exe' });
    if (f && typeof f === 'string') {
      settings.value.python_path = f;
      await api.setSetting('python_path', f);
      notify('Python 路径已保存');
    }
  } catch {
    notify('请手动输入 python.exe 路径');
  }
}

</script>

<template>
  <div class="settings-page">
    <div class="page-head">
      <h2>全局设置</h2>
      <p class="muted">全局配置、备份恢复与脚本管理</p>
    </div>

    <div class="card section">
      <h3>运行环境</h3>
      <div class="field">
        <label>Python 解释器路径（全局生效）</label>
        <div class="row">
          <input v-model="settings.python_path" class="input mono" placeholder="D:\tools\IT\Python310\python.exe" />
          <button class="btn" @click="pickPython">浏览</button>
          <button class="btn" @click="saveSetting('python_path')">保存</button>
        </div>
        <span class="muted hint">建议填 python.exe 完整路径。所有命令运行时会把它所在目录置于 PATH 最前，确保 python/pip/uvicorn 用的是装了依赖的那个解释器；留空则用系统 PATH（可能因解释器不对报 ModuleNotFoundError）。图片压缩脚本也用此解释器。</span>
      </div>
      <div class="field" style="margin-top: 16px">
        <label>默认 Shell</label>
        <div class="row">
          <select v-model="settings.default_shell" class="select">
            <option value="powershell">PowerShell</option>
            <option value="cmd">CMD</option>
            <option value="bash">Git Bash</option>
          </select>
          <button class="btn" @click="saveSetting('default_shell')">保存</button>
        </div>
      </div>
      <div class="field" style="margin-top: 16px">
        <label>界面主题</label>
        <div class="row">
          <select v-model="settings.theme" class="select">
            <option value="dark">深色</option>
            <option value="light">浅色</option>
          </select>
          <button class="btn" @click="saveSetting('theme')">应用</button>
        </div>
      </div>
    </div>

    <div class="card section">
      <div class="section-head">
        <h3>备份与恢复</h3>
        <button class="btn primary sm" :disabled="backingUp" @click="runBackup">
          {{ backingUp ? '备份中…' : '立即备份' }}
        </button>
      </div>
      <div class="field" style="margin-bottom: 12px">
        <label>当前数据库文件</label>
        <div class="path-box mono">{{ dbLocation?.current || dataDir }}</div>
      </div>
      <div class="field">
        <label>备份目录</label>
        <div class="row">
          <input v-model="backupDirInput" class="input mono" :placeholder="defaultBackupDir" />
          <button class="btn" @click="saveBackupDir">保存</button>
          <button class="btn" @click="loadBackups">刷新列表</button>
        </div>
        <span class="muted hint">
          备份脚本和恢复功能共用这个目录。
        </span>
      </div>
      <div class="backup-list" style="margin-top: 14px">
        <div v-if="!backups.length" class="muted hint">该目录下暂无备份。点「立即备份」后会显示在这里。</div>
        <div v-for="b in backups" :key="b.name" class="backup-row">
          <div class="backup-info">
            <strong>{{ b.name }}</strong>
            <span v-if="b.local" class="tag">本机</span>
            <span class="muted">{{ b.modified }}</span>
            <span class="muted">{{ (b.size / 1024).toFixed(0) }} KB</span>
          </div>
          <button class="btn sm danger" @click="restoreBackup(b)">恢复</button>
        </div>
      </div>
      <span class="muted hint warning" style="margin-top: 10px">
        ⚠ 恢复会覆盖当前数据库的全部数据（覆盖前会自动另存一份），仅在你确认备份内容正确时执行。两台电脑请各用各的本地库，坚果云只放备份文件，不要把数据库本体放进同步盘。
      </span>
    </div>

    <div class="card section">
      <div class="section-head">
        <div>
          <h3>脚本管理</h3>
          <p class="muted hint">这里新增的脚本，在「图片压缩」页的预设里通过"使用脚本"下拉选择后生效。脚本源码存入数据库（随库备份迁移），也支持外链本地 .py 文件。</p>
        </div>
        <button class="btn sm" @click="openScript(null)">+ 新建脚本</button>
      </div>
      <div v-for="s in scripts" :key="s.id" class="script-row">
        <div class="script-info">
          <strong>{{ s.name }}</strong>
          <span class="tag">{{ s.source_type === 'builtin' ? '存于数据库' : '外链文件' }}</span>
          <span class="muted">{{ s.language }} · v{{ s.version }}</span>
          <span class="muted">{{ s.updated_at }}</span>
        </div>
        <div class="row" style="gap: 6px">
          <button class="btn sm" @click="openScript(s)">编辑</button>
          <button class="btn sm danger" @click="deleteScript(s)">删除</button>
        </div>
      </div>
    </div>

    <div v-if="showScriptModal" class="modal-mask" @click.self="showScriptModal = false">
      <div class="modal script-modal">
        <h3>{{ editingScript ? '编辑脚本' : '新建脚本' }}</h3>
        <div class="field">
          <label>脚本名称</label>
          <input v-model="scriptForm.name" class="input" />
        </div>
        <div class="field" style="margin-top: 14px">
          <label>来源</label>
          <select v-model="scriptForm.source_type" class="select">
            <option value="builtin">存于数据库</option>
            <option value="file">外链本地文件</option>
          </select>
        </div>
        <div v-if="scriptForm.source_type === 'builtin'" class="field" style="margin-top: 14px">
          <label>脚本源码（Python）</label>
          <textarea v-model="scriptForm.content" class="textarea" rows="14"></textarea>
        </div>
        <div v-else class="field" style="margin-top: 14px">
          <label>脚本文件路径</label>
          <div class="row">
            <input v-model="scriptForm.file_path" class="input mono" />
            <button class="btn" @click="pickScriptFile">浏览</button>
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="showScriptModal = false">取消</button>
          <button class="btn primary" @click="saveScript">保存</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.page-head { margin-bottom: 18px; }
.page-head h2 { font-size: 18px; font-weight: 600; }
.page-head p { font-size: 12px; margin-top: 4px; }
.section { padding: 18px 20px; margin-bottom: 14px; }
.section h3 { font-size: 14px; font-weight: 600; margin-bottom: 16px; }
.section-head { display: flex; justify-content: space-between; align-items: flex-start; margin-bottom: 14px; }
.section-head h3 { margin-bottom: 4px; }
.hint { font-size: 11px; margin-top: 5px; display: block; }
.hint.warning { color: var(--warn); line-height: 1.6; }
.path-box { background: var(--bg-0); border: 1px solid var(--border); border-radius: 6px; padding: 9px 11px; font-size: 12px; color: var(--text-1); }
.script-row { display: flex; justify-content: space-between; align-items: center; padding: 10px 0; border-top: 1px solid var(--border-soft); }
.script-info { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; font-size: 12px; }
.script-info strong { font-size: 13px; }
.script-modal { width: 720px; }
.backup-list { max-height: 220px; overflow-y: auto; }
.backup-row { display: flex; justify-content: space-between; align-items: center; padding: 9px 0; border-top: 1px solid var(--border-soft); }
.backup-info { display: flex; align-items: center; gap: 9px; flex-wrap: wrap; font-size: 12px; min-width: 0; }
.backup-info strong { font-size: 12px; font-family: var(--mono, monospace); word-break: break-all; }
</style>
