//! Durable, consent-bound publication of manager plugins (not ordinary works).
use crate::{
    error::{AppError, AppResult},
    plugin_publish::{self as provenance, PublicationFile, PublicationSource},
    plugins,
    settings::Settings,
    transaction::FileEngine,
    util,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use tokio_util::sync::CancellationToken;

#[path = "manager_publication_remote.rs"]
mod remote;
#[cfg(test)]
#[path = "manager_publication_tests.rs"]
mod tests;

pub use crate::plugin_publish_remote::ReconciliationOutcome;

const MAX_PREVIEW_BYTES: usize = 3 * 1024 * 1024;
const PAGE_LIMIT: u32 = 100;
const RECONCILE_PAGES: u32 = 5;
const MAX_SAFE_ID: i64 = 9_007_199_254_740_991;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PublicationTarget {
    First,
    Release { plugin_id: i64 },
}

impl PublicationTarget {
    fn plugin_id(self) -> Option<i64> {
        match self {
            Self::First => None,
            Self::Release { plugin_id } => Some(plugin_id),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationPreview {
    pub source: PublicationSource,
    pub target: PublicationTarget,
    pub slug: String,
    pub version: String,
    pub tags: Vec<String>,
    pub files: [PublicationFile; 2],
    pub destination: String,
    pub digest: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PublicationState {
    Intended,
    Pending,
    Unknown,
    Completed,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PublishedPlugin {
    pub id: i64,
    pub release_id: i64,
    pub slug: String,
    pub version: String,
    pub tags: Vec<String>,
    pub hidden: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OwnPluginTexts {
    pub adapter: String,
    pub skill: String,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicationRecord {
    pub id: String,
    pub preview: PublicationPreview,
    pub approved_digest: Option<String>,
    pub state: PublicationState,
    pub remote: Option<PublishedPlugin>,
    pub created_at: String,
    pub updated_at: String,
}

pub fn prepare_draft(
    engine: &FileEngine,
    draft_id: &str,
    target: PublicationTarget,
    tags: Vec<String>,
) -> AppResult<PublicationRecord> {
    provenance::with_draft_source(engine, draft_id, |source, slug, files| {
        create(engine, preview(source, slug, files, target, tags)?)
    })
}

pub fn prepare_installed(
    engine: &FileEngine,
    slug: &str,
    target: PublicationTarget,
    tags: Vec<String>,
) -> AppResult<PublicationRecord> {
    provenance::with_installed_source(engine, slug, |source, slug, files| {
        create(engine, preview(source, slug, files, target, tags)?)
    })
}

fn valid_id(id: i64) -> bool {
    (1..=MAX_SAFE_ID).contains(&id)
}

fn tags_set(tags: &[String]) -> Option<BTreeSet<&str>> {
    let mut set = BTreeSet::new();
    // Resource bounds, not the old ordinary-work tag constraints. Do not trim,
    // truncate, or otherwise change what the user approved on the wire.
    (tags.len() <= 256
        && tags.iter().all(|tag| {
            !tag.is_empty()
                && tag.len() <= 8192
                && !tag.chars().any(char::is_control)
                && tag.trim() == tag
                && set.insert(tag.as_str())
        }))
    .then_some(set)
}

fn same_tags(a: &[String], b: &[String]) -> bool {
    tags_set(a).is_some_and(|a| tags_set(b).is_some_and(|b| a == b))
}

fn invalid(message: &str) -> AppError {
    AppError::Invalid(message.into())
}

fn stale() -> AppError {
    AppError::Conflict("插件发布记录、批准或原始文件已更改，请重新检查".into())
}

fn unavailable() -> AppError {
    AppError::Other("无法核实 Mod 客栈管理器插件响应".into())
}

fn preview(
    source: PublicationSource,
    slug: &str,
    files: [PublicationFile; 2],
    target: PublicationTarget,
    tags: Vec<String>,
) -> AppResult<PublicationPreview> {
    if target.plugin_id().is_some_and(|id| !valid_id(id)) || tags_set(&tags).is_none() {
        return Err(invalid("管理器插件 ID 或标签无效"));
    }
    if tags.iter().map(String::len).sum::<usize>() > 64 * 1024 {
        return Err(invalid("管理器插件标签总大小超过限制"));
    }
    for ((file, name), limit) in files
        .iter()
        .zip(["adapter.json", "SKILL.md"])
        .zip([provenance::ADAPTER_LIMIT, provenance::SKILL_LIMIT])
    {
        if file.name != name
            || file.size != file.text.len() as u64
            || file.text.len() > limit
            || file.sha256 != hex::encode(Sha256::digest(file.text.as_bytes()))
        {
            return Err(stale());
        }
    }
    let version = plugins::parse_package(slug, &files[0].text, &files[1].text)?
        .adapter
        .version;
    let destination = provenance::DESTINATION.to_owned();
    let mut hash = Sha256::new();
    hash.update(b"amm.manager-plugin-publication.v1\0");
    hash.update(serde_json::to_vec(&(
        &source,
        target,
        slug,
        &version,
        &tags,
        &files,
        &destination,
    ))?);
    Ok(PublicationPreview {
        source,
        target,
        slug: slug.into(),
        version,
        tags,
        files,
        destination,
        digest: hex::encode(hash.finalize()),
    })
}

fn with_current_source<T>(
    engine: &FileEngine,
    observed: &PublicationRecord,
    f: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    let check = |source, slug: &str, files| {
        let fresh = preview(
            source,
            slug,
            files,
            observed.preview.target,
            observed.preview.tags.clone(),
        )?;
        if fresh != observed.preview {
            return Err(stale());
        }
        f()
    };
    match &observed.preview.source {
        PublicationSource::Draft { draft_id, .. } => {
            provenance::with_draft_source(engine, draft_id, check)
        }
        PublicationSource::Installed { slug, .. } => {
            provenance::with_installed_source(engine, slug, check)
        }
    }
}

pub fn migrate(conn: &Connection) -> AppResult<()> {
    // Legacy works keep their separate records. Neither old IDs nor approval
    // digests can authorize this new target, even for the very same bytes.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS manager_publications (
        id TEXT PRIMARY KEY, preview_json TEXT NOT NULL, digest TEXT NOT NULL,
        slug TEXT NOT NULL, version TEXT NOT NULL, target_plugin_id INTEGER,
        approved_digest TEXT, remote_json TEXT,
        state TEXT NOT NULL CHECK(state IN ('intended','pending','unknown','completed','error')),
        created_at TEXT NOT NULL, updated_at TEXT NOT NULL
    );
    CREATE INDEX IF NOT EXISTS manager_publications_digest ON manager_publications(digest);
    CREATE INDEX IF NOT EXISTS manager_publications_slug ON manager_publications(slug,state);",
    )?;
    Ok(())
}

pub fn recover_pending_publications(engine: &FileEngine) -> AppResult<usize> {
    Ok(engine.db.conn().execute(
        "UPDATE manager_publications SET state='unknown',updated_at=?1 WHERE state='pending'",
        [util::now()],
    )?)
}

fn load(conn: &Connection, id: &str) -> AppResult<PublicationRecord> {
    if !id.strip_prefix("manager_publish_").is_some_and(|suffix| {
        suffix.len() == 32
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    }) {
        return Err(invalid("管理器插件发布记录 ID 无效"));
    }
    let (
        raw,
        digest,
        slug,
        version,
        target_id,
        approved_digest,
        state,
        remote,
        created_at,
        updated_at,
    ) = conn
        .query_row(
            "SELECT CASE WHEN length(CAST(preview_json AS BLOB))<=?2 THEN preview_json ELSE '' END,
                digest,slug,version,target_plugin_id,approved_digest,state,
                CASE WHEN length(CAST(remote_json AS BLOB))<=524288 THEN remote_json
                     WHEN remote_json IS NULL THEN NULL ELSE '' END,created_at,updated_at
           FROM manager_publications WHERE id=?1",
            params![id, MAX_PREVIEW_BYTES],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, Option<i64>>(4)?,
                    r.get::<_, Option<String>>(5)?,
                    r.get::<_, String>(6)?,
                    r.get::<_, Option<String>>(7)?,
                    r.get(8)?,
                    r.get(9)?,
                ))
            },
        )
        .optional()?
        .ok_or_else(|| AppError::NotFound("管理器插件发布记录".into()))?;
    let stored: PublicationPreview = serde_json::from_str(&raw).map_err(|_| stale())?;
    let rebuilt = preview(
        stored.source.clone(),
        &stored.slug,
        stored.files.clone(),
        stored.target,
        stored.tags.clone(),
    )?;
    let state: PublicationState =
        serde_json::from_value(Value::String(state)).map_err(|_| stale())?;
    let remote: Option<PublishedPlugin> = remote
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(|_| stale())?;
    if stored != rebuilt
        || stored.digest != digest
        || stored.slug != slug
        || stored.version != version
        || stored.target.plugin_id() != target_id
        || approved_digest
            .as_ref()
            .is_some_and(|value| value != &digest)
        || (state != PublicationState::Intended && approved_digest.is_none())
        || (state == PublicationState::Completed) != remote.is_some()
        || remote
            .as_ref()
            .is_some_and(|p| !matches_preview(p, &stored))
    {
        return Err(stale());
    }
    Ok(PublicationRecord {
        id: id.into(),
        preview: stored,
        approved_digest,
        state,
        remote,
        created_at,
        updated_at,
    })
}

fn create(engine: &FileEngine, preview: PublicationPreview) -> AppResult<PublicationRecord> {
    let raw = serde_json::to_string(&preview)?;
    if raw.len() > MAX_PREVIEW_BYTES {
        return Err(invalid("管理器插件发布预览超过大小限制"));
    }
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let existing: Option<String> = tx.query_row(
        "SELECT id FROM manager_publications WHERE digest=?1 AND state!='error' ORDER BY rowid DESC LIMIT 1",
        [&preview.digest], |r| r.get(0),
    ).optional()?;
    let id = if let Some(existing) = existing {
        existing
    } else {
        let id = util::new_id("manager_publish");
        tx.execute("INSERT INTO manager_publications(id,preview_json,digest,slug,version,target_plugin_id,state,created_at,updated_at)
            VALUES(?1,?2,?3,?4,?5,?6,'intended',?7,?7)", params![id, raw, preview.digest, preview.slug, preview.version, preview.target.plugin_id(), util::now()])?;
        id
    };
    let record = load(&tx, &id)?;
    if record.preview != preview {
        return Err(stale());
    }
    tx.commit()?;
    Ok(record)
}

pub fn get_publication(engine: &FileEngine, id: &str) -> AppResult<PublicationRecord> {
    load(&engine.db.conn(), id)
}

pub fn list_publications(engine: &FileEngine) -> AppResult<Vec<PublicationRecord>> {
    let conn = engine.db.conn();
    let mut stmt =
        conn.prepare("SELECT id FROM manager_publications ORDER BY rowid DESC LIMIT 50")?;
    let ids = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    ids.iter().map(|id| load(&conn, id)).collect()
}

/// For the trusted desktop/Agent approval broker only; never expose self-approval.
pub fn approve_publication(
    engine: &FileEngine,
    id: &str,
    digest: &str,
) -> AppResult<PublicationRecord> {
    let record = get_publication(engine, id)?;
    if record.state != PublicationState::Intended || digest != record.preview.digest {
        return Err(stale());
    }
    with_current_source(engine, &record, || {
        let mut conn = engine.db.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load(&tx, id)? != record {
            return Err(stale());
        }
        if record.approved_digest.is_none() {
            tx.execute(
                "UPDATE manager_publications SET approved_digest=?2,updated_at=?3 WHERE id=?1",
                params![id, digest, util::now()],
            )?;
        }
        let record = load(&tx, id)?;
        tx.commit()?;
        Ok(record)
    })
}

fn claim(engine: &FileEngine, record: &PublicationRecord) -> AppResult<PublicationRecord> {
    with_current_source(engine, record, || {
        let mut conn = engine.db.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load(&tx, &record.id)? != *record {
            return Err(stale());
        }
        // No differently-tagged preview, alternate source, or First/Release
        // switch may resend the same ambiguous release. An uncertain first
        // publication also blocks a second First attempt at any version.
        let blocked: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM manager_publications
            WHERE id<>?1 AND slug=?2 AND (
                (state IN ('pending','unknown') AND (version=?3 OR target_plugin_id IS NULL OR ?4 IS NULL OR target_plugin_id=?4))
                OR (state='completed' AND (version=?3 OR ?4 IS NULL))
            ))",
            params![
                record.id,
                record.preview.slug,
                record.preview.version,
                record.preview.target.plugin_id()
            ],
            |r| r.get(0),
        )?;
        if blocked {
            return Err(AppError::Conflict(
                "该插件已有已提交或结果待核对的发布记录，不能重复发送".into(),
            ));
        }
        tx.execute(
            "UPDATE manager_publications SET state='pending',updated_at=?2 WHERE id=?1",
            params![record.id, util::now()],
        )?;
        let pending = load(&tx, &record.id)?;
        tx.commit()?;
        Ok(pending)
    })
}

fn transition(
    engine: &FileEngine,
    observed: &PublicationRecord,
    state: &str,
    remote: Option<&PublishedPlugin>,
) -> AppResult<PublicationRecord> {
    let mut conn = engine.db.conn();
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    if load(&tx, &observed.id)? != *observed {
        return Err(stale());
    }
    tx.execute(
        "UPDATE manager_publications SET state=?2,remote_json=?3,updated_at=?4 WHERE id=?1",
        params![
            observed.id,
            state,
            remote.map(serde_json::to_string).transpose()?,
            util::now()
        ],
    )?;
    let record = load(&tx, &observed.id)?;
    tx.commit()?;
    Ok(record)
}

struct UnknownOnDrop<'a> {
    engine: &'a FileEngine,
    pending: &'a PublicationRecord,
    armed: bool,
}

impl Drop for UnknownOnDrop<'_> {
    fn drop(&mut self) {
        if self.armed {
            // Startup recovery is the fallback if this write fails or the process exits.
            let _ = self.engine.db.conn().execute(
                "UPDATE manager_publications SET state='unknown',updated_at=?2 WHERE id=?1 AND state='pending'",
                params![self.pending.id, util::now()],
            );
        }
    }
}

