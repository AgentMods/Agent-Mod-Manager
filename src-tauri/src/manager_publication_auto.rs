//! Automatic manager publication snapshots and authenticated owner preflight.
use super::{
    create, matches_preview, own_page, own_texts, send, stale, transition, unavailable, valid_id,
    with_current_source, PublicationPreview, PublicationRecord, PublicationState,
    PublicationTarget, ReconciliationOutcome, Remote, PAGE_LIMIT,
};
use crate::{
    error::{AppError, AppResult},
    plugin_publish::{self as provenance, PublicationFile, PublicationSource},
    plugins::{self, GamePackage},
    publication_copy::{self, PublicationListing},
    settings::{SecretStore, Settings},
    transaction::FileEngine,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{cmp::Ordering, collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

#[cfg(test)]
#[path = "manager_publication_auto_tests.rs"]
mod tests;

const SECRET_REF: &str = "mcp.glossmod_http.api_key";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(super) struct CredentialBinding(String);

impl CredentialBinding {
    fn from_key(key: &str) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"amm.manager-plugin-publication.credential.v1\0");
        hash.update(key.as_bytes());
        Self(hex::encode(hash.finalize()))
    }

    pub(super) fn for_preview(preview: &PublicationPreview) -> AppResult<&Self> {
        preview
            .snapshot
            .as_ref()
            .map(|snapshot| &snapshot.credential_binding)
            .ok_or_else(|| AppError::Conflict("请使用最新流程重新预览此插件发布".into()))
    }

    fn check(&self, settings: &Settings) -> AppResult<Zeroizing<String>> {
        configured_key(settings)?
            .filter(|key| Self::from_key(key) == *self)
            .ok_or_else(|| AppError::Conflict("Mod 客栈发布凭据已更改，请重新预览".into()))
    }

    pub(super) async fn connect(
        &self,
        settings: &Settings,
        cancel: &CancellationToken,
    ) -> AppResult<super::remote::Transport> {
        connect_with_key(settings, self.check(settings)?, cancel).await
    }
}

fn configured_key(settings: &Settings) -> AppResult<Option<Zeroizing<String>>> {
    Ok(settings
        .read_secret(SECRET_REF)
        .map_err(|_| AppError::Other("无法读取 Mod 客栈凭据".into()))?
        .map(Zeroizing::new)
        .filter(|key| !key.is_empty() && key.len() <= 8192 && !key.chars().any(char::is_control)))
}

fn require_key(settings: &Settings) -> AppResult<Zeroizing<String>> {
    configured_key(settings)?
        .ok_or_else(|| AppError::Denied("请先在设置中配置 Mod 客栈 API Key".into()))
}

async fn connect_with_key(
    settings: &Settings,
    key: Zeroizing<String>,
    cancel: &CancellationToken,
) -> AppResult<super::remote::Transport> {
    // The transport reads precisely the key used for the fingerprint. Another
    // independent settings read during connection could use a rotated key.
    let pinned = Settings::with_secret_store(settings.db.clone(), Arc::new(PinnedKey(key)));
    super::remote::Transport::connect(&pinned, cancel).await
}

struct PinnedKey(Zeroizing<String>);

impl SecretStore for PinnedKey {
    fn read(&self, key: &str) -> AppResult<Option<String>> {
        Ok((key == SECRET_REF).then(|| self.0.as_str().to_owned()))
    }

