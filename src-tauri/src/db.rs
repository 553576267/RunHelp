use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

const MIGRATION: &str = include_str!("../../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_recycle.sql");
const BUILTIN_COMPRESS: &str = include_str!("../../scripts/compress_images.py");
const BUILTIN_BACKUP: &str = include_str!("../../scripts/backup_db.py");

/// 指针文件名：固定放在默认 app_data_dir，记录用户自定义的数据库文件位置。
/// 这样无论数据库被移到哪里，下次启动都能找到它。
pub const LOCATION_FILE: &str = "db_location.json";

/// 解析本次启动要使用的数据库路径：读默认目录下的指针文件，若其中的路径
/// 有效（绝对路径 + 文件存在）则用它，否则回退到默认目录/runhelp.db。
pub fn resolve_db_path(default_dir: &Path) -> PathBuf {
    let pointer = default_dir.join(LOCATION_FILE);
    if let Ok(text) = fs::read_to_string(&pointer) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
            if let Some(s) = v.get("path").and_then(|x| x.as_str()) {
                let p = PathBuf::from(s);
                if p.is_absolute() && p.is_file() {
                    return p;
                }
            }
        }
    }
    default_dir.join("runhelp.db")
}

pub fn init_db(db_path: &Path) -> Result<(), String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    conn.execute_batch(MIGRATION)
        .map_err(|e| format!("数据库初始化失败: {}", e))?;
    // 增量迁移：项目软删除（回收站）。ADD COLUMN 不幂等，先检测列是否存在。
    let has_deleted_at = conn
        .prepare("PRAGMA table_info(projects)")
        .and_then(|mut s| {
            let rows = s.query_map([], |r| r.get::<_, String>(1))?;
            for name in rows {
                if name? == "deleted_at" {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .unwrap_or(false);
    if !has_deleted_at {
        conn.execute("ALTER TABLE projects ADD COLUMN deleted_at TEXT DEFAULT NULL", [])
            .map_err(|e| format!("数据库迁移失败: {}", e))?;
    }
    conn.execute_batch(MIGRATION_002)
        .map_err(|e| format!("数据库迁移失败: {}", e))?;
    seed_builtin_script(&conn)?;
    seed_builtin_backup(&conn)?;
    // 上次运行期间若应用被强制关闭，run_history 里会残留 status='running'
    // 的记录，但这些进程早已不存在。启动时统一收尾为 stopped。
    conn.execute(
        "UPDATE run_history SET status='stopped', \
         ended_at=COALESCE(ended_at, datetime('now','localtime')) \
         WHERE status='running'",
        [],
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}

/// 内置数据库备份脚本：把 backup_db.py 源码存入 scripts 表，随数据库一起
/// 备份/迁移。若用户改过内置脚本内容（用户可编辑），升级时不覆盖。
fn seed_builtin_backup(conn: &Connection) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM scripts WHERE name = '内置数据库备份脚本')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;
    if !exists {
        conn.execute(
            "INSERT INTO scripts(name, language, source_type, content, file_path, version) \
             VALUES ('内置数据库备份脚本', 'python', 'builtin', ?1, '', 1)",
            [BUILTIN_BACKUP],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn seed_builtin_script(conn: &Connection) -> Result<(), String> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM scripts WHERE name = '内置图片压缩脚本')",
            [],
            |row| row.get(0),
        )
        .map_err(|e| e.to_string())?;

    if !exists {
        conn.execute(
            "INSERT INTO scripts(name, language, source_type, content, file_path, version) \
             VALUES ('内置图片压缩脚本', 'python', 'builtin', ?1, '', 1)",
            [BUILTIN_COMPRESS],
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}
