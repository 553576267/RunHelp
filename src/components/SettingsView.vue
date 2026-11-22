<script setup lang="ts">
import { ref, onMounted, inject } from 'vue';
import { api } from '../lib/api';
import { t, setLocale } from '../lib/i18n';
import type { Script, BackupInfo } from '../types';

const notify = inject<(m: string) => void>('notify', () => {});
const confirm = inject<(m: string, o?: { title?: string; danger?: boolean; confirmText?: string }) => Promise<boolean>>('confirm', async () => false);
const settings = ref<Record<string, string>>({});
const shellOptions = ref<{ value: string; label: string }[]>([]);
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
  // 存量库可能没有这个新键，前端兜个默认值，避免下拉框空白
  if (!settings.value.sync_focus_on_request) settings.value.sync_focus_on_request = '1';
  shellOptions.value = await api.getShellOptions();
  // 存量 default_shell 在当前平台不可用时，界面显示平台默认值
  const shell = settings.value.default_shell;
  if (shell && !shellOptions.value.some((o) => o.value === shell)) {
    settings.value.default_shell = await api.getDefaultShell();
  }
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
    notify(t('读取备份列表失败: {error}', { error: String(e) }));
  }
}

async function saveBackupDir() {
  const dir = backupDirInput.value.trim();
  if (!dir) {
    notify(t('请填写备份目录'));
    return;
  }
  settings.value.backup_dir = dir;
  await api.setSetting('backup_dir', dir);
  await loadBackups();
  notify(t('备份目录已保存（备份脚本与恢复都会用这个目录）'));
}

async function runBackup() {
  if (backingUp.value) return;
  backingUp.value = true;
  try {
    await api.runBackup();
    notify(t('备份已启动，进度可在「运行日志」查看'));
    // 备份是异步任务，稍后刷新列表让用户看到新生成的备份文件
    setTimeout(loadBackups, 2500);
  } catch (e) {
    notify(t('备份启动失败: {error}', { error: String(e) }));
  } finally {
    backingUp.value = false;
  }
}

async function restoreBackup(b: BackupInfo) {
  const ok = await confirm(
    t('从备份恢复？\n\n备份文件：{name}\n时间：{modified}\n\n当前数据库会先自动另存一份保命，然后用该备份覆盖。恢复后会自动升级到当前结构并立即生效。', { name: b.name, modified: b.modified }),
    { title: t('从备份恢复'), danger: true, confirmText: t('确认恢复') }
  );
  if (!ok) return;
  try {
    const msg = await api.restoreBackup(b.name);
    notify(msg);
    await loadBackups();
  } catch (e) {
    notify(t('恢复失败: {error}', { error: String(e) }));
  }
}

async function saveSetting(key: string) {
  const value = settings.value[key];
  await api.setSetting(key, value);
  if (key === 'theme') {
    document.documentElement.setAttribute('data-theme', value);
    // 同步原生标题栏明暗与窗口背景色
    try { await api.setWindowTheme(value); } catch { /* 忽略 */ }
  } else if (key === 'language') {
    setLocale(value);
  }
  notify(t('设置已保存'));
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
    notify(t('请填写脚本名称'));
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
    notify(t('脚本已保存到数据库'));
  } catch (e) {
    notify(t('保存失败: {error}', { error: String(e) }));
  }
}

async function deleteScript(s: Script) {
  if (!(await confirm(t('删除脚本「{name}」？', { name: s.name }), { title: t('删除脚本'), danger: true, confirmText: t('删除') }))) return;
  await api.deleteScript(s.id);
  await load();
}

async function pickScriptFile() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const f = await open({ filters: [{ name: 'Python', extensions: ['py'] }], title: t('选择 Python 脚本') });
    if (f && typeof f === 'string') scriptForm.value.file_path = f;
  } catch {
    notify(t('请手动输入脚本路径'));
  }
}

async function pickPython() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog');
    const f = await open({ filters: [{ name: 'Python', extensions: ['exe'] }], title: t('选择 python.exe') });
    if (f && typeof f === 'string') {
      settings.value.python_path = f;
      await api.setSetting('python_path', f);
      notify(t('Python 路径已保存'));
    }
  } catch {
    notify(t('请手动输入 python.exe 路径'));
  }
}

