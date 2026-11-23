//! Durable, consent-bound publication of manager plugins (not ordinary works).
use crate::{
    error::{AppError, AppResult},
    plugin_publish::{self as provenance, PublicationFile, PublicationSource},
    plugins,
    publication_copy::PublicationListing,
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

#[path = "manager_publication_auto.rs"]
mod automatic;
#[path = "manager_publication_remote.rs"]
mod remote;
#[cfg(test)]
#[path = "manager_publication_tests.rs"]
mod tests;

pub use crate::plugin_publish_remote::ReconciliationOutcome;
pub use automatic::prepare_automatic;
pub(crate) use automatic::version_cmp;
#[cfg(test)]
pub(crate) use automatic::{prepare_automatic_with, publish_automatic_with};

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub listing: Option<PublicationListing>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot: Option<automatic::Snapshot>,
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

#[cfg(test)]
pub fn prepare_draft(
    engine: &FileEngine,
    draft_id: &str,
    target: PublicationTarget,
    tags: Vec<String>,
) -> AppResult<PublicationRecord> {
    provenance::with_draft_source(engine, draft_id, |source, slug, files| {
        create(engine, &preview(source, slug, files, target, tags)?)
    })
}

#[cfg(test)]
pub fn prepare_installed(
    engine: &FileEngine,
    slug: &str,
    target: PublicationTarget,
    tags: Vec<String>,
) -> AppResult<PublicationRecord> {
    provenance::with_installed_source(engine, slug, |source, slug, files| {
        create(engine, &preview(source, slug, files, target, tags)?)
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
    // The site identifies tags by works.TagSlug: lowercase, split on
    // whitespace and join with hyphens. Compare pre-hash keys; equal keys
    // also produce equal hashes for long tags. Fold only ASCII here to avoid
    // assuming Go and Rust use the same Unicode lowercase tables. Database
    // aliases that change the key cannot be established by this comparison.
    fn keys(tags: &[String]) -> Option<BTreeSet<String>> {
        Some(
            tags_set(tags)?
                .into_iter()
                .map(|tag| {
                    tag.split_whitespace()
                        .map(str::to_ascii_lowercase)
                        .collect::<Vec<_>>()
                        .join("-")
                })
                .collect(),
        )
    }
    keys(a).is_some_and(|a| keys(b).is_some_and(|b| a == b))
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

#[cfg(test)]
fn preview(
    source: PublicationSource,
    slug: &str,
    files: [PublicationFile; 2],
    target: PublicationTarget,
    tags: Vec<String>,
) -> AppResult<PublicationPreview> {
    let version = plugins::parse_package(slug, &files[0].text, &files[1].text)?
        .adapter
        .version;
    let mut preview = PublicationPreview {
        source,
        target,
        slug: slug.into(),
        version,
        tags,
        files,
        destination: provenance::DESTINATION.into(),
        digest: String::new(),
        listing: None,
        original_version: None,
        remote_version: None,
        snapshot: None,
    };
    preview.digest = preview.checked_digest()?;
    Ok(preview)
}

impl PublicationPreview {
    // Historical pixel-art receipts must stay byte-identical for audit/reconciliation.
    // They are no longer eligible to authorize a new write, even if approved before upgrade.
    fn require_current_cover(&self) -> AppResult<()> {
        if self
            .listing
            .as_ref()
            .is_some_and(|listing| !listing.cover_png.is_empty())
        {
            return Err(AppError::Conflict(
                "这是旧版像素封面预览，请重新点击「发布插件」核对游戏封面".into(),
            ));
        }
        Ok(())
    }

    fn checked_digest(&self) -> AppResult<String> {
        if self.target.plugin_id().is_some_and(|id| !valid_id(id))
            || tags_set(&self.tags).is_none()
            || self.tags.iter().map(String::len).sum::<usize>() > 64 * 1024
        {
            return Err(invalid("管理器插件 ID 或标签无效"));
        }
        for (file, name) in self.files.iter().zip(["adapter.json", "SKILL.md"]) {
            if file.name != name
                || file.size != file.text.len() as u64
                || file.sha256 != hex::encode(Sha256::digest(file.text.as_bytes()))
            {
                return Err(stale());
            }
        }
        let package = plugins::parse_package(&self.slug, &self.files[0].text, &self.files[1].text)?;
        if self.listing.as_ref().is_some_and(|listing| {
            listing.cover_png.is_empty() && listing.cover_url != package.adapter.cover_url
        }) {
            return Err(stale());
        }
        if package.adapter.version != self.version || self.destination != provenance::DESTINATION {
            return Err(stale());
        }
        let mut hash = Sha256::new();
        let base = (
            &self.source,
            self.target,
            &self.slug,
            &self.version,
            &self.tags,
            &self.files,
            &self.destination,
        );
        if let Some(snapshot) = &self.snapshot {
            snapshot.validate(self)?;
            hash.update(b"amm.manager-plugin-publication.v2\0");
            serde_json::to_writer(
                &mut hash,
                &(
                    base,
                    &self.listing,
                    &self.original_version,
                    &self.remote_version,
                    snapshot,
                ),
            )?;
        } else {
            if self.listing.is_some()
                || self.original_version.is_some()
                || self.remote_version.is_some()
            {
                return Err(stale());
            }
            hash.update(b"amm.manager-plugin-publication.v1\0");
            serde_json::to_writer(&mut hash, &base)?;
        }
        Ok(hex::encode(hash.finalize()))
    }
}

fn with_source<T>(
    engine: &FileEngine,
    source: &PublicationSource,
    check: impl FnOnce(PublicationSource, &str, [PublicationFile; 2]) -> AppResult<T>,
) -> AppResult<T> {
    match source {
        PublicationSource::Draft { draft_id, .. } => {
            provenance::with_draft_source(engine, draft_id, check)
        }
        PublicationSource::Installed { slug, .. } => {
            provenance::with_installed_source(engine, slug, check)
        }
    }
}

fn with_current_source<T>(
    engine: &FileEngine,
    preview: &PublicationPreview,
    f: impl FnOnce() -> AppResult<T>,
) -> AppResult<T> {
    with_source(engine, &preview.source, |source, slug, files| {
        if source != preview.source || slug != preview.slug {
            return Err(stale());
        }
        let matches = match &preview.snapshot {
            Some(snapshot) => snapshot.matches_files(&files),
            None => files == preview.files,
        };
        if !matches {
            return Err(stale());
        }
        // Historical digests and receipts stay readable. Only a new approval or
        // write requires the source game's name in the already-reviewed tags.
        if let Some(listing) = &preview.listing {
            let adapter: plugins::GameAdapter = serde_json::from_str(&files[0].text)?;
            listing.require_game_tag(adapter.display_game_name())?;
        }
        f()
    })
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
            "SELECT preview_json,
                digest,slug,version,target_plugin_id,approved_digest,state,
                CASE WHEN length(CAST(remote_json AS BLOB))<=524288 THEN remote_json
                     WHEN remote_json IS NULL THEN NULL ELSE '' END,created_at,updated_at
           FROM manager_publications WHERE id=?1",
            [id],
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
    let state: PublicationState =
        serde_json::from_value(Value::String(state)).map_err(|_| stale())?;
    let remote: Option<PublishedPlugin> = remote
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(|_| stale())?;
    if stored.digest != stored.checked_digest()?
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
            .is_some_and(|p| !matches_receipt(p, &stored))
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

fn create(engine: &FileEngine, preview: &PublicationPreview) -> AppResult<PublicationRecord> {
    let raw = serde_json::to_string(preview)?;
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
    if record.preview != *preview {
        return Err(stale());
    }
    tx.commit()?;
    Ok(record)
}

pub fn get_publication(engine: &FileEngine, id: &str) -> AppResult<PublicationRecord> {
    load(&engine.db.conn(), id)
}

pub fn list_publications(
    engine: &FileEngine,
    before: Option<&str>,
) -> AppResult<Vec<PublicationRecord>> {
    let conn = engine.db.conn();
    let mut stmt = conn.prepare(
        "SELECT id FROM manager_publications
        WHERE ?1 IS NULL OR rowid < (SELECT rowid FROM manager_publications WHERE id=?1)
        ORDER BY rowid DESC LIMIT 50",
    )?;
    let ids = stmt
        .query_map([before], |r| r.get::<_, String>(0))?
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
    record.preview.require_current_cover()?;
    with_current_source(engine, &record.preview, || {
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

// Check inside the same IMMEDIATE transaction that claims the send. Only the
// authenticated preflight for this send can distinguish a new owned release
// from a replay; a slug or a historical receipt alone grants no permission.
fn check_duplicate(
    conn: &Connection,
    record: &PublicationRecord,
    owner: Option<&PublishedPlugin>,
) -> AppResult<()> {
    let p = &record.preview;
    let credential = p
        .snapshot
        .as_ref()
        .map(automatic::Snapshot::credential_fingerprint);
    let mut stmt = conn.prepare(
        "SELECT state,version,target_plugin_id,json_extract(preview_json,'$.snapshot.credentialBinding'),
                json_extract(remote_json,'$.id') FROM manager_publications
         WHERE slug=?1 AND id<>?2 AND (state IN ('pending','unknown')
            OR (state='completed' AND (?3 IS NULL OR version=?4)))",
    )?;
    let mut rows = stmt.query(params![p.slug, record.id, p.target.plugin_id(), p.version])?;
    while let Some(row) = rows.next()? {
        let state: String = row.get(0)?;
        let version: String = row.get(1)?;
        let target: Option<i64> = row.get(2)?;
        let prior_credential: Option<String> = row.get(3)?;
        let receipt_id: Option<i64> = row.get(4)?;
        let same_plugin = owner.is_some_and(|plugin| Some(plugin.id) == receipt_id.or(target));
        let unbound_uncertain = prior_credential.is_none() && state != "completed";
        if credential.is_some()
            && prior_credential.as_deref() != credential
            && !same_plugin
            && !unbound_uncertain
        {
            continue; // A different credential, with no evidence of a shared target.
        }
        let message = match state.as_str() {
            "pending" => "该插件正在发布，请等待本次发布完成",
            "unknown" => {
                // A separately reviewed release above the verified cloud head
                // cannot resend this older version. Leave its outcome unknown.
                if prior_credential.is_some()
                    && owner.is_some_and(|plugin| {
                        target.is_none_or(|id| id == plugin.id)
                            && plugins::validate_version(&version).is_ok()
                            && !automatic::version_cmp(&version, &plugin.version).is_gt()
                            && automatic::version_cmp(&p.version, &plugin.version).is_gt()
                    })
                {
                    continue;
                }
                "该插件有尚未确认的发布，请打开发布记录查看状态"
            }
            "completed"
                if owner.is_some_and(|plugin| receipt_id.is_some_and(|id| id != plugin.id)) =>
            {
                continue
            }
            "completed" if owner.is_some() => "该插件的此版本已发布，请重新预览新版本",
            "completed" => "该插件已有发布记录，请重新预览并核对当前账号的插件归属",
            _ => return Err(stale()),
        };
        return Err(AppError::Conflict(message.into()));
    }
    Ok(())
}

fn claim_checked(
    engine: &FileEngine,
    record: &PublicationRecord,
    owner: Option<&PublishedPlugin>,
    check: impl FnOnce() -> AppResult<()>,
) -> AppResult<PublicationRecord> {
    with_current_source(engine, &record.preview, || {
        check()?;
        let mut conn = engine.db.conn();
        let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
        if load(&tx, &record.id)? != *record {
            return Err(stale());
        }
        check_duplicate(&tx, record, owner)?;
        tx.execute(
            "UPDATE manager_publications SET state='pending',updated_at=?2 WHERE id=?1",
            params![record.id, util::now()],
        )?;
        let pending = load(&tx, &record.id)?;
        tx.commit()?;
        Ok(pending)
    })
}

#[cfg(test)]
fn claim(engine: &FileEngine, record: &PublicationRecord) -> AppResult<PublicationRecord> {
    claim_checked(engine, record, None, || Ok(()))
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
    drop(conn);

    // A receipt is durable before the independent, best-effort draft cleanup.
    if record.state == PublicationState::Completed {
        if let PublicationSource::Draft { draft_id, .. } = &record.preview.source {
            crate::plugin_workflow::cleanup_consumed_agent_drafts(engine, Some(draft_id));
        }
    }
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
    /// `Denied`, `NotFound`, `Conflict`, and `Invalid` are reserved for
    /// definitive pre-commit rejections, not transport or decoding errors.
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

fn matches_receipt(plugin: &PublishedPlugin, preview: &PublicationPreview) -> bool {
    valid_plugin(plugin)
        && plugin.slug == preview.slug
        && plugin.version == preview.version
        && preview.target.plugin_id().is_none_or(|id| id == plugin.id)
        // The committed receipt returns site-canonical tag names, which can
        // differ from the submitted names (existing tags and aliases). Each
        // submitted tag can contribute at most one returned canonical tag.
        && plugin.tags.len() <= preview.tags.len()
        && plugin.tags.is_empty() == preview.tags.is_empty()
        && preview
            .listing
            .as_ref()
            .is_none_or(|listing| plugin.name == listing.title)
        && preview
            .snapshot
            .as_ref()
            .is_none_or(|snapshot| snapshot.previous_release_id() != Some(plugin.release_id))
}

fn matches_preview(plugin: &PublishedPlugin, preview: &PublicationPreview) -> bool {
    // Read-back is not a response to our write. Without a mapping from each
    // requested tag to its canonical tag, require the same deterministic tag keys.
    matches_receipt(plugin, preview) && same_tags(&plugin.tags, &preview.tags)
}

fn approved(engine: &FileEngine, id: &str, digest: &str) -> AppResult<PublicationRecord> {
    let record = get_publication(engine, id)?;
    if record.approved_digest.as_deref() != Some(digest) {
        return Err(stale());
    }
    match record.state {
        PublicationState::Intended | PublicationState::Completed => Ok(record),
        _ => Err(AppError::Conflict(
            "此发布不能重复发送，请打开发布记录查看状态".into(),
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
    record.preview.require_current_cover()?;
    let binding = automatic::CredentialBinding::for_preview(&record.preview)?;
    with_current_source(engine, &record.preview, || Ok(()))?;
    let remote = binding.connect(settings, cancel).await?;
    automatic::publish_with(engine, settings, &remote, &record, cancel).await
}

#[cfg(test)]
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
    if record.preview.snapshot.is_some() {
        return Err(stale());
    }
    send(engine, remote, &record, None, cancel, || Ok(())).await
}

async fn send(
    engine: &FileEngine,
    remote: &dyn Remote,
    record: &PublicationRecord,
    owner: Option<&PublishedPlugin>,
    cancel: &CancellationToken,
    check: impl FnOnce() -> AppResult<()>,
) -> AppResult<PublicationRecord> {
    if cancel.is_cancelled() {
        return Err(AppError::Cancelled);
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
    let pending = claim_checked(engine, record, owner, check)?;
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
            Ok(receipt) if matches_receipt(&receipt.plugin, p) => {
                transition(engine, &pending, "completed", Some(&receipt.plugin))
            }
            _ => transition(engine, &pending, "unknown", None),
        },
        Err(
            error @ (AppError::Denied(_)
            | AppError::NotFound(_)
            | AppError::Conflict(_)
            | AppError::Invalid(_)),
        ) => {
            transition(engine, &pending, "error", None)?;
            guard.armed = false;
            return Err(error);
        }
        // Timeouts, session/server failures, cancellation, and unverified
        // responses are never retried. Return a durable record for UI/Agent.
        Err(_) => transition(engine, &pending, "unknown", None),
    };
    if result.is_ok() {
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
    direct_read: bool,
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
    // Unsolicited remote text keeps the catalog limits. Reconciliation may
    // compare larger text only against the exact approved local snapshot;
    // the MCP transport's independent inbound response cap still applies.
    if (direct_read
        && (texts.adapter.len() > crate::plugin_workflow::ADAPTER_LIMIT
            || texts.skill.len() > crate::plugin_workflow::SKILL_LIMIT))
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
        true,
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
    if record.preview.snapshot.is_some() {
        let binding = automatic::CredentialBinding::for_preview(&record.preview)?;
        let transport = binding.connect(settings, cancel).await?;
        return automatic::reconcile_with(engine, settings, &transport, &record, cancel).await;
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
    let texts = own_texts(remote, expected.id, false, cancel).await?;
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
