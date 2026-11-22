use crate::error::AppResult;
use rusqlite::Connection;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

#[derive(Clone)]
pub struct Db(pub Arc<Mutex<Connection>>);

impl Db {
    pub fn open(path: &Path) -> AppResult<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.pragma_update(None, "busy_timeout", 8000)?;
        conn.pragma_update(None, "synchronous", "FULL")?;
        let db = Db(Arc::new(Mutex::new(conn)));
        db.migrate()?;
        Ok(db)
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn migrate(&self) -> AppResult<()> {
        let mut c = self.conn();
        c.execute_batch(SCHEMA)?;
        let has_auto_title: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('conversations') WHERE name='auto_title_pending')",
            [],
            |row| row.get(0),
        )?;
        if !has_auto_title {
            c.execute("ALTER TABLE conversations ADD COLUMN auto_title_pending INTEGER NOT NULL DEFAULT 0", [])?;
        }
        // NULL on upgrade: enabled configurations predate persistent launch consent.
        let has_mcp_consent: bool = c.query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mcp_servers') WHERE name='startup_digest')",
            [], |row| row.get(0),
        )?;
        if !has_mcp_consent {
            c.execute("ALTER TABLE mcp_servers ADD COLUMN startup_digest BLOB", [])?;
        }
        // lightweight forward migrations
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            let tx = c.transaction()?;
            #[cfg(windows)]
            if version < 3 {
                crate::mcp::config::migrate_playwright_browser(&tx)?;
            }
            if version < 4 {
                // Bare aliases now use a different executable and package source. Explicit
                // paths, other commands and HTTP keep their previous startup consent.
                tx.execute(
                    "UPDATE mcp_servers SET startup_digest=NULL WHERE transport='stdio'
                    AND lower(command) IN ('npx','npx.cmd','npx.exe','uvx','uvx.exe')",
                    [],
                )?;
            }
            if version < 5 {
                let has_attachments: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('messages') WHERE name='attachments_json')",
                    [],
                    |row| row.get(0),
                )?;
                if !has_attachments {
                    tx.execute("ALTER TABLE messages ADD COLUMN attachments_json TEXT", [])?;
                }
            }
            // v1 already used the package-only arguments. v6 supersedes v2's index addition.
            if version < 6 {
                crate::mcp::config::migrate_markitdown_to_launcher_index(&tx)?;
            }
            if version < 7 {
                let has_credential_url: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mcp_servers') WHERE name='credential_url')",
                    [],
                    |row| row.get(0),
                )?;
                if !has_credential_url {
                    tx.execute("ALTER TABLE mcp_servers ADD COLUMN credential_url TEXT", [])?;
                }
                crate::mcp::config::migrate_credential_urls(&tx)?;
            }
            if version < 8 {
                let has_plugin_slug: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mcp_servers') WHERE name='plugin_slug')",
                    [],
                    |row| row.get(0),
                )?;
                if !has_plugin_slug {
                    tx.execute(
                        "ALTER TABLE mcp_servers ADD COLUMN plugin_slug TEXT REFERENCES plugins(slug) ON DELETE CASCADE",
                        [],
                    )?;
                }
                tx.execute(
                    "CREATE INDEX IF NOT EXISTS mcp_servers_plugin_slug ON mcp_servers(plugin_slug)
                     WHERE plugin_slug IS NOT NULL",
                    [],
                )?;
            }
            if version < 9 {
                crate::mcp::config::migrate_3dm_to_http(&tx)?;
            }
            if version < 10 {
                let has_description: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mcp_servers') WHERE name='description')",
                    [],
                    |row| row.get(0),
                )?;
                if !has_description {
                    tx.execute("ALTER TABLE mcp_servers ADD COLUMN description TEXT", [])?;
                }
                crate::mcp::config::migrate_descriptions(&tx)?;
            }
            if version < 11 {
                crate::mcp::config::migrate_unpinned_presets(&tx)?;
            }
            if version < 12 {
                crate::mcp::config::migrate_playwright_downloads(&tx)?;
            }
            if version < 13 {
                let has_context_checkpoint: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('conversations') WHERE name='context_summary_through_message_id')",
                    [],
                    |row| row.get(0),
                )?;
                if !has_context_checkpoint {
                    tx.execute(
                        "ALTER TABLE conversations ADD COLUMN context_summary_through_message_id TEXT",
                        [],
                    )?;
                }
                tx.execute_batch(CONTEXT_CHECKPOINT_SCHEMA)?;
            }
            if version < 15 {
                let has_author_hint: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM pragma_table_info('mod_library_entries') WHERE name='author_hint')",
                    [],
                    |row| row.get(0),
                )?;
                if !has_author_hint {
                    tx.execute(
                        "ALTER TABLE mod_library_entries ADD COLUMN author_hint TEXT",
                        [],
                    )?;
                }
            }
            if version < 16 {
                // Desktop and Agent used the same draft writer. Only a successful
                // Agent receipt matching both the draft ID and write plan proves origin.
                tx.execute_batch("WITH receipts(name, payload) AS (
                      SELECT name, CASE WHEN json_valid(result_json) THEN result_json END
                      FROM tool_calls WHERE status='done' AND name IN ('write_plugin_draft','import_cloud_plugin')
                    ) INSERT OR IGNORE INTO agent_plugin_drafts(draft_id)
                    SELECT d.id FROM receipts r JOIN plugin_drafts d ON d.id = CASE r.name
                      WHEN 'write_plugin_draft' THEN json_extract(r.payload,'$.preview.draft.id')
                      WHEN 'import_cloud_plugin' THEN json_extract(r.payload,'$.draftId') END
                    WHERE json_extract(r.payload,'$.status')='committed'
                      AND json_extract(r.payload,'$.plan.id')=d.write_transaction_id;")?;
            }
            if version < 17 {
                crate::mcp::config::migrate_bundled_playwright(&tx)?;
            }
            tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            tx.commit()?;
        }
        drop(c);
        crate::skills::migrate(self)?;
        crate::plugin_publish::migrate(&self.conn())?;
        crate::manager_publication::migrate(&self.conn())?;
        crate::game_identity::migrate(&self.conn())?;
        Ok(())
    }
}

