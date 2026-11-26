use super::{
    from_row, McpServers, McpTransport, ServerConfig, GLOSSMOD_3DM_ENDPOINT,
    GLOSSMOD_HTTP_ENDPOINT, OWNER_ACTIVE, SERVER_COLUMNS,
};
use crate::{
    error::{AppError, AppResult},
    mcp_connections::LOCAL_SERVER_ID,
};
use rusqlite::{
    params, Connection, OptionalExtension, Params, Row, Transaction, TransactionBehavior,
};
use sha2::{Digest, Sha256};

const GAME_BOUND: &str =
    "EXISTS (SELECT 1 FROM games WHERE games.adapter_slug=mcp_servers.plugin_slug)";

#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
enum Decision {
    Policy = 1,
    Allow = 2,
    Block = 3,
}

pub(super) struct ServerState {
    pub(super) config: ServerConfig,
    pub(super) manual_digest: Option<Vec<u8>>,
    owner_active: bool,
    game_bound: bool,
    decision: Option<i64>,
    game_digest: Option<Vec<u8>>,
}

impl ServerState {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        Ok(Self {
            config: from_row(row)?,
            owner_active: row.get(13)?,
            game_bound: row.get(14)?,
            manual_digest: row.get(15)?,
            decision: row.get(16)?,
            game_digest: row.get(17)?,
        })
    }

    fn eligible(&self) -> bool {
        self.owner_active
            && self.game_bound
            && !self.config.builtin
            && self.config.id != LOCAL_SERVER_ID
    }

    /// The game decision includes all connection settings including the credential page.
    /// Effective enablement and display-only descriptions can change independently.
    fn exact_digest(&self) -> AppResult<[u8; 32]> {
        exact_digest(&self.config)
    }

    fn exact_decision(&self) -> AppResult<Option<i64>> {
        if self.game_digest.is_none()
            || self.game_digest.as_deref() != Some(&self.exact_digest()?[..])
        {
            return Ok(None);
        }
        if self.decision == Some(Decision::Policy as i64) && !safe_by_policy(&self.config) {
            return Ok(None);
        }
        Ok(self.decision)
    }

    pub(super) fn has_manual_start(&self) -> AppResult<bool> {
        Ok(self.owner_active
            && self.config.enabled
            && self.manual_digest.is_some()
            && self.manual_digest.as_deref() == Some(&self.config.confirmation_digest()?[..]))
    }

    pub(super) fn has_game_start(&self) -> AppResult<bool> {
        if !self.eligible() {
            return Ok(false);
        }
        let decision = self.exact_decision()?;
        Ok(decision == Some(Decision::Allow as i64) || decision == Some(Decision::Policy as i64))
    }

    pub(super) fn effective_config(mut self) -> AppResult<ServerConfig> {
        self.config.enabled =
            (self.config.enabled && self.owner_active) || self.has_game_start()?;
        Ok(self.config)
    }
}

fn select(suffix: &str) -> String {
    format!(
        "SELECT {SERVER_COLUMNS}, {OWNER_ACTIVE}, {GAME_BOUND},
                startup_digest, game_tool_state.decision, game_tool_state.digest
         FROM mcp_servers LEFT JOIN game_tool_state ON server_id=mcp_servers.id {suffix}"
    )
}

pub(super) fn find(conn: &Connection, id: &str) -> AppResult<ServerState> {
    conn.query_row(&select("WHERE id=?1"), [id], ServerState::from_row)
        .optional()?
        .ok_or_else(|| AppError::NotFound("MCP server".into()))
}

pub(super) fn states(
    conn: &Connection,
    suffix: &str,
    params: impl Params,
) -> AppResult<Vec<ServerState>> {
    Ok(conn
        .prepare(&select(suffix))?
        .query_map(params, ServerState::from_row)?
        .collect::<Result<_, _>>()?)
}

fn game_candidates(conn: &Connection) -> AppResult<Vec<ServerState>> {
    states(
        conn,
        &format!("WHERE {GAME_BOUND} AND {OWNER_ACTIVE} AND builtin=0 AND id<>?1 ORDER BY id"),
        [LOCAL_SERVER_ID],
    )
}

fn exact_digest(config: &ServerConfig) -> AppResult<[u8; 32]> {
    let mut settings = config.clone();
    settings.enabled = false;
    settings.description = None;
    Ok(Sha256::digest(serde_json::to_vec(&settings)?).into())
}

fn safe_by_policy(config: &ServerConfig) -> bool {
    if !config.env_secret_refs.is_empty()
        || config.bearer_secret_ref.is_some()
        || config.credential_url.is_some()
    {
        return false;
    }
    match config.transport {
        McpTransport::Stdio => config.is_gta_rpf_launcher(),
        McpTransport::StreamableHttp => matches!(
            config.url.as_deref(),
            Some(GLOSSMOD_HTTP_ENDPOINT | GLOSSMOD_3DM_ENDPOINT)
        ),
    }
}