#[async_trait::async_trait]
pub(crate) trait Remote: Send + Sync {
    /// The returned value is the tool's parsed data, not the MCP wrapper.
    /// `Denied` is reserved for a definitive pre-mutation authorization rejection.
    async fn call(&self, name: &str, args: Value, cancel: &CancellationToken) -> AppResult<Value>;
}

fn valid_plugin(plugin: &PublishedPlugin) -> bool {
    valid_id(plugin.id)
        && valid_id(plugin.release_id)
        && plugins::validate_slug(&plugin.slug).is_ok()
        && plugins::validate_version(&plugin.version).is_ok()
        && tags_set(&plugin.tags).is_some()
        && plugin.tags.iter().map(String::len).sum::<usize>() <= 64 * 1024
        && plugin.name.len() <= 8192
        && plugin.reason.len() <= 64 * 1024
}

fn matches_preview(plugin: &PublishedPlugin, preview: &PublicationPreview) -> bool {
    valid_plugin(plugin)
        && plugin.slug == preview.slug
        && plugin.version == preview.version
        && preview.target.plugin_id().is_none_or(|id| id == plugin.id)
        && same_tags(&plugin.tags, &preview.tags)
}

fn approved(engine: &FileEngine, id: &str, digest: &str) -> AppResult<PublicationRecord> {
    let record = get_publication(engine, id)?;
    if record.approved_digest.as_deref() != Some(digest) {
        return Err(stale());
    }
    match record.state {
        PublicationState::Intended | PublicationState::Completed => Ok(record),
        _ => Err(AppError::Conflict(
            "发布正在进行、结果待核对或已结束，不能重复发送".into(),
        )),
    }
}

