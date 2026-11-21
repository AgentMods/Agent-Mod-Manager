//! Test-only external adapters used by discovery, installation and diagnostics.
use super::*;

macro_rules! fixture {
    ($slug:literal) => {
        (
            $slug,
            include_str!(concat!(
                "../../tests/fixtures/game-adapters/",
                $slug,
                "/adapter.json"
            )),
            include_str!(concat!(
                "../../tests/fixtures/game-adapters/",
                $slug,
                "/SKILL.md"
            )),
        )
    };
}

pub(crate) const PACKAGES: [(&str, &str, &str); 6] = [
    fixture!("minecraft-java"),
    fixture!("skyrim-se"),
    fixture!("fallout4"),
    fixture!("stardew-valley"),
    fixture!("cyberpunk-2077"),
    fixture!("baldurs-gate-3"),
];

pub(crate) fn packages() -> AppResult<Vec<GamePackage>> {
    PACKAGES
        .iter()
        .map(|(slug, json, skill)| parse_package(slug, json, skill))
        .collect()
}

/// Populate the test registry without manufacturing deployment history.
pub(crate) fn install(db: &Db, paths: &AppPaths) -> AppResult<()> {
    let mut conn = db.conn();
    let tx = conn.transaction()?;
    for (slug, json, markdown) in PACKAGES {
        let package = parse_package(slug, json, markdown)?;
        let dir = paths.plugins().join("games").join(slug);
        fs::create_dir_all(&dir)?;
        fs::write(dir.join("adapter.json"), json)?;
        fs::write(dir.join("SKILL.md"), markdown)?;
        let now = crate::util::now();
        tx.execute("INSERT INTO plugins(slug,name,game_name,version,description,source,enabled,validated,install_path,adapter_json,created_at,updated_at)
            VALUES (?1,?2,?2,?3,?4,'external',1,1,?5,?6,?7,?7)", params![slug, package.adapter.name, package.adapter.version, package.skill.description, dir.to_string_lossy(), json, now])?;
        tx.execute("INSERT INTO skills(name,description,location,source,enabled,created_at) VALUES (?1,?2,?3,'external',1,?4)", params![slug, package.skill.description, dir.join("SKILL.md").to_string_lossy(), now])?;
    }
    tx.commit()?;
    Ok(())
}