pub const SCHEMA_VERSION: i64 = 17;

// SQLite appends rowid to the index key: these indexes bound scans to the
// conversation's insertion-order suffix and the most recent retained user.
// Invalidate a covered prefix on edits/deletions even when the runtime writes
// messages directly rather than using History's update methods. Attachment
// edits go through History's rewind, which clears the checkpoint explicitly.
const CONTEXT_CHECKPOINT_SCHEMA: &str = r#"
CREATE INDEX IF NOT EXISTS idx_messages_conv_rowid ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_messages_conv_users ON messages(conversation_id) WHERE role='user';

CREATE TRIGGER IF NOT EXISTS invalidate_context_checkpoint_on_message_update
AFTER UPDATE OF role, content, tool_calls_json, tool_call_id ON messages
WHEN OLD.role IS NOT NEW.role OR OLD.content IS NOT NEW.content
  OR OLD.tool_calls_json IS NOT NEW.tool_calls_json OR OLD.tool_call_id IS NOT NEW.tool_call_id
BEGIN
  UPDATE conversations SET task_summary=NULL, context_summary_through_message_id=NULL
  WHERE id=OLD.conversation_id AND EXISTS (
    SELECT 1 FROM messages WHERE id=context_summary_through_message_id
      AND conversation_id=OLD.conversation_id AND rowid>=OLD.rowid
  );
END;

CREATE TRIGGER IF NOT EXISTS invalidate_context_checkpoint_on_message_delete
BEFORE DELETE ON messages
BEGIN
  UPDATE conversations SET task_summary=NULL, context_summary_through_message_id=NULL
  WHERE id=OLD.conversation_id AND EXISTS (
    SELECT 1 FROM messages WHERE id=context_summary_through_message_id
      AND conversation_id=OLD.conversation_id AND rowid>=OLD.rowid
  );
END;
"#;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS games (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  slug TEXT NOT NULL,
  root_path TEXT NOT NULL UNIQUE,
  platform TEXT NOT NULL DEFAULT 'unknown',
  adapter_slug TEXT,
  adapter_version TEXT,
  confidence REAL NOT NULL DEFAULT 0,
  cover_path TEXT,
  steam_app_id TEXT,
  last_scanned_at TEXT,
  installed_mod_count INTEGER NOT NULL DEFAULT 0,
  enabled_mod_count INTEGER NOT NULL DEFAULT 0,
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS mod_sources (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  kind TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  has_secret INTEGER NOT NULL DEFAULT 0,
  config_json TEXT NOT NULL DEFAULT '{}',
  sort_order INTEGER NOT NULL DEFAULT 0
);

