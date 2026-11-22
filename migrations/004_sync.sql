-- 数据同步：可信设备与传输历史。全部幂等，可随 001 一起重复执行。
CREATE TABLE IF NOT EXISTS sync_devices (
  device_id   TEXT PRIMARY KEY,
  name        TEXT NOT NULL,
  platform    TEXT NOT NULL DEFAULT '',
  last_seen   TEXT NOT NULL DEFAULT (datetime('now','localtime')),
  trusted     INTEGER NOT NULL DEFAULT 0,
  auto_accept INTEGER NOT NULL DEFAULT 0,
  created_at  TEXT NOT NULL DEFAULT (datetime('now','localtime'))
);

CREATE TABLE IF NOT EXISTS sync_transfers (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  direction   TEXT NOT NULL DEFAULT 'send' CHECK(direction IN ('send','receive')),
  peer_id     TEXT NOT NULL DEFAULT '',
  peer_name   TEXT NOT NULL DEFAULT '',
  kind        TEXT NOT NULL DEFAULT 'file',
  item_name   TEXT NOT NULL DEFAULT '',
  size        INTEGER NOT NULL DEFAULT 0,
  status      TEXT NOT NULL DEFAULT 'pending',
  error       TEXT NOT NULL DEFAULT '',
  created_at  TEXT NOT NULL DEFAULT (datetime('now','localtime')),
  finished_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_sync_transfers_created ON sync_transfers(created_at);
CREATE INDEX IF NOT EXISTS idx_sync_devices_seen ON sync_devices(last_seen);