pub async fn publish_approved(
    engine: &FileEngine,
    settings: &Settings,
    id: &str,
    digest: &str,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let record = approved(engine, id, digest)?;
    if record.state == PublicationState::Completed {
        return Ok(record);
    }
    // Detect changed sources before credential access as well as after the handshake.
    with_current_source(engine, &record, || Ok(()))?;
    let remote = remote::Transport::connect(settings, cancel).await?;
    publish_with(engine, &remote, id, digest, cancel).await
}

pub(crate) async fn publish_with(
    engine: &FileEngine,
    remote: &dyn Remote,
    id: &str,
    digest: &str,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    let record = approved(engine, id, digest)?;
    if record.state == PublicationState::Completed {
        return Ok(record);
    }
    let p = &record.preview;
    let mut args = json!({"adapter":p.files[0].text,"skill":p.files[1].text,"tags":p.tags});
    let tool = match p.target {
        PublicationTarget::First => "publish_manager_plugin",
        PublicationTarget::Release { plugin_id } => {
            args["plugin_id"] = json!(plugin_id);
            "publish_manager_plugin_release"
        }
    };
    let pending = claim(engine, &record)?;
    let mut guard = UnknownOnDrop {
        engine,
        pending: &pending,
        armed: true,
    };
    let response = remote.call(tool, args, cancel).await;
    #[derive(Deserialize)]
    struct Receipt {
        plugin: PublishedPlugin,
    }
    let result = match response {
        Ok(raw) => match serde_json::from_value::<Receipt>(raw) {
            Ok(receipt) if matches_preview(&receipt.plugin, p) => {
                transition(engine, &pending, "completed", Some(&receipt.plugin))
            }
            _ => transition(engine, &pending, "unknown", None),
        },
        Err(error @ AppError::Denied(_)) => {
            transition(engine, &pending, "error", None)?;
            Err(error)
        }
        // Timeouts, session/server failures, cancellation, and unverified
        // responses are never retried. Return a durable record for UI/Agent.
        Err(_) => transition(engine, &pending, "unknown", None),
    };
    if result.is_ok() || matches!(result, Err(AppError::Denied(_))) {
        guard.armed = false;
    }
    result
}

