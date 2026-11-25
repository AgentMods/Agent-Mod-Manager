use super::{taxonomy, EnrichmentInput, ITEM_RULES, QUERY_RULES};
use crate::{db::Db, error::AppResult};
use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};
use sha2::{Digest, Sha256};

const MAX_ROWS: i64 = 4096;
const MAX_PAYLOAD_BYTES: usize = 16 * 1024;
const MAX_AGE_SECONDS: i64 = 60 * 60 * 24 * 30;
const SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS browse_enrichment_cache (
        fingerprint TEXT PRIMARY KEY NOT NULL,
        payload TEXT NOT NULL,
        written_at INTEGER NOT NULL
    );
    CREATE INDEX IF NOT EXISTS browse_enrichment_recency
    ON browse_enrichment_cache(written_at DESC, fingerprint DESC);";

fn fingerprint(
    kind: &str,
    rules: &str,
    fields: impl IntoIterator<Item = impl AsRef<str>>,
) -> String {
    // Length framing covers field boundaries (including arbitrary punctuation and
    // newlines), without serializing identities or original text into SQLite.
    let mut hash = Sha256::new();
    let mut add = |value: &str| {
        hash.update((value.len() as u64).to_le_bytes());
        hash.update(value.as_bytes());
    };
    for field in [taxonomy::VERSION, kind, rules] {
        add(field);
    }
    for field in fields {
        add(field.as_ref());
    }
    hex::encode(hash.finalize())
}

pub(super) fn item_key(input: &EnrichmentInput<'_>, locale: &str) -> String {
    // Preserve absence vs empty, and hash full originals even if only an excerpt
    // will be sent to the model.
    let fields = [
        input.source,
        input.item_id,
        locale,
        input.title,
        if input.summary.is_some() { "1" } else { "0" },
        input.summary.unwrap_or_default(),
        if input.kind.is_some() { "1" } else { "0" },
        input.kind.unwrap_or_default(),
    ];
    fingerprint(
        "item",
        ITEM_RULES,
        fields
            .into_iter()
            .chain(input.tags.iter().map(String::as_str)),
    )
}

pub(super) fn query_key(original: &str, app_locale: &str, target_locale: &str) -> String {
    fingerprint("query", QUERY_RULES, [original, app_locale, target_locale])
}

fn initialize(connection: &Connection) -> AppResult<()> {
    Ok(connection.execute_batch(SCHEMA)?)
}

pub(super) fn read<T: DeserializeOwned>(
    db: &Db,
    keys: &[Option<String>],
) -> AppResult<Vec<Option<T>>> {
    let connection = db.conn();
    initialize(&connection)?;
    let mut statement = connection.prepare_cached(
        "SELECT payload FROM browse_enrichment_cache
         WHERE fingerprint=?1 AND written_at >= unixepoch() - ?2 AND length(CAST(payload AS BLOB)) <= ?3",
    )?;
    keys.iter()
        .map(|key| {
            let Some(key) = key else { return Ok(None) };
            let payload: Option<String> = statement
                .query_row(params![key, MAX_AGE_SECONDS, MAX_PAYLOAD_BYTES], |row| {
                    row.get(0)
                })
                .optional()?;
            Ok(payload.and_then(|text| serde_json::from_str::<T>(&text).ok()))
        })
        .collect()
}

pub(super) fn write<T: Serialize>(db: &Db, entries: &[(&str, T)]) -> AppResult<()> {
    if entries.is_empty() {
        return Ok(());
    }
    let mut connection = db.conn();
    initialize(&connection)?;
    let transaction = connection.transaction()?;
    {
        let mut statement = transaction.prepare_cached(
            "INSERT INTO browse_enrichment_cache(fingerprint,payload,written_at)
             VALUES(?1,?2,unixepoch()) ON CONFLICT(fingerprint) DO UPDATE SET
             payload=excluded.payload,written_at=excluded.written_at",
        )?;
        for (key, value) in entries {
            let payload = serde_json::to_string(value)?;
            if payload.len() <= MAX_PAYLOAD_BYTES {
                statement.execute(params![key, payload])?;
            }
        }
    }
    transaction.execute(
        "DELETE FROM browse_enrichment_cache WHERE written_at < unixepoch() - ?1
         OR fingerprint IN (SELECT fingerprint FROM browse_enrichment_cache
                            ORDER BY written_at DESC, fingerprint DESC LIMIT -1 OFFSET ?2)",
        params![MAX_AGE_SECONDS, MAX_ROWS],
    )?;
    Ok(transaction.commit()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expired_and_malformed_rows_are_ignored_and_cache_is_bounded() {
        let db = Db(std::sync::Arc::new(std::sync::Mutex::new(
            Connection::open_in_memory().unwrap(),
        )));
        write(
            &db,
            &[
                ("valid", "translation"),
                ("stale", "obsolete"),
                ("invalid", "ok"),
            ],
        )
        .unwrap();
        {
            let connection = db.conn();
            connection.execute(
                "UPDATE browse_enrichment_cache SET written_at=unixepoch()-?1-1 WHERE fingerprint='stale'",
                [MAX_AGE_SECONDS],
            ).unwrap();
            connection.execute("UPDATE browse_enrichment_cache SET payload='broken' WHERE fingerprint='invalid'", []).unwrap();
        }
        assert_eq!(
            read::<String>(
                &db,
                &[
                    Some("valid".into()),
                    Some("stale".into()),
                    Some("invalid".into())
                ]
            )
            .unwrap(),
            [Some("translation".into()), None, None]
        );
        let keys: Vec<_> = (0..=MAX_ROWS).map(|index| format!("row-{index}")).collect();
        let entries: Vec<_> = keys.iter().map(|key| (key.as_str(), "result")).collect();
        write(&db, &entries).unwrap();
        let count: i64 = db
            .conn()
            .query_row("SELECT count(*) FROM browse_enrichment_cache", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(count, MAX_ROWS);
    }

    #[test]
    fn fingerprints_include_rules_locale_identity_and_original_field_boundaries() {
        let make = |rules: &str, fields: [&str; 2]| fingerprint("item", rules, fields);
        assert_ne!(make("r1", ["a", "bc"]), make("r2", ["a", "bc"]));
        assert_ne!(make("r1", ["a", "bc"]), make("r1", ["ab", "c"]));
        assert_ne!(query_key("mod", "en", "ja"), query_key("mod", "ja", "en"));
        let mut input = EnrichmentInput {
            source: "3dm",
            item_id: "13",
            title: "mod",
            summary: Some(""),
            kind: None,
            tags: &[],
        };
        let first = item_key(&input, "en");
        input.summary = None;
        assert_ne!(first, item_key(&input, "en"));
        input.summary = Some("");
        assert_ne!(first, item_key(&input, "fr"));
        input.source = "nexus";
        assert_ne!(first, item_key(&input, "en"));
        input.source = "3dm";
        input.item_id = "14";
        assert_ne!(first, item_key(&input, "en"));
    }
}
