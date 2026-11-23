//! Where Coursify keeps things on disk.
//!
//! App internals (database, sealed exams, downloads, logs) live in `%LOCALAPPDATA%\Coursify`.
//! Anything meant for the learner (transcript, completed-course archives) lives in `Documents\Coursify`.

use std::io;
use std::path::{Path, PathBuf};

pub const APP_DIR_NAME: &str = "Coursify";
/// The folder name used before the app was renamed to Coursify.
pub const OLD_APP_DIR_NAME: &str = "SelfSemester";

/// What's carried over from the old folders: data-folder items, then Documents-folder items.
const OLD_DATA_ITEMS: &[&str] = &["app.db", "app.db-wal", "app.db-shm", "courses", "domains", "tools", "tool-packs"];
const OLD_USER_ITEMS: &[&str] = &["Completed", "Backups", "Course builder"];

fn copy_item(from: &Path, to: &Path) -> io::Result<()> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        for e in std::fs::read_dir(from)? {
            let e = e?;
            copy_item(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(from, to).map(|_| ())
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AppPaths {
    /// `%LOCALAPPDATA%\Coursify`
    pub data_dir: PathBuf,
    /// `Documents\Coursify`
    pub user_dir: PathBuf,
}

impl AppPaths {
    /// Standard Windows locations, or everything under `COURSIFY_HOME` when that's set (for a separate test or
    /// demo copy that mustn't touch real courses).
    pub fn standard() -> io::Result<Self> {
        if let Some(home) = std::env::var_os("COURSIFY_HOME").filter(|h| !h.is_empty()) {
            let home = Path::new(&home);
            return Ok(Self::rooted(&home.join("AppData"), &home.join("Documents")));
        }
        let local = dirs::data_local_dir()
            .ok_or_else(|| io::Error::other("could not find the local app data folder"))?;
        let docs = dirs::document_dir()
            .ok_or_else(|| io::Error::other("could not find the Documents folder"))?;
        Ok(Self::rooted(&local, &docs))
    }

    /// Paths under explicit roots (used by tests).
    pub fn rooted(local_root: &Path, docs_root: &Path) -> Self {
        Self {
            data_dir: local_root.join(APP_DIR_NAME),
            user_dir: docs_root.join(APP_DIR_NAME),
        }
    }

    /// Paths the app used under its old name, next to these ones.
    pub fn old_name(&self) -> Self {
        let sibling = |p: &Path| p.parent().map_or_else(|| PathBuf::from(OLD_APP_DIR_NAME), |d| d.join(OLD_APP_DIR_NAME));
        Self { data_dir: sibling(&self.data_dir), user_dir: sibling(&self.user_dir) }
    }

    /// On the first launch after the rename, copy your courses and files from the old `SelfSemester` folders.
    /// Only runs when there's no Coursify database yet. The old folders are copied, never moved or deleted, so
    /// nothing is lost if this is interrupted. Returns whether anything was copied.
    pub fn migrate_from_old_name(&self) -> io::Result<bool> {
        let old = self.old_name();
        if self.db_file().exists() || !old.db_file().exists() {
            return Ok(false);
        }
        for (items, from, to) in [(OLD_USER_ITEMS, &old.user_dir, &self.user_dir), (OLD_DATA_ITEMS, &old.data_dir, &self.data_dir)] {
            for item in items {
                let src = from.join(item);
                // the database goes last (below), so a half-finished copy is tried again next launch
                if src.exists() && *item != "app.db" {
                    copy_item(&src, &to.join(item))?;
                }
            }
        }
        copy_item(&old.db_file(), &self.db_file())?;
        Ok(true)
    }

    pub fn db_file(&self) -> PathBuf {
        self.data_dir.join("app.db")
    }
    pub fn domains_dir(&self) -> PathBuf {
        self.data_dir.join("domains")
    }
    pub fn courses_dir(&self) -> PathBuf {
        self.data_dir.join("courses")
    }
    pub fn course_dir(&self, course_id: &str) -> PathBuf {
        self.courses_dir().join(course_id)
    }
    /// Empty working folder Claude Code runs in, so it never picks up a project's CLAUDE.md or files.
    pub fn claude_workdir(&self) -> PathBuf {
        self.data_dir.join("claude-work")
    }
    pub fn logs_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }
    pub fn completed_dir(&self) -> PathBuf {
        self.user_dir.join("Completed")
    }
    /// Safety copies of your data, made before each update (see backup.rs). Outside the program folder on purpose.
    pub fn backups_dir(&self) -> PathBuf {
        self.user_dir.join("Backups")
    }

    /// Create every top-level folder. Safe to call on each launch.
    pub fn ensure(&self) -> io::Result<()> {
        for dir in [
            self.data_dir.clone(),
            self.domains_dir(),
            self.courses_dir(),
            self.claude_workdir(),
            self.logs_dir(),
            self.user_dir.clone(),
            self.completed_dir(),
        ] {
            std::fs::create_dir_all(dir)?;
        }
        Ok(())
    }

    /// Per-course folder layout (see docs/RESEARCH.md "Storage").
    pub fn ensure_course(&self, course_id: &str) -> io::Result<PathBuf> {
        let root = self.course_dir(course_id);
        for sub in ["units", "sealed", "sources", "submissions", "materials"] {
            std::fs::create_dir_all(root.join(sub))?;
        }
        Ok(root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_layout() {
        let tmp = tempfile::tempdir().unwrap();
        let p = AppPaths::rooted(&tmp.path().join("local"), &tmp.path().join("docs"));
        p.ensure().unwrap();
        let c = p.ensure_course("c1").unwrap();
        assert!(p.db_file().parent().unwrap().is_dir());
        assert!(p.completed_dir().is_dir());
        assert!(p.claude_workdir().is_dir());
        assert!(c.join("sealed").is_dir());
        assert!(c.ends_with("Coursify/courses/c1") || c.ends_with("Coursify\\courses\\c1"));
    }

    #[test]
    fn copies_data_from_the_old_name_once() {
        let tmp = tempfile::tempdir().unwrap();
        let p = AppPaths::rooted(&tmp.path().join("local"), &tmp.path().join("docs"));
        let old = p.old_name();
        assert!(old.data_dir.ends_with(OLD_APP_DIR_NAME));
        // nothing old: nothing to do
        assert!(!p.migrate_from_old_name().unwrap());
        old.ensure().unwrap();
        old.ensure_course("c1").unwrap();
        std::fs::write(old.db_file(), b"db").unwrap();
        std::fs::write(old.course_dir("c1").join("units").join("u01.json"), b"{}").unwrap();
        std::fs::write(old.completed_dir().join("summary.md"), b"done").unwrap();
        assert!(p.migrate_from_old_name().unwrap());
        assert_eq!(std::fs::read(p.db_file()).unwrap(), b"db");
        assert!(p.course_dir("c1").join("units").join("u01.json").is_file());
        assert!(p.completed_dir().join("summary.md").is_file());
        assert!(old.db_file().is_file(), "the old folders are left as they were");
        // already has its own data: never copied over again
        assert!(!p.migrate_from_old_name().unwrap());
    }
}
