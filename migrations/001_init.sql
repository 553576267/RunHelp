PRAGMA journal_mode = WAL;
PRAGMA foreign_keys = ON;
PRAGMA busy_timeout = 5000;

CREATE TABLE IF NOT EXISTS projects (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT NOT NULL,
  path        TEXT NOT NULL,
  type        TEXT NOT NULL DEFAULT 'dev' CHECK (type IN ('dev', 'image')),
  description TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
  updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE IF NOT EXISTS commands (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  cmd         TEXT NOT NULL,
  cwd         TEXT NOT NULL DEFAULT '',
  shell       TEXT NOT NULL DEFAULT 'powershell',
  type        TEXT NOT NULL DEFAULT 'custom',
  timeout_ms  INTEGER NOT NULL DEFAULT 0,
  sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS pipelines (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS pipeline_steps (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  pipeline_id INTEGER NOT NULL REFERENCES pipelines(id) ON DELETE CASCADE,
  command_id  INTEGER REFERENCES commands(id) ON DELETE SET NULL,
  step_type   TEXT NOT NULL DEFAULT 'cmd' CHECK (step_type IN ('cmd', 'delay')),
  delay_ms    INTEGER NOT NULL DEFAULT 0,
  on_error    TEXT NOT NULL DEFAULT 'stop' CHECK (on_error IN ('stop', 'continue')),
  sort_order  INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS scripts (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT NOT NULL,
  language    TEXT NOT NULL DEFAULT 'python',
  source_type TEXT NOT NULL DEFAULT 'builtin' CHECK (source_type IN ('builtin', 'file')),
  content     TEXT NOT NULL DEFAULT '',
  file_path   TEXT NOT NULL DEFAULT '',
  version     INTEGER NOT NULL DEFAULT 1,
  updated_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE IF NOT EXISTS compression_profiles (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id  INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name        TEXT NOT NULL,
  params_json TEXT NOT NULL DEFAULT '{}',
  script_id   INTEGER REFERENCES scripts(id) ON DELETE SET NULL,
  created_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime'))
);

CREATE TABLE IF NOT EXISTS run_history (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  project_id  INTEGER REFERENCES projects(id) ON DELETE SET NULL,
  target      TEXT NOT NULL DEFAULT '',
  target_id   INTEGER,
  started_at  TEXT NOT NULL DEFAULT (datetime('now', 'localtime')),
  ended_at    TEXT,
  exit_code   INTEGER NOT NULL DEFAULT -1,
  status      TEXT NOT NULL DEFAULT 'running',
  log_path    TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL DEFAULT ''
);

CREATE INDEX IF NOT EXISTS idx_commands_project ON commands(project_id);
CREATE INDEX IF NOT EXISTS idx_pipelines_project ON pipelines(project_id);
CREATE INDEX IF NOT EXISTS idx_steps_pipeline ON pipeline_steps(pipeline_id);
CREATE INDEX IF NOT EXISTS idx_profiles_project ON compression_profiles(project_id);
CREATE INDEX IF NOT EXISTS idx_history_started ON run_history(started_at);

INSERT OR IGNORE INTO settings(key, value) VALUES ('python_path', 'python');
INSERT OR IGNORE INTO settings(key, value) VALUES ('default_shell', 'powershell');
INSERT OR IGNORE INTO settings(key, value) VALUES ('theme', 'dark');
INSERT OR IGNORE INTO settings(key, value) VALUES ('notify_on_success', '1');
INSERT OR IGNORE INTO settings(key, value) VALUES ('language', 'zh-CN');
INSERT OR IGNORE INTO settings(key, value) VALUES ('sync_focus_on_request', '1');
