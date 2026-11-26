//! Standalone archive jobs only publish input snapshots in the MCP workspace.
use super::*;
use crate::safe_fs::{DiskBudget, StagingDirectory};

pub(crate) const MAX_ARCHIVE_PAYLOADS: usize = 2047;
const MAX_ARCHIVE_BYTES: u64 = u32::MAX as u64;
const MAX_PAYLOAD_BYTES: u64 = 128 * 1024 * 1024;
const MAX_JOB_BYTES: u64 = 32 * 1024 * 1024 * 1024;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveInput {
    name: String,
    source: Blob,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchiveManifest<'a> {
    schema_version: u8,
    job_id: &'a str,
    kind: Edition,
    archive_inputs: [ArchiveInput; 1],
    payload_inputs: Vec<Blob>,
}

#[derive(Serialize)]
struct ArchiveReference {
    path: String,
    name: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PayloadReference {
    game_path: String,
    source_path: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PreparedArchiveJob {
    job_id: String,
    game_id: String,
    edition: Edition,
    archive_path: String,
    archive: ArchiveReference,
    payloads: Vec<PayloadReference>,
}

struct Source {
    game_path: String,
    path: PathBuf,
    size: u64,
}

impl Source {
    fn resolve(
        engine: &FileEngine,
        root: &Path,
        requested: &str,
        sizes: std::ops::RangeInclusive<u64>,
    ) -> AppResult<Self> {
        let game_path = requested.replace('\\', "/");
        if game_path.chars().count() > 512
            || game_path.split('/').any(|part| {
                part.is_empty()
                    || part == "."
                    || part.starts_with(' ')
                    || part
                        .split('.')
                        .next()
                        .is_some_and(|stem| stem.ends_with(' '))
                    || part
                        .as_bytes()
                        .windows(2)
                        .any(|p| p[0] == b'~' && p[1].is_ascii_digit())
            })
        {
            return Err(invalid("请提供不含别名的游戏相对文件路径"));
        }
        let path = crate::paths::safe_join(root, &game_path)?;
        if engine
            .paths
            .guard(&path, std::slice::from_ref(&root.to_path_buf()))?
            != path
        {
            return Err(conflict("指定的游戏文件路径经过链接或别名"));
        }
        let file = safe_fs::open_regular(&path).map_err(|error| match error {
            AppError::Io(error) if error.kind() == std::io::ErrorKind::NotFound => {
                AppError::NotFound(format!("游戏目录缺少指定文件 {game_path}"))
            }
            error => error,
        })?;
        safe_fs::verify_regular_at(&file, &path)?;
        let size = file.metadata()?.len();
        if !sizes.contains(&size) {
            return Err(invalid("RPF 输入文件大小超出允许范围"));
        }
        Ok(Self {
            game_path,
            path,
            size,
        })
    }

    fn snapshot(
        &self,
        dir: &StagingDirectory,
        destination: String,
        cancel: &CancellationToken,
    ) -> AppResult<Blob> {
        check_cancel(cancel)?;
        let mut file = dir.create_file(Path::new(&destination))?;
        let blob = consume_bounded(&self.path, cancel, &mut file, self.size..=self.size)?;
        file.sync_all()?;
        Ok(Blob {
            path: destination,
            ..blob
        })
    }
}

pub(crate) fn prepare_archive_job(
    engine: &FileEngine,
    game_id: &str,
    archive_path: &str,
    source_paths: &[String],
    cancel: &CancellationToken,
) -> AppResult<PreparedArchiveJob> {
    check_cancel(cancel)?;
    if source_paths.len() > MAX_ARCHIVE_PAYLOADS {
        return Err(invalid("RPF 载荷文件数量超出允许范围"));
    }
    let name = archive_path.rsplit(['/', '\\']).next().unwrap_or_default();
    if !(5..=180).contains(&name.chars().count())
        || !Path::new(name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("rpf"))
    {
        return Err(invalid(
            "archive_path 必须以游戏目录中的实际 .rpf 文件结尾；归档内部路径请另传 entry",
        ));
    }

    let game = games::get_game(&engine.db, game_id)?;
    let (edition, root, _) = checked_game_root(engine, &game)?;
    let archive = Source::resolve(engine, &root, archive_path, 16..=MAX_ARCHIVE_BYTES)?;
    check_shape(&archive.path, "archive")?;
    let mut total = archive.size;
    let mut payloads = Vec::new();
    for path in source_paths {
        check_cancel(cancel)?;
        let payload = Source::resolve(engine, &root, path, 0..=MAX_PAYLOAD_BYTES)?;
        total += payload.size;
        if total > MAX_JOB_BYTES {
            return Err(invalid("RPF 输入超过任务配额"));
        }
        payloads.push(payload);
    }

    let parent = jobs(engine)?;
    safe_fs::create_dir_all(&parent)?;
    DiskBudget::new(&parent)?.require(total)?;
    let job_id = uuid::Uuid::new_v4().simple().to_string();
    let dir = StagingDirectory::create(&parent.join(&job_id))?;
    let mut cleanup = dir.cleanup_guard()?;
    dir.create_dir_all(Path::new("inputs"))?;
    let source = archive.snapshot(&dir, "inputs/archive".into(), cancel)?;
    check_shape(&dir.path().join(&source.path), "archive")?;
    let reference = ArchiveReference {
        path: source.path.clone(),
        name: name.into(),
    };
    let mut manifest = ArchiveManifest {
        schema_version: 1,
        job_id: &job_id,
        kind: edition,
        archive_inputs: [ArchiveInput {
            name: name.into(),
            source,
        }],
        payload_inputs: Vec::new(),
    };
    let mut payload_references = Vec::new();
    for (i, payload) in payloads.into_iter().enumerate() {
        let source = payload.snapshot(&dir, format!("inputs/payload-{i}"), cancel)?;
        payload_references.push(PayloadReference {
            game_path: payload.game_path,
            source_path: source.path.clone(),
        });
        manifest.payload_inputs.push(source);
    }

    check_cancel(cancel)?;
    let mut input = dir.create_file(Path::new("input.json"))?;
    serde_json::to_writer(&mut input, &manifest)?;
    input.sync_all()?;
    dir.verify_path()?;
    cleanup.keep();
    Ok(PreparedArchiveJob {
        job_id,
        game_id: game_id.into(),
        edition,
        archive_path: archive.game_path,
        archive: reference,
        payloads: payload_references,
    })
}
