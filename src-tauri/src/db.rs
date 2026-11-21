use std::cmp::Reverse;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, Transaction};

const MIGRATION_001: &str = include_str!("../../migrations/001_init.sql");
const MIGRATION_002: &str = include_str!("../../migrations/002_recycle.sql");
const BUILTIN_COMPRESS: &str = include_str!("../../scripts/compress_images.py");
const BUILTIN_BACKUP: &str = include_str!("../../scripts/backup_db.py");

/// 指针文件名：固定放在默认 app_data_dir，记录用户自定义的数据库文件位置。
/// 这样无论数据库被移到哪里，下次启动都能找到它。
pub const LOCATION_FILE: &str = "db_location.json";

/// 当前库结构版本。新增迁移时递增，并在 migrate() 里追加对应步骤。
pub const SCHEMA_VERSION: u32 = 2;

/// 迁移前自动备份的存放目录（相对于数据库所在目录）。
const MIGRATION_BACKUP_DIR: &str = "migration_backups";
const KEEP_MIGRATION_BACKUPS: usize = 5;

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

/// 打开数据库并把结构升级到 SCHEMA_VERSION。
/// 安装包只替换程序文件，数据库留在 AppData；每次启动和每次恢复备份后都会调用这里。
pub fn init_db(db_path: &Path) -> Result<(), String> {
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut conn = Connection::open(db_path).map_err(|e| e.to_string())?;
    let lang = crate::i18n::lang_from_conn(&conn);

    // 基础建表保持“每次启动自愈”：一律执行，靠 IF NOT EXISTS / INSERT OR IGNORE 保证幂等。
    // 这样即使老库缺表、缺索引或缺默认设置，也能补回来。
    conn.execute_batch(MIGRATION_001)
        .map_err(|e| crate::i18n::trf(&lang, "数据库初始化失败: {error}", &[("error", &e.to_string())]))?;

    let version = schema_version(&conn)?;
    if version < SCHEMA_VERSION {
        // 迁移前先留一份完整快照。backup API 能正确处理 WAL 里尚未 checkpoint 的数据。
        let stamp = db_timestamp(&conn)?;
        let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
        backup_before_migration(db_path, version, &stamp, &lang)?;
        migrate(&mut conn, version)?;
    }

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

fn schema_version(conn: &Connection) -> Result<u32, String> {
    let lang = crate::i18n::lang_from_conn(conn);
    conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i32>(0))
        .map(|v| v.max(0) as u32)
        .map_err(|e| crate::i18n::trf(&lang, "读取数据库版本失败: {error}", &[("error", &e.to_string())]))
}

fn set_schema_version(conn: &Connection, version: u32) -> Result<(), String> {
    let lang = crate::i18n::lang_from_conn(conn);
    conn.execute_batch(&format!("PRAGMA user_version = {}", version))
        .map_err(|e| crate::i18n::trf(&lang, "写入数据库版本失败: {error}", &[("error", &e.to_string())]))
}

fn db_timestamp(conn: &Connection) -> Result<String, String> {
    conn.query_row(
        "SELECT strftime('%Y%m%d_%H%M%S', 'now', 'localtime')",
        [],
        |r| r.get(0),
    )
    .map_err(|e| e.to_string())
}

/// 按版本顺序执行增量迁移，整批包在一个事务里；任何一步失败则全部回滚。
fn migrate(conn: &mut Connection, from: u32) -> Result<(), String> {
    let lang = crate::i18n::lang_from_conn(conn);
    let tx = conn
        .transaction()
        .map_err(|e| crate::i18n::trf(&lang, "开启数据库迁移事务失败: {error}", &[("error", &e.to_string())]))?;

    if from < 2 {
        migrate_to_v2(&tx)?;
    }
    // 后续新增表结构改动时：if from < 3 { migrate_to_v3(&tx)?; } 并把 SCHEMA_VERSION 改成 3。

    set_schema_version(&tx, SCHEMA_VERSION)?;
    tx.commit().map_err(|e| crate::i18n::trf(&lang, "提交数据库迁移失败: {error}", &[("error", &e.to_string())]))
}

/// v2：项目软删除（回收站）。ADD COLUMN 不幂等，先检测列是否存在；
/// 002_recycle.sql 只放幂等索引定义。
fn migrate_to_v2(conn: &Transaction<'_>) -> Result<(), String> {
    let lang = crate::i18n::lang_from_conn(conn);
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
            .map_err(|e| crate::i18n::trf(&lang, "数据库迁移失败: {error}", &[("error", &e.to_string())]))?;
    }
    conn.execute_batch(MIGRATION_002)
        .map_err(|e| crate::i18n::trf(&lang, "数据库迁移失败: {error}", &[("error", &e.to_string())]))
}

/// 迁移前快照：用 SQLite 在线备份 API，而不是 fs::copy，避免漏掉 WAL 中的数据。
fn backup_before_migration(db_path: &Path, from: u32, stamp: &str, lang: &str) -> Result<PathBuf, String> {
    let parent = db_path
        .parent()
        .ok_or_else(|| crate::i18n::tr(lang, "数据库路径没有父目录"))?;
    let dir = parent.join(MIGRATION_BACKUP_DIR);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let stem = db_path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let backup_path = dir.join(format!("{}.bak-v{}-{}.db", stem, from, stamp));

    let src = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| crate::i18n::trf(lang, "打开迁移源库失败: {error}", &[("error", &e.to_string())]))?;
    if backup_path.exists() {
        return Err(crate::i18n::trf(
            lang,
            "迁移备份文件已存在: {path}",
            &[("path", &backup_path.display().to_string())],
        ));
    }
    let mut dst = Connection::open(&backup_path)
        .map_err(|e| crate::i18n::trf(lang, "创建迁移备份失败: {error}", &[("error", &e.to_string())]))?;
    {
        let backup = rusqlite::backup::Backup::new(&src, &mut dst)
            .map_err(|e| crate::i18n::trf(lang, "初始化迁移备份失败: {error}", &[("error", &e.to_string())]))?;
        backup
            .run_to_completion(100, Duration::from_millis(250), None)
            .map_err(|e| crate::i18n::trf(lang, "写入迁移备份失败: {error}", &[("error", &e.to_string())]))?;
    }
    // 备份库切回 DELETE 模式，避免单独拷走时 WAL 内容丢失。
    let _ = dst.execute_batch("PRAGMA journal_mode=DELETE;");
    drop(dst);
    drop(src);

    prune_migration_backups(&dir, &stem)?;
    Ok(backup_path)
}

fn prune_migration_backups(dir: &Path, stem: &str) -> Result<(), String> {
    let prefix = format!("{}.bak-v", stem);
    let mut files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| e.to_string())?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.file_name()
                    .and_then(|n| n.to_str())
                    .map(|n| n.starts_with(&prefix))
                    .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|p| Reverse(p.metadata().and_then(|m| m.modified()).ok()));
    for old in files.into_iter().skip(KEEP_MIGRATION_BACKUPS) {
        let _ = fs::remove_file(old);
    }
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
