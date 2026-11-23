//! 7-Zip only returns bytes on stdout. It is never given the destination, an
//! archive-controlled filename, or permission to choose where a member goes.
use super::{
    process_tar_stream, unsafe_archive, ArchiveEntry, CheckedInput, Processing, Work, RAR4_MAGIC,
    RAR5_MAGIC,
};
use crate::error::{AppError, AppResult};
use crate::safe_fs::DiskBudget;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{BufReader, Read, Seek, Write},
    path::PathBuf,
    process::{Child, Stdio},
    sync::mpsc,
    time::Duration,
};

pub(super) struct Snapshot {
    dir: tempfile::TempDir,
    pub(super) path: PathBuf,
    signature: [u8; 8],
}

impl Snapshot {
    pub(super) fn create(
        file: &mut File,
        expected: Option<&str>,
        work: &Work<'_>,
    ) -> AppResult<Self> {
        work.check()?;
        let mut builder = tempfile::Builder::new();
        builder.prefix("amm-extract-");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            builder.permissions(std::fs::Permissions::from_mode(0o700));
        }
        let dir = builder.tempdir()?;
        let mut budget = DiskBudget::new(dir.path())?;
        let mut remaining = file.metadata()?.len();
        budget.require(remaining)?;
        let path = dir.path().join("input");
        let mut copy = File::create_new(&path)?;
        file.rewind()?;
        let mut signature = [0; 8];
        let mut digest = expected.map(|_| Sha256::new());
        let mut buf = [0; 64 * 1024];
        let mut sig_len = 0;
        while remaining > 0 {
            work.check()?;
            let size = remaining.min(buf.len() as u64) as usize;
            file.read_exact(&mut buf[..size])?;
            budget.charge(size as u64)?;
            copy.write_all(&buf[..size])?;
            let n = (signature.len() - sig_len).min(size);
            signature[sig_len..sig_len + n].copy_from_slice(&buf[..n]);
            sig_len += n;
            if let Some(digest) = &mut digest {
                digest.update(&buf[..size]);
            }
            remaining -= size as u64;
        }
        if file.read(&mut buf[..1])? != 0 {
            return Err(AppError::Conflict("归档在复制期间已变化".into()));
        }
        if let (Some(hash), Some(expected)) = (digest, expected) {
            if !hex::encode(hash.finalize()).eq_ignore_ascii_case(expected) {
                return Err(AppError::Conflict("归档副本 SHA-256 与预期不符".into()));
            }
        }
        copy.flush()?;
        Ok(Self {
            path,
            signature,
            dir,
        })
    }
}

struct Decoder(Child);
impl Drop for Decoder {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Snapshot {
    fn rar_type(&self) -> AppResult<&'static str> {
        if self.signature.starts_with(RAR5_MAGIC) {
            Ok("-trar5")
        } else if self.signature.starts_with(RAR4_MAGIC) {
            Ok("-trar")
        } else {
            Err(unsafe_archive("RAR 文件头无效"))
        }
    }
}

pub(super) fn process_rar(snapshot: &Snapshot, output: &mut Processing<'_>) -> AppResult<()> {
    let file = File::open(&snapshot.path)?;
    let length = file.metadata()?.len();
    let mut input = CheckedInput::new(file, output.work, length);
    let inventory = super::archive_rar::inventory(&mut input, length)?;
    let mut remaining = output.budget.limit;
    for (entry, _) in &inventory {
        super::check_entry(entry)?;
        remaining = remaining
            .checked_sub(entry.size)
            .ok_or_else(|| unsafe_archive("RAR 声明解压大小超过可用磁盘预算"))?;
    }
    with_decoder(snapshot, snapshot.rar_type()?, output.work, |stream| {
        for (entry, crc) in inventory {
            let mut reader = flate2::CrcReader::new((&mut *stream).take(entry.size));
            output.entry(entry, &mut reader)?;
            if crc.is_some_and(|expected| expected != reader.crc().sum()) {
                return Err(unsafe_archive("RAR 条目 CRC32 校验失败"));
            }
        }
        require_eof(stream)
    })
}

fn require_eof(stream: &mut dyn Read) -> AppResult<()> {
    if stream.read(&mut [0; 1])? != 0 {
        return Err(unsafe_archive("7-Zip 输出超过已验证的归档大小"));
    }
    Ok(())
}

pub(super) fn extract(
    snapshot: &Snapshot,
    format: &str,
    entries: &[ArchiveEntry],
    output: &mut Processing<'_>,
) -> AppResult<()> {
    let kind = match format {
        "zip" => "-tzip",
        "7z" => "-t7z",
        "rar" => snapshot.rar_type()?,
        "tar" if snapshot.signature.starts_with(b"\x1f\x8b") => "-tgzip",
        "tar" if snapshot.signature.starts_with(b"BZh") => "-tbzip2",
        "tar" => "-ttar",
        _ => return Err(unsafe_archive("不支持的 7-Zip 解压类型")),
    };
    with_decoder(snapshot, kind, output.work, |stream| {
        if matches!(kind, "-tgzip" | "-tbzip2") {
            return process_tar_stream(stream, output);
        }
        for entry in entries {
            output.entry(entry.clone(), &mut (&mut *stream).take(entry.size))?;
        }
        require_eof(stream)
    })
}

fn with_decoder(
    snapshot: &Snapshot,
    kind: &str,
    work: &Work<'_>,
    consume: impl FnOnce(&mut dyn Read) -> AppResult<()>,
) -> AppResult<()> {
    let mut cmd = crate::seven_zip::command()?;
    cmd.current_dir(snapshot.dir.path())
        .args(["x", kind, "-so", "-bso0", "-bsp0", "-mmt1"]);
    if matches!(kind, "-trar" | "-trar5") {
        cmd.arg("-smemx256m");
    }
    let mut child = Decoder(
        cmd.args(["--", "input"])
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?,
    );
    let stdout = child.0.stdout.take().expect("requested piped stdout");
    // If the decoder is busy before its next write, a blocked pipe read alone
    // cannot notice cancellation. The monitor owns and reaps this child only.
    std::thread::scope(|scope| {
        let (stop, abort) = mpsc::channel();
        let monitor =
            std::thread::Builder::new().spawn_scoped(scope, move || -> AppResult<_> {
                loop {
                    work.check()?;
                    if let Some(status) = child.0.try_wait()? {
                        return Ok(status);
                    }
                    // Take the whole guard into the closure, not just its Child field.
                    match abort.recv_timeout(Duration::from_millis(20)) {
                        Err(mpsc::RecvTimeoutError::Timeout) => continue,
                        _ => {
                            drop(child);
                            return Err(unsafe_archive("已停止 7-Zip 解压"));
                        }
                    }
                }
            })?;
        let mut stream = BufReader::new(stdout);
        let decoded = consume(&mut stream);
        drop(stream);
        if decoded.is_err() {
            let _ = stop.send(());
        }
        let result = monitor
            .join()
            .map_err(|_| AppError::Other("7-Zip 监控线程异常结束".into()))?;
        work.check()?;
        decoded?;
        let status = result?;
        if !status.success() {
            return Err(unsafe_archive(format!("7-Zip 解压失败（{status}）")));
        }
        Ok(())
    })
}