async fn own_page(
    remote: &dyn Remote,
    limit: u32,
    offset: u32,
    cancel: &CancellationToken,
) -> AppResult<Vec<PublishedPlugin>> {
    #[derive(Deserialize)]
    struct Page {
        items: Vec<PublishedPlugin>,
    }
    let page: Page = serde_json::from_value(
        remote
            .call(
                "list_my_manager_plugins",
                json!({"limit":limit,"offset":offset}),
                cancel,
            )
            .await?,
    )
    .map_err(|_| unavailable())?;
    let mut ids = BTreeSet::new();
    if page.items.len() > limit as usize
        || page
            .items
            .iter()
            .any(|p| !valid_plugin(p) || !ids.insert(p.id))
    {
        return Err(unavailable());
    }
    Ok(page.items)
}

async fn own_texts(
    remote: &dyn Remote,
    plugin_id: i64,
    cancel: &CancellationToken,
) -> AppResult<OwnPluginTexts> {
    let texts: OwnPluginTexts = serde_json::from_value(
        remote
            .call(
                "get_my_manager_plugin_texts",
                json!({"plugin_id":plugin_id}),
                cancel,
            )
            .await?,
    )
    .map_err(|_| unavailable())?;
    if texts.adapter.len() > provenance::ADAPTER_LIMIT
        || texts.skill.len() > provenance::SKILL_LIMIT
        || tags_set(&texts.tags).is_none()
    {
        return Err(unavailable());
    }
    Ok(texts)
}

