//! Persistent, metadata-only FTS5 index. Prompts never enumerate it. Full scans
//! are explicit (or first discovery only); ordinary searches/load calls are bounded.
use super::Skill;
use anyhow::Context;
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use std::path::{Component, Path, PathBuf};

pub const MAX_RESULTS: usize = 20;

pub struct SkillIndex {
    conn: Connection,
    root: PathBuf,
}

#[derive(Debug, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub description: String,
    pub required_parameters: Vec<String>,
    pub skill_hidden: bool,
    pub user_only: bool,
    pub version: Option<String>,
}

impl From<Skill> for SkillSummary {
    fn from(skill: Skill) -> Self {
        Self {
            name: skill.name,
            description: crate::util::truncate_chars(&skill.description, 320).to_string(),
            required_parameters: skill.required_parameters,
            skill_hidden: skill.skill_hidden,
            user_only: skill.user_only,
            version: skill.version,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SearchResults {
    pub skills: Vec<SkillSummary>,
    pub has_more: bool,
    pub next_after: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct IndexReport {
    pub indexed: usize,
    pub skipped: usize,
}

impl SkillIndex {
    pub fn open(data_dir: &Path, root: &Path) -> anyhow::Result<Self> {
        let root = root
            .canonicalize()
            .context("Skills directory is unavailable")?;
        anyhow::ensure!(root.is_dir(), "Skills path is not a directory");
        std::fs::create_dir_all(data_dir)?;
        let file = data_dir.join("skill-index.sqlite");
        if let Ok(meta) = std::fs::symlink_metadata(&file) {
            anyhow::ensure!(
                meta.is_file() && !meta.file_type().is_symlink(),
                "Invalid skill index file"
            );
        }
        let conn = Connection::open(file)?;
        conn.busy_timeout(std::time::Duration::from_secs(10))?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        anyhow::ensure!(version <= 1, "Unsupported skill index version");
        if version == 0 {
            conn.pragma_update(None, "journal_mode", "WAL")?;
            conn.execute_batch("BEGIN IMMEDIATE;
                CREATE TABLE IF NOT EXISTS skill_roots (root TEXT PRIMARY KEY);
                CREATE TABLE IF NOT EXISTS skill_catalog (
                    id INTEGER PRIMARY KEY, root TEXT NOT NULL, name TEXT NOT NULL,
                    description TEXT NOT NULL, folder TEXT NOT NULL,
                    skill_hidden INTEGER NOT NULL, user_only INTEGER NOT NULL,
                    UNIQUE(root, name), UNIQUE(root, folder)
                );
                CREATE VIRTUAL TABLE IF NOT EXISTS skill_search USING fts5(
                    name, description, content='skill_catalog', content_rowid='id',
                    tokenize='unicode61 remove_diacritics 2', prefix='2 3 4'
                );
                CREATE TRIGGER IF NOT EXISTS skill_insert AFTER INSERT ON skill_catalog BEGIN
                    INSERT INTO skill_search(rowid,name,description) VALUES (new.id,new.name,new.description);
                END;
                CREATE TRIGGER IF NOT EXISTS skill_delete AFTER DELETE ON skill_catalog BEGIN
                    INSERT INTO skill_search(skill_search,rowid,name,description) VALUES ('delete',old.id,old.name,old.description);
                END;
                PRAGMA user_version=1;
                COMMIT;")?;
        }
        Ok(Self { conn, root })
    }

    /// Once per root, persisted across restarts. No mtime polling or per-turn scan.
    pub fn ensure_indexed(&mut self) -> anyhow::Result<()> {
        let ready: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM skill_roots WHERE root=?1)",
            [self.root.to_string_lossy()],
            |row| row.get(0),
        )?;
        if !ready {
            self.rebuild_inner(true)?;
        }
        Ok(())
    }

    /// Streaming directory walk, transactional replacement: memory does not grow
    /// with the number of skills. Duplicate names abort instead of shadowing policy.
    pub fn rebuild(&mut self) -> anyhow::Result<IndexReport> {
        self.rebuild_inner(false)
    }

    fn rebuild_inner(&mut self, only_if_missing: bool) -> anyhow::Result<IndexReport> {
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        if only_if_missing {
            let ready: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM skill_roots WHERE root=?1)",
                [self.root.to_string_lossy()],
                |row| row.get(0),
            )?;
            if ready {
                return Ok(IndexReport::default());
            }
        }
        tx.execute(
            "DELETE FROM skill_catalog WHERE root=?1",
            [self.root.to_string_lossy()],
        )?;
        let mut report = IndexReport::default();
        walk(&self.root, 0, &mut |folder| {
            match Skill::from_dir(&self.root, folder) {
                Ok(skill) => {
                    insert(&tx, &self.root, &skill)?;
                    report.indexed += 1;
                }
                Err(error) => {
                    tracing::warn!(path = %folder.display(), %error, "Skipping invalid skill manifest");
                    report.skipped += 1;
                }
            }
            Ok(())
        })?;
        tx.execute(
            "INSERT OR IGNORE INTO skill_roots(root) VALUES (?1)",
            [self.root.to_string_lossy()],
        )?;
        tx.commit()?;
        Ok(report)
    }

    /// Incremental registration after authoring: only one reviewed folder is read.
    /// A different folder cannot take over an existing registered name.
    pub fn refresh(&mut self, relative: &str) -> anyhow::Result<SkillSummary> {
        anyhow::ensure!(
            !relative.is_empty()
                && Path::new(relative)
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "Invalid relative skill folder"
        );
        let skill = Skill::from_dir(&self.root, &self.root.join(relative))?;
        let tx = self
            .conn
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "DELETE FROM skill_catalog WHERE root=?1 AND folder=?2",
            params![self.root.to_string_lossy(), skill.folder],
        )?;
        insert(&tx, &self.root, &skill)?;
        tx.commit()?;
        Ok(skill.into())
    }

