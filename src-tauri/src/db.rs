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
        let c = self.conn();
        c.execute_batch(SCHEMA)?;
        // lightweight forward migrations
        let version: i64 = c.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < SCHEMA_VERSION {
            c.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        }
        drop(c);
        crate::skills::migrate(self)?;
        crate::plugin_publish::migrate(&self.conn())?;
        crate::game_identity::migrate(&self.conn())?;
        Ok(())
    }
}

pub const SCHEMA_VERSION: i64 = 1;

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
  source TEXT NOT NULL DEFAULT 'builtin',
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
  source TEXT NOT NULL DEFAULT 'builtin',
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
  transport TEXT NOT NULL,
  command TEXT,
  args_json TEXT NOT NULL DEFAULT '[]',
  url TEXT,
  env_json TEXT NOT NULL DEFAULT '{}',
  secret_ref TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  builtin INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL DEFAULT '新对话',
  game_id TEXT,
  task_summary TEXT,
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