pub async fn list_own_plugins(
    settings: &Settings,
    limit: u32,
    offset: u32,
    cancel: &CancellationToken,
) -> AppResult<Vec<PublishedPlugin>> {
    if !(1..=PAGE_LIMIT).contains(&limit) {
        return Err(invalid("每页可查询 1–100 个插件"));
    }
    own_page(
        &remote::Transport::connect(settings, cancel).await?,
        limit,
        offset,
        cancel,
    )
    .await
}

pub async fn get_own_plugin_texts(
    settings: &Settings,
    plugin_id: i64,
    cancel: &CancellationToken,
) -> AppResult<OwnPluginTexts> {
    if !valid_id(plugin_id) {
        return Err(invalid("管理器插件 ID 无效"));
    }
    own_texts(
        &remote::Transport::connect(settings, cancel).await?,
        plugin_id,
        cancel,
    )
    .await
}

pub async fn reconcile_publication(
    engine: &FileEngine,
    settings: &Settings,
    id: &str,
    cancel: &CancellationToken,
) -> AppResult<ReconciliationOutcome> {
    let record = get_publication(engine, id)?;
    match record.state {
        PublicationState::Completed => return Ok(ReconciliationOutcome::Verified),
        PublicationState::Unknown => {}
        _ => return Ok(ReconciliationOutcome::Unconfirmed),
    }
    let transport = remote::Transport::connect(settings, cancel).await?;
    reconcile_with(engine, &transport, record, cancel).await
}