CREATE TABLE IF NOT EXISTS mods (
  id TEXT PRIMARY KEY,
  source_id TEXT NOT NULL,
  source_mod_id TEXT NOT NULL,
  game_id TEXT,
  name TEXT NOT NULL,
  summary TEXT,
  author TEXT,
  version TEXT,
  download_url TEXT,
  homepage_url TEXT,
  icon_url TEXT,
  file_size INTEGER,
  sha256 TEXT,
  tags_json TEXT NOT NULL DEFAULT '[]',
  metadata_json TEXT NOT NULL DEFAULT '{}',
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_mods_source ON mods(source_id, source_mod_id);
CREATE INDEX IF NOT EXISTS idx_mods_game ON mods(game_id);
CREATE TABLE IF NOT EXISTS download_artifacts (
  id TEXT PRIMARY KEY,
  path TEXT NOT NULL UNIQUE,
  filename TEXT NOT NULL,
  artifact_json TEXT NOT NULL,
  source_id TEXT NOT NULL,
  project_id TEXT NOT NULL,
  version_id TEXT NOT NULL,
  name TEXT NOT NULL,
  version TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS mod_library_categories (
  key TEXT PRIMARY KEY,
  directory TEXT NOT NULL UNIQUE
);
CREATE TABLE IF NOT EXISTS mod_library_entries (
  id TEXT PRIMARY KEY,
  category_key TEXT NOT NULL REFERENCES mod_library_categories(key),
  game_name TEXT NOT NULL,
  game_name_key TEXT NOT NULL,
  -- Keep the binding even after unregistering a game (not an unbound candidate).
  game_id TEXT,
  name TEXT NOT NULL,
  filename TEXT NOT NULL,
  archive_relative_path TEXT NOT NULL UNIQUE,
  payload_relative_path TEXT NOT NULL UNIQUE,
  size INTEGER NOT NULL CHECK(size >= 0),
  sha256 TEXT NOT NULL,
  sha512 TEXT,
  source_id TEXT NOT NULL,
  project_id TEXT,
  version_id TEXT,
  version TEXT NOT NULL,
  integrity_verified INTEGER NOT NULL,
  project_page TEXT,
  author_hint TEXT,
  created_at TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN ('pending','ready')),
  CHECK((project_id IS NULL) = (version_id IS NULL))
);
CREATE INDEX IF NOT EXISTS idx_mod_library_game ON mod_library_entries(game_id,state);
CREATE INDEX IF NOT EXISTS idx_mod_library_game_name ON mod_library_entries(game_name_key,state) WHERE game_id IS NULL;

CREATE TABLE IF NOT EXISTS installs (
  id TEXT PRIMARY KEY,
  game_id TEXT NOT NULL,
  mod_id TEXT,
  name TEXT NOT NULL,
  version TEXT,
  source TEXT NOT NULL DEFAULT 'local',
  status TEXT NOT NULL DEFAULT 'enabled',
  install_root TEXT,
  files_json TEXT NOT NULL DEFAULT '[]',
  load_order INTEGER,
  dependencies_json TEXT NOT NULL DEFAULT '[]',
  metadata_json TEXT NOT NULL DEFAULT '{}',
  installed_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_installs_game ON installs(game_id);
-- List membership is separate from file ownership and rollback history.
CREATE TABLE IF NOT EXISTS removed_mod_items (
  game_id TEXT NOT NULL REFERENCES games(id) ON DELETE CASCADE,
  kind TEXT NOT NULL CHECK(kind IN ('install','archive')),
  item_id TEXT NOT NULL,
  PRIMARY KEY(game_id,kind,item_id)
);
CREATE TABLE IF NOT EXISTS install_transactions (
  install_id TEXT NOT NULL,
  transaction_id TEXT NOT NULL UNIQUE,
  action TEXT NOT NULL CHECK(action IN ('install','uninstall')),
  created_at TEXT NOT NULL,
  PRIMARY KEY(install_id, transaction_id)
);
CREATE TABLE IF NOT EXISTS install_replacements (
  transaction_id TEXT PRIMARY KEY REFERENCES file_transactions(id),
  replaced_install_id TEXT NOT NULL REFERENCES installs(id),
  replacement_install_id TEXT NOT NULL UNIQUE REFERENCES installs(id),
  CHECK(replaced_install_id <> replacement_install_id)
);
CREATE INDEX IF NOT EXISTS idx_install_replacements_previous ON install_replacements(replaced_install_id);
CREATE TABLE IF NOT EXISTS game_user_roots (
  game_id TEXT NOT NULL REFERENCES games(id),
  root_id TEXT NOT NULL,
  path TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(game_id, root_id)
);

CREATE TABLE IF NOT EXISTS file_transactions (
  id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  status TEXT NOT NULL,
  game_id TEXT,
  conversation_id TEXT,
  summary TEXT NOT NULL DEFAULT '',
  plan_json TEXT NOT NULL,
  journal_json TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL,
  finished_at TEXT,
  error TEXT
);

CREATE TABLE IF NOT EXISTS desktop_archive_imports (
  transaction_id TEXT PRIMARY KEY REFERENCES file_transactions(id) ON DELETE CASCADE,
  artifact_id TEXT NOT NULL UNIQUE,
  filename TEXT NOT NULL,
  size INTEGER NOT NULL,
  sha256 TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS file_transaction_entries (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  transaction_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  op TEXT NOT NULL,
  src TEXT,
  dst TEXT NOT NULL,
  backup_path TEXT,
  result TEXT NOT NULL DEFAULT 'pending',
  error TEXT,
  FOREIGN KEY(transaction_id) REFERENCES file_transactions(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ftx_entries ON file_transaction_entries(transaction_id, seq);

CREATE TABLE IF NOT EXISTS file_transaction_journal (
  transaction_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  entry_json TEXT NOT NULL,
  PRIMARY KEY (transaction_id, seq),
  FOREIGN KEY(transaction_id) REFERENCES file_transactions(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS trash_items (
  id TEXT PRIMARY KEY,
  original_path TEXT NOT NULL,
  trash_path TEXT NOT NULL,
  size INTEGER NOT NULL DEFAULT 0,
  is_dir INTEGER NOT NULL DEFAULT 0,
  game_id TEXT,
  transaction_id TEXT,
  label TEXT NOT NULL DEFAULT '',
  reason TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL,
  restored_at TEXT
);
CREATE INDEX IF NOT EXISTS idx_trash_created ON trash_items(created_at DESC);
CREATE TABLE IF NOT EXISTS trash_restore_links (
  item_id TEXT NOT NULL,
  transaction_id TEXT NOT NULL,
  PRIMARY KEY (item_id, transaction_id)
);
CREATE TABLE IF NOT EXISTS trash_purges (
  item_id TEXT PRIMARY KEY,
  status TEXT NOT NULL,
  confirmed_at TEXT NOT NULL,
  finished_at TEXT
);

CREATE TABLE IF NOT EXISTS snapshots (
  id TEXT PRIMARY KEY,
  game_id TEXT NOT NULL,
  label TEXT NOT NULL,
  root_path TEXT NOT NULL,
  path TEXT NOT NULL,
  file_count INTEGER NOT NULL DEFAULT 0,
  total_size INTEGER NOT NULL DEFAULT 0,
  kind TEXT NOT NULL DEFAULT 'manual',
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_snapshots_game ON snapshots(game_id, created_at DESC);
CREATE TABLE IF NOT EXISTS snapshot_transactions (
  snapshot_id TEXT PRIMARY KEY,
  transaction_id TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS plugins (
  slug TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  game_name TEXT NOT NULL,
  version TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  source TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  validated INTEGER NOT NULL DEFAULT 1,
  install_path TEXT NOT NULL,
  adapter_json TEXT NOT NULL,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS skills (
  name TEXT PRIMARY KEY,
  description TEXT NOT NULL,
  location TEXT NOT NULL,
  source TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS plugin_drafts (
  id TEXT PRIMARY KEY,
  slug TEXT NOT NULL,
  path TEXT NOT NULL,
  write_transaction_id TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS agent_plugin_drafts (
  draft_id TEXT PRIMARY KEY REFERENCES plugin_drafts(id)
);
CREATE TABLE IF NOT EXISTS plugin_draft_deletions (
  draft_id TEXT NOT NULL REFERENCES plugin_drafts(id),
  transaction_id TEXT PRIMARY KEY REFERENCES file_transactions(id),
  trash_item_id TEXT NOT NULL UNIQUE REFERENCES trash_items(id)
);
CREATE INDEX IF NOT EXISTS idx_plugin_draft_deletions_draft ON plugin_draft_deletions(draft_id);
-- A failed plan does not delete the draft. A committed restore, or rollback of
-- the deletion itself, makes the original ID available again.
CREATE VIEW IF NOT EXISTS unavailable_plugin_drafts AS
  SELECT d.draft_id FROM plugin_draft_deletions d
  JOIN file_transactions f ON f.id=d.transaction_id
  WHERE f.status IN ('committed','applying','rolling_back','recovery_failed')
    AND NOT EXISTS (
      SELECT 1 FROM trash_restore_links l
      JOIN file_transactions r ON r.id=l.transaction_id
      WHERE l.item_id=d.trash_item_id AND r.status='committed'
    );
CREATE TABLE IF NOT EXISTS plugin_deployments (
  transaction_id TEXT PRIMARY KEY,
  slug TEXT NOT NULL,
  package_json TEXT NOT NULL,
  install_path TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS mcp_servers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  description TEXT,
  transport TEXT NOT NULL,
  command TEXT,
  args_json TEXT NOT NULL DEFAULT '[]',
  url TEXT,
  credential_url TEXT,
  env_json TEXT NOT NULL DEFAULT '{}',
  secret_ref TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  builtin INTEGER NOT NULL DEFAULT 0,
  startup_digest BLOB,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL DEFAULT '新对话',
  auto_title_pending INTEGER NOT NULL DEFAULT 0,
  game_id TEXT,
  task_summary TEXT,
  context_summary_through_message_id TEXT,
  archived INTEGER NOT NULL DEFAULT 0,
  pinned INTEGER NOT NULL DEFAULT 0,
  message_count INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_conv_updated ON conversations(archived, updated_at DESC);

CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  role TEXT NOT NULL,
  content TEXT NOT NULL DEFAULT '',
  reasoning TEXT,
  attachments_json TEXT,
  tool_calls_json TEXT,
  tool_call_id TEXT,
  name TEXT,
  token_count INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  FOREIGN KEY(conversation_id) REFERENCES conversations(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_messages_conv ON messages(conversation_id, created_at);

CREATE TABLE IF NOT EXISTS tool_calls (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  message_id TEXT,
  name TEXT NOT NULL,
  arguments_json TEXT NOT NULL DEFAULT '{}',
  result_json TEXT,
  status TEXT NOT NULL DEFAULT 'pending',
  approval_status TEXT NOT NULL DEFAULT 'none',
  started_at TEXT NOT NULL,
  finished_at TEXT,
  duration_ms INTEGER,
  error TEXT
);
CREATE INDEX IF NOT EXISTS idx_tool_calls_conv ON tool_calls(conversation_id, started_at);

CREATE TABLE IF NOT EXISTS approvals (
  id TEXT PRIMARY KEY,
  conversation_id TEXT,
  tool_call_id TEXT,
  kind TEXT NOT NULL,
  title TEXT NOT NULL,
  detail TEXT NOT NULL DEFAULT '',
  payload_json TEXT NOT NULL DEFAULT '{}',
  status TEXT NOT NULL DEFAULT 'pending',
  created_at TEXT NOT NULL,
  resolved_at TEXT
);

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS secrets_ref (
  key TEXT PRIMARY KEY,
  service TEXT NOT NULL,
  account TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_turns (
  id TEXT PRIMARY KEY,
  conversation_id TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL,
  finished_at TEXT,
  error TEXT
);
CREATE TABLE IF NOT EXISTS agent_events (
  seq INTEGER PRIMARY KEY AUTOINCREMENT,
  turn_id TEXT NOT NULL,
  conversation_id TEXT NOT NULL,
  message_id TEXT,
  kind TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_events_conv ON agent_events(conversation_id,seq);
"#;