    fn write(&self, _: &str, _: &str) -> AppResult<()> {
        Err(AppError::Denied("发布连接使用只读凭据".into()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct Snapshot {
    source_sha256: [String; 2],
    credential_binding: CredentialBinding,
    remote_release_id: Option<i64>,
}

impl Snapshot {
    pub(super) fn validate(&self, preview: &PublicationPreview) -> AppResult<()> {
        let listing = preview.listing.as_ref().ok_or_else(stale)?;
        let original_version = preview.original_version.as_deref().ok_or_else(stale)?;
        let remote_version = preview.remote_version.as_deref();
        let is_release = preview.target.plugin_id().is_some();
        if preview.tags != listing.tags
            || is_release != remote_version.is_some()
            || is_release != self.remote_release_id.is_some()
            || self.remote_release_id.is_some_and(|id| !valid_id(id))
            || !valid_sha256(&self.credential_binding.0)
            || !self.source_sha256.iter().all(|hash| valid_sha256(hash))
            || preview.version != published_version(original_version, remote_version)?
        {
            return Err(stale());
        }
        Ok(())
    }

    pub(super) fn credential_fingerprint(&self) -> &str {
        &self.credential_binding.0
    }

    pub(super) fn previous_release_id(&self) -> Option<i64> {
        self.remote_release_id
    }

    pub(super) fn matches_files(&self, files: &[PublicationFile; 2]) -> bool {
        files
            .iter()
            .zip(&self.source_sha256)
            .all(|(file, hash)| &file.sha256 == hash)
    }
}

fn valid_sha256(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

struct SourceSnapshot {
    source: PublicationSource,
    files: [PublicationFile; 2],
    package: GamePackage,
}

impl SourceSnapshot {
    fn read(engine: &FileEngine, draft_id: Option<&str>, slug: Option<&str>) -> AppResult<Self> {
        let capture = |source, slug: &str, files: [PublicationFile; 2]| {
            let package = plugins::parse_package(slug, &files[0].text, &files[1].text)?;
            Ok(Self {
                source,
                files,
                package,
            })
        };
        match (draft_id, slug) {
            (Some(id), None) => provenance::with_draft_source(engine, id, capture),
            (None, Some(slug)) => provenance::with_installed_source(engine, slug, capture),
            _ => Err(super::invalid("请选择一个已提交草稿或已安装的插件")),
        }
    }
}

pub async fn prepare_automatic(
    engine: &FileEngine,
    settings: &Settings,
    draft_id: Option<String>,
    slug: Option<String>,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    check_cancel(cancel)?;
    let source = SourceSnapshot::read(engine, draft_id.as_deref(), slug.as_deref())?;
    let key = require_key(settings)?;
    let credential = CredentialBinding::from_key(&key);
    let remote = connect_with_key(settings, key, cancel).await?;
    prepare_with(
        engine,
        settings,
        &remote,
        source,
        credential,
        cancel,
        async |package| publication_copy::generate(settings, package, cancel).await,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn prepare_automatic_with(
    engine: &FileEngine,
    settings: &Settings,
    remote: &dyn Remote,
    draft_id: Option<String>,
    slug: Option<String>,
    listing: PublicationListing,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    let source = SourceSnapshot::read(engine, draft_id.as_deref(), slug.as_deref())?;
    let credential = CredentialBinding::from_key(&require_key(settings)?);
    prepare_with(
        engine,
        settings,
        remote,
        source,
        credential,
        cancel,
        async |_| Ok(listing),
    )
    .await
}

#[cfg(test)]
pub(crate) async fn publish_automatic_with(
    engine: &FileEngine,
    settings: &Settings,
    remote: &dyn Remote,
    id: &str,
    digest: &str,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    check_cancel(cancel)?;
    let record = super::approved(engine, id, digest)?;
    if record.state == PublicationState::Completed {
        return Ok(record);
    }
    with_current_source(engine, &record.preview, || Ok(()))?;
    publish_with(engine, settings, remote, &record, cancel).await
}

async fn prepare_with(
    engine: &FileEngine,
    settings: &Settings,
    remote: &dyn Remote,
    source: SourceSnapshot,
    credential: CredentialBinding,
    cancel: &CancellationToken,
    generate: impl AsyncFnOnce(&GamePackage) -> AppResult<PublicationListing>,
) -> AppResult<PublicationRecord> {
    let slug = &source.package.adapter.slug;
    let owner = owned_slug(remote, slug, cancel).await?;
    let remote_version = owner.as_ref().map(|plugin| plugin.version.clone());
    let original_version = &source.package.adapter.version;
    let version = published_version(original_version, remote_version.as_deref())?;
    let listing = generate(&source.package).await?;
    check_cancel(cancel)?;
    let files = publication_copy::render_files(&source.package, &source.files, &listing, &version)?;
    let mut preview = PublicationPreview {
        source: source.source,
        target: owner.as_ref().map_or(PublicationTarget::First, |plugin| {
            PublicationTarget::Release {
                plugin_id: plugin.id,
            }
        }),
        slug: slug.clone(),
        version,
        tags: listing.tags.clone(),
        files,
        destination: provenance::DESTINATION.into(),
        digest: String::new(),
        listing: Some(listing),
        original_version: Some(original_version.clone()),
        remote_version,
        snapshot: Some(Snapshot {
            source_sha256: source.files.map(|file| file.sha256),
            credential_binding: credential.clone(),
            remote_release_id: owner.as_ref().map(|plugin| plugin.release_id),
        }),
    };
    preview.digest = preview.checked_digest()?;
    if contains_key(
        &serde_json::to_value(&preview)?,
        &credential.check(settings)?,
    ) {
        return Err(AppError::Denied("插件发布预览包含 Mod 客栈凭据".into()));
    }
    with_current_source(engine, &preview, || {
        check_cancel(cancel)?;
        create(engine, &preview)
    })
}

fn contains_key(value: &serde_json::Value, key: &str) -> bool {
    use serde_json::Value;
    match value {
        Value::String(text) => text.contains(key),
        Value::Array(items) => items.iter().any(|value| contains_key(value, key)),
        Value::Object(items) => items
            .iter()
            .any(|(name, value)| name.contains(key) || contains_key(value, key)),
        _ => false,
    }
}

async fn owned_slug(
    remote: &dyn Remote,
    slug: &str,
    cancel: &CancellationToken,
) -> AppResult<Option<super::PublishedPlugin>> {
    let mut seen = BTreeSet::new();
    let mut matched = None;
    let mut offset = 0_u32;
    loop {
        check_cancel(cancel)?;
        let page = own_page(remote, PAGE_LIMIT, offset, cancel).await?;
        let last = page.len() < PAGE_LIMIT as usize;
        for plugin in page {
            if !seen.insert(plugin.id) {
                return Err(unavailable());
            }
            if plugin.slug == slug && matched.replace(plugin).is_some() {
                return Err(AppError::Conflict(
                    "Mod 客栈账号下有多个相同 slug 的插件，请核对后重新预览".into(),
                ));
            }
        }
        if last {
            check_cancel(cancel)?;
            return Ok(matched);
        }
        offset = offset.checked_add(PAGE_LIMIT).ok_or_else(unavailable)?;
    }
}

pub(super) async fn publish_with(
    engine: &FileEngine,
    settings: &Settings,
    remote: &dyn Remote,
    record: &PublicationRecord,
    cancel: &CancellationToken,
) -> AppResult<PublicationRecord> {
    let preview = &record.preview;
    let binding = CredentialBinding::for_preview(preview)?;
    binding.check(settings)?;
    let owner = owned_slug(remote, &preview.slug, cancel).await?;
    let unchanged = match (preview.target, owner) {
        (PublicationTarget::First, None) => true,
        (PublicationTarget::Release { plugin_id }, Some(plugin)) => {
            plugin.id == plugin_id
                && Some(plugin.version.as_str()) == preview.remote_version.as_deref()
                && Some(plugin.release_id)
                    == preview
                        .snapshot
                        .as_ref()
                        .and_then(Snapshot::previous_release_id)
        }
        _ => false,
    };
    if !unchanged {
        return Err(AppError::Conflict(
            "Mod 客栈插件归属或最新版本已更改，请重新预览".into(),
        ));
    }
    send(engine, remote, record, cancel, || {
        binding.check(settings)?;
        check_cancel(cancel)
    })
    .await
}

pub(super) async fn reconcile_with(
    engine: &FileEngine,
    settings: &Settings,
    remote: &dyn Remote,
    record: &PublicationRecord,
    cancel: &CancellationToken,
) -> AppResult<ReconciliationOutcome> {
    use ReconciliationOutcome::{Unconfirmed, Verified};
    if record.state != PublicationState::Unknown {
        return Ok(Unconfirmed);
    }
    let preview = &record.preview;
    let binding = CredentialBinding::for_preview(preview)?;
    binding.check(settings)?;
    let Some(expected) = owned_slug(remote, &preview.slug, cancel)
        .await?
        .filter(|plugin| matches_preview(plugin, preview))
    else {
        return Ok(Unconfirmed);
    };
    let texts = own_texts(remote, expected.id, false, cancel).await?;
    if texts.adapter != preview.files[0].text
        || texts.skill != preview.files[1].text
        || !super::same_tags(&texts.tags, &preview.tags)
    {
        return Ok(Unconfirmed);
    }
    // The text endpoint has no release ID. Confirm that the same release was
    // current on both sides of the text read.
    let Some(actual) = owned_slug(remote, &preview.slug, cancel)
        .await?
        .filter(|plugin| {
            plugin.id == expected.id
                && plugin.release_id == expected.release_id
                && matches_preview(plugin, preview)
        })
    else {
        return Ok(Unconfirmed);
    };
    binding.check(settings)?;
    check_cancel(cancel)?;
    transition(engine, record, "completed", Some(&actual))?;
    Ok(Verified)
}

fn check_cancel(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

fn published_version(source: &str, remote: Option<&str>) -> AppResult<String> {
    plugins::validate_version(source)?;
    let Some(remote) = remote else {
        return Ok(source.into());
    };
    plugins::validate_version(remote)?;
    if version_cmp(source, remote).is_gt() {
        return Ok(source.into());
    }
    let (core, pre) = precedence_parts(remote);
    if pre.is_some() {
        return Ok(core.into());
    }
    let mut parts: Vec<String> = core.split('.').map(str::to_owned).collect();
    for index in (0..parts.len()).rev() {
        parts[index] = increment_decimal(&parts[index]);
        let candidate = parts.join(".");
        if plugins::validate_version(&candidate).is_ok() {
            return Ok(candidate);
        }
        parts[index] = "0".into();
    }
    Err(super::invalid("远程版本已达到适配包允许的版本长度"))
}

fn precedence_parts(version: &str) -> (&str, Option<&str>) {
    let base = version.split('+').next().unwrap_or_default();
    base.split_once('-')
        .map_or((base, None), |(a, b)| (a, Some(b)))
}

fn decimal_cmp(a: &str, b: &str) -> Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

fn version_cmp(a: &str, b: &str) -> Ordering {
    let (a, a_pre) = precedence_parts(a);
    let (b, b_pre) = precedence_parts(b);
    for (a, b) in a.split('.').zip(b.split('.')) {
        let cmp = decimal_cmp(a, b);
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
    let (Some(a), Some(b)) = (a_pre, b_pre) else {
        return b_pre.is_some().cmp(&a_pre.is_some());
    };
    for (a, b) in a.split('.').zip(b.split('.')) {
        let numeric = |id: &str| id.bytes().all(|byte| byte.is_ascii_digit());
        let cmp = match (numeric(a), numeric(b)) {
            (true, true) => decimal_cmp(a, b),
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            (false, false) => a.cmp(b),
        };
        if cmp != Ordering::Equal {
            return cmp;
        }
    }
    a.split('.').count().cmp(&b.split('.').count())
}

fn increment_decimal(decimal: &str) -> String {
    let mut digits: Vec<char> = decimal.chars().collect();
    for digit in digits.iter_mut().rev() {
        if *digit != '9' {
            *digit = char::from(*digit as u8 + 1);
            return digits.into_iter().collect();
        }
        *digit = '0';
    }
    std::iter::once('1').chain(digits).collect()
}