async fn reconcile_with(
    engine: &FileEngine,
    remote: &dyn Remote,
    record: PublicationRecord,
    cancel: &CancellationToken,
) -> AppResult<ReconciliationOutcome> {
    use ReconciliationOutcome::{Unconfirmed, Verified};
    if record.state != PublicationState::Unknown {
        return Ok(Unconfirmed);
    }
    let p = &record.preview;
    let mut candidate = None;
    let mut seen = BTreeSet::new();
    let mut exhausted = false;
    for offset in (0..RECONCILE_PAGES).map(|page| page * PAGE_LIMIT) {
        let items = own_page(remote, PAGE_LIMIT, offset, cancel).await?;
        exhausted = items.len() < PAGE_LIMIT as usize;
        for row in items {
            if !seen.insert(row.id) {
                return Ok(Unconfirmed);
            }
            let target = p
                .target
                .plugin_id()
                .map_or(row.slug == p.slug, |id| id == row.id);
            if target {
                if !matches_preview(&row, p) || candidate.is_some() {
                    return Ok(Unconfirmed);
                }
                candidate = Some((row, offset));
            }
        }
        if exhausted {
            break;
        }
    }
    let Some((expected, offset)) = candidate.filter(|_| exhausted) else {
        return Ok(Unconfirmed);
    };
    let texts = own_texts(remote, expected.id, cancel).await?;
    if texts.adapter != p.files[0].text
        || texts.skill != p.files[1].text
        || !same_tags(&texts.tags, &p.tags)
    {
        return Ok(Unconfirmed);
    }
    // The text endpoint returns the current texts but no releaseId. Bracket it
    // with owner metadata and require the same immutable release on both reads.
    let Some(actual) = own_page(remote, PAGE_LIMIT, offset, cancel)
        .await?
        .into_iter()
        .find(|row| row.id == expected.id)
    else {
        return Ok(Unconfirmed);
    };
    if actual.release_id != expected.release_id || !matches_preview(&actual, p) {
        return Ok(Unconfirmed);
    }
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
    }
    transition(engine, &record, "completed", Some(&actual))?;
    Ok(Verified)
}