    pub fn lookup(&self, name: &str) -> anyhow::Result<Skill> {
        super::validate_name(name)?;
        let folder: Option<String> = self
            .conn
            .query_row(
                "SELECT folder FROM skill_catalog WHERE root=?1 AND name=?2",
                params![self.root.to_string_lossy(), name],
                |row| row.get(0),
            )
            .optional()?;
        let folder = folder.ok_or_else(|| anyhow::anyhow!("Skill is not registered: {name}"))?;
        let skill = Skill::from_dir(&self.root, Path::new(&folder))?;
        anyhow::ensure!(
            skill.name == name,
            "Skill manifest changed; refresh the index"
        );
        Ok(skill)
    }

    /// Literal Unicode word-prefix search, not caller-controlled FTS syntax/SQL.
    /// Hidden entries are excluded for the model, but visible to an authorized user.
    pub fn search(
        &self,
        query: &str,
        limit: usize,
        include_hidden: bool,
    ) -> anyhow::Result<SearchResults> {
        anyhow::ensure!(
            (1..=MAX_RESULTS).contains(&limit),
            "Skill result limit must be 1..={MAX_RESULTS}"
        );
        anyhow::ensure!(query.chars().count() <= 512, "Skill query is too long");
        let words: Vec<_> = query
            .split(|c: char| !c.is_alphanumeric())
            .filter(|s| !s.is_empty())
            .take(17)
            .collect();
        anyhow::ensure!(words.len() <= 16, "Use at most 16 search words");
        if words.is_empty() {
            return self.browse("", limit, include_hidden);
        }
        let literal = words
            .iter()
            .map(|s| format!("\"{s}\"*"))
            .collect::<Vec<_>>()
            .join(" AND ");
        let mut stmt = self.conn.prepare(
            "SELECT c.name FROM skill_search
            JOIN skill_catalog c ON c.id=skill_search.rowid
            WHERE skill_search MATCH ?1 AND c.root=?2 AND (?3 OR c.skill_hidden=0)
            ORDER BY rank LIMIT ?4",
        )?;
        let names = stmt
            .query_map(
                params![
                    literal,
                    self.root.to_string_lossy(),
                    include_hidden,
                    (limit + 1) as i64
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        self.summaries(names, limit, include_hidden, false)
    }

    /// Keyset pagination for human browsing; never OFFSET through a million rows.
    pub fn browse(
        &self,
        after: &str,
        limit: usize,
        include_hidden: bool,
    ) -> anyhow::Result<SearchResults> {
        anyhow::ensure!(
            (1..=MAX_RESULTS).contains(&limit),
            "Invalid skill result limit"
        );
        if !after.is_empty() {
            super::validate_name(after)?;
        }
        let mut stmt = self.conn.prepare(
            "SELECT name FROM skill_catalog
            WHERE root=?1 AND name>?2 AND (?3 OR skill_hidden=0) ORDER BY name LIMIT ?4",
        )?;
        let names = stmt
            .query_map(
                params![
                    self.root.to_string_lossy(),
                    after,
                    include_hidden,
                    (limit + 1) as i64
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?;
        self.summaries(names, limit, include_hidden, true)
    }

    fn summaries(
        &self,
        names: Vec<String>,
        limit: usize,
        include_hidden: bool,
        paged: bool,
    ) -> anyhow::Result<SearchResults> {
        let has_more = names.len() > limit;
        let next_after = if paged && has_more {
            names.get(limit - 1).cloned()
        } else {
            None
        };
        // Re-read only returned manifests: stale flags/deleted files fail closed.
        // Do not refill indefinitely through stale entries or read instruction bodies.
        let skills = names
            .into_iter()
            .take(limit)
            .filter_map(|name| self.lookup(&name).ok())
            .filter(|s| include_hidden || !s.skill_hidden)
            .map(Into::into)
            .collect();
        Ok(SearchResults {
            skills,
            has_more,
            next_after,
        })
    }
}

fn insert(conn: &Connection, root: &Path, skill: &Skill) -> anyhow::Result<()> {
    conn.execute("INSERT INTO skill_catalog(root,name,description,folder,skill_hidden,user_only) VALUES (?1,?2,?3,?4,?5,?6)",
        params![root.to_string_lossy(), skill.name, skill.description, skill.folder, skill.skill_hidden, skill.user_only])
        .with_context(|| format!("Cannot index skill '{}': duplicate name/folder or index write failure", skill.name))?;
    Ok(())
}

fn walk(
    dir: &Path,
    depth: usize,
    visit: &mut impl FnMut(&Path) -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    anyhow::ensure!(depth <= 16, "Skill directory nesting exceeds 16 levels");
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() || entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let folder = entry.path();
        if super::is_skill_dir(&folder) {
            visit(&folder)?;
        } else {
            walk(&folder, depth + 1, visit)?;
        }
    }
    Ok(())
}

/// No full scan for ordinary canonical skills, even on first activation. Aliased
/// or sharded folders are resolved via the persisted index, never guessed paths.
pub fn lookup_skill(db: &crate::db::Database, root: &Path, name: &str) -> anyhow::Result<Skill> {
    super::validate_name(name)?;
    // An already registered aliased/sharded folder wins over a newly created
    // canonical folder; a duplicate must not silently shadow its activation flags.
    if db.data_dir().join("skill-index.sqlite").is_file() && root.is_dir() {
        let index = SkillIndex::open(&db.data_dir(), root)?;
        let registered: bool = index.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM skill_catalog WHERE root=?1 AND name=?2)",
            params![index.root.to_string_lossy(), name],
            |row| row.get(0),
        )?;
        if registered {
            return index.lookup(name);
        }
    }
    let canonical = root.join(name);
    if super::is_skill_dir(&canonical) {
        let skill = Skill::from_dir(root, &canonical)?;
        anyhow::ensure!(
            skill.name == name,
            "Skill folder/name mismatch; use the registered name"
        );
        return Ok(skill);
    }
    anyhow::ensure!(root.is_dir(), "Skill is not registered: {name}");
    let mut index = SkillIndex::open(&db.data_dir(), root)?;
    index.ensure_indexed()?;
    index.lookup(name)
}
