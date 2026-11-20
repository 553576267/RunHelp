-- 需求：项目软删除（回收站）索引。注意：ADD COLUMN 不幂等，
-- 由 db.rs 检测列是否存在后再执行；本文件只放幂等的索引定义。
CREATE INDEX IF NOT EXISTS idx_projects_deleted ON projects(deleted_at);
