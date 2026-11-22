-- 数据同步 v5：设备身份从 device_id 升级为证书指纹 + 收藏/别名/最后 IP。
-- 全部用幂等方式：db.rs 会先检测列是否存在再 ADD，这个文件只放索引定义。
CREATE INDEX IF NOT EXISTS idx_sync_devices_fingerprint ON sync_devices(fingerprint);
CREATE INDEX IF NOT EXISTS idx_sync_devices_favorite ON sync_devices(favorite);