fn store_decision(tx: &Transaction<'_>, state: &ServerState, decision: Decision) -> AppResult<()> {
    tx.execute(
        "INSERT INTO game_tool_state(server_id, digest, decision) VALUES (?1, ?2, ?3)
         ON CONFLICT(server_id) DO UPDATE SET digest=excluded.digest,decision=excluded.decision
         WHERE game_tool_state.digest<>excluded.digest OR game_tool_state.decision<>excluded.decision",
        params![state.config.id, state.exact_digest()?, decision as i64],
    )?;
    Ok(())
}

pub(super) fn record_stop(tx: &Transaction<'_>, id: &str) -> AppResult<()> {
    let state = tx
        .query_row(
            &select("WHERE id=?1 AND plugin_slug IS NOT NULL"),
            [id],
            ServerState::from_row,
        )
        .optional()?;
    if let Some(state) = state {
        store_decision(tx, &state, Decision::Block)?;
    }
    Ok(())
}

pub(super) fn revoke_bound_plugin(tx: &Transaction<'_>, slug: &str) -> AppResult<bool> {
    let managed = tx.query_row(
        &format!(
            "SELECT EXISTS(SELECT 1 FROM mcp_servers LEFT JOIN game_tool_state ON server_id=id
                  WHERE plugin_slug=?1 AND ({GAME_BOUND} OR game_tool_state.decision IS NOT NULL))"
        ),
        [slug],
        |row| row.get(0),
    )?;
    if managed {
        tx.execute(
            "DELETE FROM game_tool_state WHERE decision=?2
             AND server_id IN (SELECT id FROM mcp_servers WHERE plugin_slug=?1)",
            params![slug, Decision::Policy as i64],
        )?;
    }
    Ok(managed)
}

impl McpServers {
    /// Materialize safe policy decisions for the current set of game bindings.
    pub(crate) fn sync_game_tools(&self) -> AppResult<()> {
        let mut conn = self.db.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute(
            &format!(
                "DELETE FROM game_tool_state WHERE decision=?1 AND server_id NOT IN (
                    SELECT id FROM mcp_servers WHERE {GAME_BOUND} AND {OWNER_ACTIVE} AND builtin=0 AND id<>?2
                )"
            ),
            params![Decision::Policy as i64, LOCAL_SERVER_ID],
        )?;
        for state in game_candidates(&tx)? {
            state.config.validate()?;
            if safe_by_policy(&state.config) && state.exact_decision()?.is_none() {
                store_decision(&tx, &state, Decision::Policy)?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Return enabled game-bound configurations with valid launch intent.
    pub(crate) fn game_tool_requests(&self) -> AppResult<Vec<ServerConfig>> {
        let mut requests = Vec::new();
        for state in game_candidates(&self.db.conn())? {
            if state.has_manual_start()? || state.has_game_start()? {
                requests.push(state.effective_config()?);
            }
        }
        Ok(requests)
    }

    /// Return the current snapshots that require desktop consent, ordered by server ID.
    pub(crate) fn pending_game_tools(&self) -> AppResult<Vec<ServerConfig>> {
        let mut pending = Vec::new();
        for state in game_candidates(&self.db.conn())? {
            if !state.has_manual_start()?
                && state.exact_decision()?.is_none()
                && !safe_by_policy(&state.config)
            {
                state.config.validate()?;
                pending.push(state.config);
            }
        }
        Ok(pending)
    }

    /// Commit a decision against the exact snapshot returned by pending_game_tools.
    pub(crate) fn set_game_tool_consent(
        &self,
        expected: &ServerConfig,
        allowed: bool,
    ) -> AppResult<()> {
        let mut conn = self.db.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let state = find(&tx, &expected.id)?;
        let stale = || AppError::Conflict("配置已变化，请重新确认完整连接配置".into());
        if !state.eligible()
            || state.exact_digest()? != exact_digest(expected)?
            || state.has_manual_start()?
        {
            return Err(stale());
        }
        state.config.validate()?;
        let decision = if allowed {
            Decision::Allow
        } else {
            Decision::Block
        };
        match state.exact_decision()? {
            Some(current) if current == decision as i64 => return Ok(()),
            None if state.config.same_connection_config(expected)
                && !safe_by_policy(&state.config) => {}
            _ => return Err(stale()),
        }
        store_decision(&tx, &state, decision)?;
        tx.commit()?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "game_tools_tests.rs"]
mod tests;