</script>

<template>
  <div class="settings-page">
    <div class="page-head">
      <h2>{{ t('全局设置') }}</h2>
      <p class="muted">{{ t('全局配置、备份恢复与脚本管理') }}</p>
    </div>

    <div class="card section">
      <h3>{{ t('运行环境') }}</h3>
      <div class="field">
        <label>{{ t('Python 解释器路径（全局生效）') }}</label>
        <div class="row">
          <input v-model="settings.python_path" class="input mono" placeholder="D:\tools\IT\Python310\python.exe" />
          <button class="btn" @click="pickPython">{{ t('浏览') }}</button>
          <button class="btn" @click="saveSetting('python_path')">{{ t('保存') }}</button>
        </div>
        <span class="muted hint">{{ t('建议填 python.exe 完整路径。所有命令运行时会把它所在目录置于 PATH 最前，确保 python/pip/uvicorn 用的是装了依赖的那个解释器；留空则用系统 PATH（可能因解释器不对报 ModuleNotFoundError）。图片压缩脚本也用此解释器。') }}</span>
      </div>
      <div class="field" style="margin-top: 16px">
        <label>{{ t('默认 Shell') }}</label>
        <div class="row">
          <select v-model="settings.default_shell" class="select">
            <option v-for="opt in shellOptions" :key="opt.value" :value="opt.value">{{ opt.label }}</option>
          </select>
          <button class="btn" @click="saveSetting('default_shell')">{{ t('保存') }}</button>
        </div>
      </div>
      <div class="field" style="margin-top: 16px">
        <label>{{ t('界面主题') }}</label>
        <div class="row">
          <select v-model="settings.theme" class="select">
            <option value="dark">{{ t('深色') }}</option>
            <option value="light">{{ t('浅色') }}</option>
          </select>
          <button class="btn" @click="saveSetting('theme')">{{ t('应用') }}</button>
        </div>
      </div>
      <div class="field" style="margin-top: 16px">
        <label>{{ t('语言') }}</label>
        <div class="row">
          <select v-model="settings.language" class="select">
            <option value="zh-CN">{{ t('中文简体') }}</option>
            <option value="en-US">English</option>
          </select>
          <button class="btn" @click="saveSetting('language')">{{ t('保存') }}</button>
        </div>
      </div>
      <div class="field" style="margin-top: 16px">
        <label>{{ t('任务完成通知') }}</label>
        <div class="row">
          <select v-model="settings.notify_on_success" class="select">
            <option value="1">{{ t('开启（仅成功，窗口失焦时）') }}</option>
            <option value="0">{{ t('关闭') }}</option>
          </select>
          <button class="btn" @click="saveSetting('notify_on_success')">{{ t('保存') }}</button>
        </div>
        <span class="muted hint">{{ t('命令或流水线成功结束后，若窗口不在前台，会在桌面右下角提示；失败和手动停止不弹。') }}</span>
      </div>
      <div class="field" style="margin-top: 16px">
        <label>{{ t('收到传输请求时置前窗口') }}</label>
        <div class="row">
          <select v-model="settings.sync_focus_on_request" class="select">
            <option value="1">{{ t('开启（并自动切到数据同步页）') }}</option>
            <option value="0">{{ t('关闭（不打扰当前窗口）') }}</option>
          </select>
          <button class="btn" @click="saveSetting('sync_focus_on_request')">{{ t('保存') }}</button>
        </div>
        <span class="muted hint">{{ t('有人向本机发送文件或文字时，把窗口显示到桌面前端并弹出确认框；Windows 可能限制后台窗口抢占前台。') }}</span>
      </div>
    </div>

    <div class="card section">
      <div class="section-head">
        <h3>{{ t('备份与恢复') }}</h3>
        <button class="btn primary sm" :disabled="backingUp" @click="runBackup">
          {{ backingUp ? t('备份中…') : t('立即备份') }}
        </button>
      </div>
      <div class="field" style="margin-bottom: 12px">
        <label>{{ t('当前数据库文件') }}</label>
        <div class="path-box mono">{{ dbLocation?.current || dataDir }}</div>
      </div>
      <div class="field">
        <label>{{ t('备份目录') }}</label>
        <div class="row">
          <input v-model="backupDirInput" class="input mono" :placeholder="defaultBackupDir" />
          <button class="btn" @click="saveBackupDir">{{ t('保存') }}</button>
          <button class="btn" @click="loadBackups">{{ t('刷新列表') }}</button>
        </div>
        <span class="muted hint">
          {{ t('备份脚本和恢复功能共用这个目录。') }}
        </span>
      </div>
      <div class="backup-list" style="margin-top: 14px">
        <div v-if="!backups.length" class="muted hint">{{ t('该目录下暂无备份。点「立即备份」后会显示在这里。') }}</div>
        <div v-for="b in backups" :key="b.name" class="backup-row">
          <div class="backup-info">
            <strong>{{ b.name }}</strong>
            <span v-if="b.local" class="tag">{{ t('本机') }}</span>
            <span class="muted">{{ b.modified }}</span>
            <span class="muted">{{ (b.size / 1024).toFixed(0) }} KB</span>
          </div>
          <button class="btn sm danger" @click="restoreBackup(b)">{{ t('恢复') }}</button>
        </div>
      </div>
      <span class="muted hint warning" style="margin-top: 10px">
        {{ t('⚠ 恢复会覆盖当前数据库的全部数据（覆盖前会自动另存一份），仅在你确认备份内容正确时执行。两台电脑请各用各的本地库，坚果云只放备份文件，不要把数据库本体放进同步盘。') }}
      </span>
    </div>

    <div class="card section">
      <div class="section-head">
        <div>
          <h3>{{ t('脚本管理') }}</h3>
          <p class="muted hint">{{ t('这里新增的脚本，在「图片压缩」页的预设里通过"使用脚本"下拉选择后生效。脚本源码存入数据库（随库备份迁移），也支持外链本地 .py 文件。') }}</p>
        </div>
        <button class="btn sm" @click="openScript(null)">+ {{ t('新建脚本') }}</button>
      </div>
      <div v-for="s in scripts" :key="s.id" class="script-row">
        <div class="script-info">
          <strong>{{ s.name }}</strong>
          <span class="tag">{{ s.source_type === 'builtin' ? t('存于数据库') : t('外链文件') }}</span>
          <span class="muted">{{ s.language }} · v{{ s.version }}</span>
          <span class="muted">{{ s.updated_at }}</span>
        </div>
        <div class="row" style="gap: 6px">
          <button class="btn sm" @click="openScript(s)">{{ t('编辑') }}</button>
          <button class="btn sm danger" @click="deleteScript(s)">{{ t('删除') }}</button>
        </div>
      </div>
    </div>

    <div v-if="showScriptModal" class="modal-mask" @click.self="showScriptModal = false">
      <div class="modal script-modal">
        <h3>{{ editingScript ? t('编辑脚本') : t('新建脚本') }}</h3>
        <div class="field">
          <label>{{ t('脚本名称') }}</label>
          <input v-model="scriptForm.name" class="input" />
        </div>
        <div class="field" style="margin-top: 14px">
          <label>{{ t('来源') }}</label>
          <select v-model="scriptForm.source_type" class="select">
            <option value="builtin">{{ t('存于数据库') }}</option>
            <option value="file">{{ t('外链本地文件') }}</option>
          </select>
        </div>
        <div v-if="scriptForm.source_type === 'builtin'" class="field" style="margin-top: 14px">
          <label>{{ t('脚本源码（Python）') }}</label>
          <textarea v-model="scriptForm.content" class="textarea" rows="14"></textarea>
        </div>
        <div v-else class="field" style="margin-top: 14px">
          <label>{{ t('脚本文件路径') }}</label>
          <div class="row">
            <input v-model="scriptForm.file_path" class="input mono" />
            <button class="btn" @click="pickScriptFile">{{ t('浏览') }}</button>
          </div>
        </div>
        <div class="modal-actions">
          <button class="btn" @click="showScriptModal = false">{{ t('取消') }}</button>
          <button class="btn primary" @click="saveScript">{{ t('保存') }}</button>
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
