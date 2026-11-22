//! Live iterator for adapter-declared diagnostics. A cursor owns the original
//! scope and decoder state; neither callers nor log content provide paths or offsets.
use super::*;
use crate::{games::pages::Sessions, safe_fs};
use encoding_rs::{DecoderResult, Encoding, UTF_16BE, UTF_16LE, UTF_8};
use std::io::{BufRead, BufReader, Seek};
use tokio_util::sync::CancellationToken;

static LOG_PAGES: Sessions<LogReport> = Sessions::new();
static DIAGNOSIS_PAGES: Sessions<Diagnosis> = Sessions::new();

pub fn read_logs_page(
    engine: &FileEngine,
    game_id: String,
    options: ReadOptions,
    scope: &str,
    cancel: &CancellationToken,
) -> AppResult<LogReport> {
    let request = options.page.clone();
    let worker = engine.clone();
    let (mut page, cursor) = LOG_PAGES.request(
        &engine.db,
        format!("{scope}:{game_id}"),
        &request,
        cancel,
        move |producer| {
            let ctx = context(&worker, &game_id)?;
            let final_page = stream(&ctx, &options, false, &producer.cancel, |page| {
                check_scope(&worker, &ctx)?;
                producer.emit(page)
            })?;
            check_scope(&worker, &ctx)?;
            Ok(final_page)
        },
    )?;
    page.next_cursor = cursor;
    Ok(page)
}

pub fn diagnose_game_page(
    engine: &FileEngine,
    game_id: String,
    options: ReadOptions,
    scope: &str,
    cancel: &CancellationToken,
) -> AppResult<Diagnosis> {
    let request = options.page.clone();
    let worker = engine.clone();
    let (mut page, cursor) = DIAGNOSIS_PAGES.request(
        &engine.db,
        format!("{scope}:{game_id}"),
        &request,
        cancel,
        move |producer| {
            let ctx = context(&worker, &game_id)?;
            let mut startup = None;
            let publish = |page| {
                check_scope(&worker, &ctx)?;
                producer.emit(page)
            };
            let evidence_limit = options.max_evidence.clamp(1, 128);
            let evaluation_options = ReadOptions {
                max_evidence: usize::MAX,
                max_line_chars: options.max_line_chars.min(8192),
                ..options.clone()
            };
            let evaluate = |page| -> AppResult<Diagnosis> {
                producer.checkpoint()?;
                diagnose_collected(
                    #[cfg(test)]
                    &worker,
                    &ctx,
                    &game_id,
                    &evaluation_options,
                    page,
                    #[cfg(test)]
                    false,
                    &producer.cancel,
                )
            };
            let emit = |mut diagnosis: Diagnosis,
                        startup: &mut Option<fabric::FabricEvidence>|
             -> AppResult<Diagnosis> {
                diagnosis = split_evidence(&ctx, diagnosis, evidence_limit, publish)?;
                if let Some(evidence) = diagnosis.fabric_startup.as_mut() {
                    *startup = Some(evidence.clone());
                    // Keep the original full selection internally for disk correlation,
                    // and render at most 64 rows in each response.
                    let mut pieces = std::mem::take(&mut evidence.mods).into_iter();
                    evidence.mods.extend(pieces.by_ref().take(64));
                    if !pieces.as_slice().is_empty() {
                        publish(diagnosis.clone())?;
                        let header = startup.as_ref().unwrap();
                        let mut next = empty_diagnosis(&ctx);
                        while !pieces.as_slice().is_empty() {
                            if next.fabric_startup.is_some() {
                                publish(next)?;
                                next = empty_diagnosis(&ctx);
                            }
                            next.fabric_startup = Some(fabric::FabricEvidence {
                                mods: pieces.by_ref().take(64).collect(),
                                ..header.clone()
                            });
                        }
                        return Ok(next);
                    }
                }
                Ok(diagnosis)
            };
            let final_page = stream(&ctx, &options, true, &producer.cancel, |page| {
                publish(emit(evaluate(page)?, &mut startup)?)
            })?;
            let mut assembled = Some(emit(evaluate(final_page)?, &mut startup)?);
            let last_disk =
                fabric_disk::stream(&worker, &ctx, startup.as_ref(), &producer.cancel, |disk| {
                    let mut page = assembled.take().unwrap_or_else(|| empty_diagnosis(&ctx));
                    page.fabric_disk = Some(disk);
                    publish(page)
                })?;
            let mut result = assembled.unwrap_or_else(|| empty_diagnosis(&ctx));
            result.fabric_disk = last_disk;
            let page = stream_state(&worker, &ctx, &options, &producer.cancel, result, publish)?;
            check_scope(&worker, &ctx)?;
            Ok(page)
        },
    )?;
    page.logs.next_cursor = cursor;
    Ok(page)
}

fn split_evidence(
    ctx: &Context,
    mut page: Diagnosis,
    limit: usize,
    emit: impl Fn(Diagnosis) -> AppResult<()>,
) -> AppResult<Diagnosis> {
    if page.evidence.len() <= limit {
        return Ok(page);
    }
    let causes = std::mem::take(&mut page.causes);
    let mut evidence = std::mem::take(&mut page.evidence).into_iter();
    loop {
        page.evidence.extend(evidence.by_ref().take(limit));
        let ids = page
            .evidence
            .iter()
            .map(|row| row.id.as_str())
            .collect::<BTreeSet<_>>();
        page.causes = causes
            .iter()
            .filter_map(|cause| {
                let mut cause = cause.clone();
                cause.evidence_ids.retain(|id| ids.contains(id.as_str()));
                (!cause.evidence_ids.is_empty()).then_some(cause)
            })
            .collect();
        finish_diagnosis(&mut page);
        if evidence.len() == 0 {
            return Ok(page);
        }
        let mut next = empty_diagnosis(ctx);
        // Keep structure on the final evidence page, where Fabric correlation
        // and Bethesda reporting can consume it once.
        next.fabric_startup = page.fabric_startup.take();
        next.bethesda_plugins = page.bethesda_plugins.take();
        emit(page)?;
        page = next;
    }
}

fn stream_state(
    engine: &FileEngine,
    ctx: &Context,
    options: &ReadOptions,
    cancel: &CancellationToken,
    mut page: Diagnosis,
    mut emit: impl FnMut(Diagnosis) -> AppResult<()>,
) -> AppResult<Diagnosis> {
    let options = ReadOptions {
        max_evidence: options.max_evidence.min(128),
        max_line_chars: options.max_line_chars.min(8192),
        ..options.clone()
    };
    let mut progress = 0;
    let mut checkpoint = |page: &mut Diagnosis| -> AppResult<()> {
        check(cancel)?;
        if page.evidence.len() >= options.max_evidence
            || page.unknowns.len() >= 60
            || progress >= 2048
        {
            let mut current = std::mem::replace(page, empty_diagnosis(ctx));
            finish_diagnosis(&mut current);
            emit(current)?;
            progress = 0;
        }
        progress += 1;
        Ok(())
    };
    inspect_markers(ctx, &options, &mut page, cancel, false, &mut checkpoint)?;
    let mut after = None::<String>;
    loop {
        check(cancel)?;
        let rows = {
            let conn = engine.db.conn();
            let mut statement = conn.prepare(&format!("SELECT id,
                CASE WHEN length(CAST(files_json AS BLOB))<=262144 THEN files_json END,
                trim(dependencies_json)<>'[]' FROM installs WHERE game_id=?1 AND (?2 IS NULL OR id>?2) AND
                CASE WHEN EXISTS(SELECT 1 FROM install_transactions WHERE install_id=installs.id)
                     THEN {} ELSE status END='enabled' ORDER BY id LIMIT 16", crate::install::INSTALL_STATE))?;
            let rows = statement.query_map(rusqlite::params![ctx.game.id, after], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.is_empty() {
            break;
        }
        for (id, files, dependencies) in rows {
            after = Some(id.clone());
            checkpoint(&mut page)?;
            if dependencies {
                page.unknown(format!("安装 {id} 有依赖记录；本诊断不解算版本约束。"));
            }
            let Some(files) = files else {
                page.unknown(format!("安装 {id} 的文件记录超出单记录安全解析大小。"));
                continue;
            };
            let Ok(files) = serde_json::from_str::<Vec<Value>>(&files) else {
                page.unknown(format!("安装 {id} 的 files_json 不是可解释数组。"));
                continue;
            };
            for file in files {
                checkpoint(&mut page)?;
                inspect_install_file(ctx, &options, &mut page, &id, &file);
            }
        }
    }
    let mut after = None::<(String, String)>;
    loop {
        check(cancel)?;
        let rows = {
            let conn = engine.db.conn();
            let mut stmt = conn.prepare("SELECT id,status,created_at FROM file_transactions
                WHERE game_id=?1 AND status IN ('applying','rolling_back','recovery_failed','failed')
                  AND (?2 IS NULL OR (created_at,id)<(?2,?3)) ORDER BY created_at DESC,id DESC LIMIT 32")?;
            let (time, id) = after
                .as_ref()
                .map(|(time, id)| (Some(time), Some(id)))
                .unwrap_or_default();
            let rows = stmt.query_map(rusqlite::params![ctx.game.id, time, id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            rows.collect::<rusqlite::Result<Vec<_>>>()?
        };
        if rows.is_empty() {
            break;
        }
        for (id, status, time) in rows {
            checkpoint(&mut page)?;
            after = Some((time, id.clone()));
            page.add(
                &options,
                CauseKind::Transaction,
                Basis::Observed,
                "database:file_transactions",
                None,
                &format!("{id}: {status}"),
                "database",
                "存在失败或尚未恢复的文件事务；不证明它是本次故障根因",
            );
        }
    }
    finish_diagnosis(&mut page);
    Ok(page)
}

fn check_scope(engine: &FileEngine, original: &Context) -> AppResult<()> {
    let current = context(engine, &original.game.id)?;
    if (
        current.root,
        current.users,
        current.adapter.slug,
        current.adapter.version,
        current.platform,
        current.managed,
    ) != (
        original.root.clone(),
        original.users.clone(),
        original.adapter.slug.clone(),
        original.adapter.version.clone(),
        original.platform,
        original.managed.clone(),
    ) {
        return Err(AppError::Conflict("诊断范围已变化，请重新开始诊断".into()));
    }
    Ok(())
}

fn empty_diagnosis(ctx: &Context) -> Diagnosis {
    Diagnosis {
        game_id: ctx.game.id.clone(),
        status: "unknown".into(),
        logs: base_report(ctx),
        ..Diagnosis::default()
    }
}

fn base_report(ctx: &Context) -> LogReport {
    let mut report = LogReport {
        game_id: ctx.game.id.clone(),
        adapter_slug: ctx.adapter.slug.clone(),
        adapter_version: ctx.adapter.version.clone(),
        limitations: vec![
            "逐页读取适配包声明的日志和顺序文件，不递归日志子目录；续页期间文件变化会停止读取该文件。".into(),
            "证据仅代表当前读取范围，缺少日志不证明缺少加载器；不会执行游戏、加载器、排序器或修复。".into(),
        ],
        ..Default::default()
    };
    if ctx.game.adapter_version.as_deref() != Some(&ctx.adapter.version) {
        report
            .limitations
            .push("登记的适配包版本与当前版本不同；目录解释可能已变化。".into());
    }
    report
}

fn check(cancel: &CancellationToken) -> AppResult<()> {
    if cancel.is_cancelled() {
        Err(AppError::Cancelled)
    } else {
        Ok(())
    }
}

struct LogPages<'a, F> {
    ctx: &'a Context,
    options: &'a ReadOptions,
    cancel: &'a CancellationToken,
    emit: F,
    report: LogReport,
    total_bytes: usize,
    file_bytes: usize,
    files: usize,
    line_chars: usize,
    lines: usize,
    page_lines: usize,
    page_text_bytes: usize,
    inspection: bool,
    scan_entries: usize,
}

impl<F: FnMut(LogReport) -> AppResult<()>> LogPages<'_, F> {
    fn flush(&mut self) -> AppResult<()> {
        check(self.cancel)?;
        self.page_lines = 0;
        self.page_text_bytes = 0;
        let report = std::mem::replace(&mut self.report, base_report(self.ctx));
        (self.emit)(report)
    }

    fn push(&mut self, file: DiagnosticFile) {
        self.page_lines += file.lines.len();
        self.page_text_bytes += file.lines.iter().map(|line| line.text.len()).sum::<usize>();
        self.report.bytes_read += file.bytes_read;
        self.report.truncated |= file.truncated;
        self.report.files.push(file);
    }

    fn output(
        &mut self,
        prefix: &DiagnosticFile,
        mut report: DiagnosticFile,
    ) -> AppResult<DiagnosticFile> {
        report.continues = true;
        self.push(report);
        self.flush()?;
        let mut next = prefix.clone();
        next.continued = true;
        Ok(next)
    }

    fn file(&mut self, path: &Path, kind: FileKind) -> AppResult<()> {
        check(self.cancel)?;
        if self.report.files.len() >= self.files
            || self.report.bytes_read >= self.total_bytes
            || self.page_lines >= self.lines
        {
            self.flush()?;
        }
        match self.stream_file(path, kind) {
            Ok(file) => self.push(file),
            Err(AppError::Cancelled) => return Err(AppError::Cancelled),
            Err(error) => self.push(failed_file(path, kind, error)),
        }
        Ok(())
    }

    fn stream_file(&mut self, path: &Path, kind: FileKind) -> AppResult<DiagnosticFile> {
        self.ctx.guard(path)?;
        let opened = safe_fs::open_regular(path)?;
        self.ctx.guard(path)?;
        safe_fs::verify_regular_at(&opened, path)?;
        let before = opened.metadata()?;
        let mut file = BufReader::with_capacity(16 * 1024, opened);
        let fallback = match kind {
            FileKind::Log => self
                .options
                .legacy_log_encoding
                .map(LegacyLogEncoding::encoding),
            FileKind::LoadOrder
                if matches!(
                    self.ctx.adapter.load_order.kind,
                    LoadOrderKind::BethesdaPlugins
                ) =>
            {
                Some(encoding_rs::WINDOWS_1252)
            }
            _ => None,
        };
        let mut prefix = blank_file(path, kind, ReadStatus::Read, String::new());
        prefix.notes.clear();
        prefix.size = Some(before.len());
        prefix.modified_at = before
            .modified()
            .ok()
            .map(|time| chrono::DateTime::<chrono::Utc>::from(time).to_rfc3339());
        let (encoding, bom) = match encoding(&mut file, fallback, before.len(), self.cancel)? {
            Ok(value) => value,
            Err((status, note)) => {
                prefix.status = status;
                prefix.notes.push(note.into());
                return Ok(prefix);
            }
        };
        let stable = |opened: &fs::File| -> AppResult<()> {
            self.ctx.guard(path)?;
            safe_fs::verify_regular_at(opened, path)?;
            let current = opened.metadata()?;
            if current.len() != before.len() || current.modified()? != before.modified()? {
                return Err(AppError::Conflict(
                    "读取期间文件有变化；请重新开始诊断".into(),
                ));
            }
            Ok(())
        };
        stable(file.get_ref())?;
        prefix.encoding = Some(encoding.name().to_ascii_lowercase());
        if Some(encoding) == fallback {
            prefix
                .notes
                .push("使用调用方选择或顺序文件规定的旧编码。".into());
        }
        let structure = self.inspection
            && kind == FileKind::LoadOrder
            && matches!(
                self.ctx.adapter.load_order.kind,
                LoadOrderKind::Bg3ModSettings
            );
        let mut xml = structure.then(String::new);
        let mut xml_limited = false;
        let mut startup = (self.inspection
            && kind == FileKind::Log
            && self.ctx.adapter.slug == "minecraft-java"
            && path == self.ctx.root.join("logs/latest.log"))
        .then(fabric::Capture::default);
        let mut bethesda = (self.inspection
            && kind == FileKind::LoadOrder
            && matches!(
                self.ctx.adapter.load_order.kind,
                LoadOrderKind::BethesdaPlugins
            ))
        .then(bethesda::Capture::new)
        .transpose()?;
        let mut report = prefix.clone();
        let mut fragment_text_bytes = 0;
        let mut bom_remaining = bom;
        while bom_remaining > 0 {
            let budget = self
                .total_bytes
                .saturating_sub(self.report.bytes_read + report.bytes_read)
                .min(self.file_bytes.saturating_sub(report.bytes_read));
            if budget == 0 {
                report = self.output(&prefix, report)?;
            } else {
                let consumed = budget.min(bom_remaining);
                report.bytes_read += consumed;
                bom_remaining -= consumed;
            }
        }
        let mut decoder = encoding.new_decoder_without_bom_handling();
        let mut position = bom as u64;
        let mut physical = PhysicalLine::default();
        let mut utf8 = String::new();
        loop {
            check(self.cancel)?;
            let input = file.fill_buf()?;
            let available = (before.len() - position).min(input.len() as u64) as usize;
            if available == 0 && position < before.len() {
                return Err(AppError::Conflict(
                    "读取期间文件缩短；请重新开始诊断".into(),
                ));
            }
            let budget = (self
                .total_bytes
                .saturating_sub(self.report.bytes_read + report.bytes_read))
            .min(self.file_bytes.saturating_sub(report.bytes_read));
            if budget == 0 && available > 0 {
                stable(file.get_ref())?;
                report = self.output(&prefix, report)?;
                fragment_text_bytes = 0;
                continue;
            }
            let count = available.min(budget);
            let eof = position + count as u64 == before.len();
            let data = &input[..count];
            utf8.clear();
            utf8.reserve(
                decoder
                    .max_utf8_buffer_length_without_replacement(count)
                    .unwrap_or(64 * 1024),
            );
            let (decoded, consumed) =
                decoder.decode_to_string_without_replacement(data, &mut utf8, eof);
            if decoded != DecoderResult::InputEmpty || utf8.bytes().any(text::is_binary_byte) {
                report.bytes_read += count;
                report.lines.clear();
                report.status = if decoded == DecoderResult::InputEmpty {
                    ReadStatus::Binary
                } else {
                    ReadStatus::UnknownEncoding
                };
                report
                    .notes
                    .push("检测到二进制内容或无效编码，未从此批次生成证据。".into());
                return Ok(report);
            }
            file.consume(consumed);
            position += consumed as u64;
            report.bytes_read += consumed;
            if let Some(text) = xml.as_mut() {
                if text.len() + utf8.len() <= bg3::MAX_BYTES {
                    text.push_str(&utf8);
                } else {
                    xml = None;
                    xml_limited = true;
                }
            }
            for ch in utf8.chars() {
                if let Some(line) = physical.consume(ch, self.line_chars) {
                    if self.page_lines + report.lines.len() >= self.lines
                        || self.page_text_bytes + fragment_text_bytes + line.text.len() > 512 * 1024
                    {
                        stable(file.get_ref())?;
                        report = self.output(&prefix, report)?;
                        fragment_text_bytes = 0;
                    }
                    fragment_text_bytes += line.text.len();
                    if let Some(capture) = startup.as_mut() {
                        capture.observe(&line);
                    }
                    if let Some(capture) = bethesda.as_mut() {
                        capture.observe(&line)?;
                    }
                    report.truncated |= line.truncated;
                    report.lines.push(line);
                }
            }
            if eof {
                break;
            }
        }
        if physical.in_progress {
            let line = physical.finish();
            if self.page_lines + report.lines.len() >= self.lines
                || self.page_text_bytes + fragment_text_bytes + line.text.len() > 512 * 1024
            {
                stable(file.get_ref())?;
                report = self.output(&prefix, report)?;
            }
            if let Some(capture) = startup.as_mut() {
                capture.observe(&line);
            }
            if let Some(capture) = bethesda.as_mut() {
                capture.observe(&line)?;
            }
            report.truncated |= line.truncated;
            report.lines.push(line);
        }
        stable(file.get_ref())?;
        if let Some(capture) = startup {
            report.fabric = Some(capture.finish(&report));
        }
        if let Some(capture) = bethesda {
            let mut view = report.clone();
            view.lines.clear();
            let (evidence, findings) =
                capture.batches(self.ctx, &view, self.cancel, |evidence, findings| {
                    stable(file.get_ref())?;
                    report.bethesda = Some((evidence, findings));
                    report =
                        self.output(&prefix, std::mem::replace(&mut report, prefix.clone()))?;
                    Ok(())
                })?;
            stable(file.get_ref())?;
            report.bethesda = Some((evidence, findings));
        }
        if structure {
            if xml_limited {
                report.bg3 = Some(Err("BG3 XML 超出安全解析大小"));
            } else {
                match bg3::inspect(xml.as_deref().unwrap_or_default(), usize::MAX) {
                    Err(reason) => report.bg3 = Some(Err(reason)),
                    Ok(findings) => {
                        let batch = self.options.max_evidence.min(128);
                        let mut next = findings.into_iter();
                        loop {
                            report.bg3 = Some(Ok(next.by_ref().take(batch).collect()));
                            if next.len() == 0 {
                                break;
                            }
                            report = self.output(&prefix, report)?;
                        }
                    }
                }
            }
        }
        Ok(report)
    }
}

#[derive(Default)]
struct PhysicalLine {
    text: String,
    chars: usize,
    number: usize,
    skip_lf: bool,
    in_progress: bool,
    clipped: bool,
}
impl PhysicalLine {
    fn consume(&mut self, ch: char, limit: usize) -> Option<LogLine> {
        if std::mem::take(&mut self.skip_lf) && ch == '\n' {
            return None;
        }
        if matches!(ch, '\r' | '\n') {
            self.skip_lf = ch == '\r';
            return Some(self.finish());
        }
        self.in_progress = true;
        if self.chars < limit {
            if (ch.is_control() && ch != '\t')
                || matches!(ch, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
            {
                for ch in ch.escape_unicode() {
                    if self.chars == limit {
                        self.clipped = true;
                        break;
                    }
                    self.text.push(ch);
                    self.chars += 1;
                }
            } else {
                self.text.push(ch);
                self.chars += 1;
            }
        } else {
            self.clipped = true;
        }
        None
    }
    fn finish(&mut self) -> LogLine {
        self.number += 1;
        self.chars = 0;
        self.in_progress = false;
        LogLine {
            number: self.number,
            text: std::mem::take(&mut self.text),
            truncated: std::mem::take(&mut self.clipped),
        }
    }
}

type EncodingChoice = Result<(&'static Encoding, usize), (ReadStatus, &'static str)>;

fn encoding(
    file: &mut BufReader<fs::File>,
    fallback: Option<&'static Encoding>,
    size: u64,
    cancel: &CancellationToken,
) -> AppResult<EncodingChoice> {
    let input = file.fill_buf()?;
    let choice = match input {
        [0xff, 0xfe, 0, 0, ..] | [0, 0, 0xfe, 0xff, ..] => {
            return Ok(Err((ReadStatus::UnknownEncoding, "不支持 UTF-32 日志")))
        }
        [0xff, 0xfe, ..] => (UTF_16LE, 2),
        [0xfe, 0xff, ..] => (UTF_16BE, 2),
        [0xef, 0xbb, 0xbf, ..] => (UTF_8, 3),
        _ => {
            let mut selected = UTF_8;
            // If the caller supplies a fallback, select it on malformed UTF-8,
            // even when the first non-UTF-8 byte occurs on a later page.
            if let Some(fallback) = fallback {
                let mut trial = UTF_8.new_decoder_without_bom_handling();
                let mut output = String::with_capacity(64 * 1024);
                let mut rest = size;
                while rest > 0 {
                    check(cancel)?;
                    let input = file.fill_buf()?;
                    let count = rest.min(input.len() as u64) as usize;
                    if count == 0 {
                        return Err(AppError::Conflict("文件在编码检查期间缩短".into()));
                    }
                    output.clear();
                    let (result, _) = trial.decode_to_string_without_replacement(
                        &input[..count],
                        &mut output,
                        rest == count as u64,
                    );
                    if result != DecoderResult::InputEmpty {
                        selected = fallback;
                        break;
                    }
                    file.consume(count);
                    rest -= count as u64;
                }
                file.rewind()?;
            }
            (selected, 0)
        }
    };
    if choice.1 > 0 {
        file.consume(choice.1);
    }
    Ok(Ok(choice))
}

fn stream(
    ctx: &Context,
    options: &ReadOptions,
    inspection: bool,
    cancel: &CancellationToken,
    emit: impl FnMut(LogReport) -> AppResult<()>,
) -> AppResult<LogReport> {
    if [
        options.max_files,
        options.max_bytes_per_file,
        options.max_total_bytes,
        options.max_lines_per_file,
        options.max_line_chars,
        options.max_directory_entries,
        options.max_evidence,
    ]
    .contains(&0)
    {
        return Err(AppError::Invalid("诊断分页参数必须为正数".into()));
    }
    let mut pages = LogPages {
        ctx,
        options,
        cancel,
        emit,
        report: base_report(ctx),
        total_bytes: options.max_total_bytes.min(4 * 1024 * 1024),
        file_bytes: options.max_bytes_per_file.min(1024 * 1024),
        files: options.max_files.min(32),
        lines: if inspection {
            options
                .max_lines_per_file
                .min(options.max_evidence.min(128))
        } else {
            options.max_lines_per_file.min(10_000)
        },
        line_chars: options.max_line_chars.min(8192),
        page_lines: 0,
        page_text_bytes: 0,
        inspection,
        scan_entries: 0,
    };
    // An adapter can declare a single file and its parent directory. Disk-backed
    // deduplication doesn't accumulate paths in RAM for large log directories.
    let temp = tempfile::tempdir()?;
    let seen = rusqlite::Connection::open(temp.path().join("diagnostics.sqlite"))?;
    seen.execute_batch("PRAGMA cache_size=-1024; PRAGMA synchronous=OFF; PRAGMA journal_mode=OFF; CREATE TABLE seen(path TEXT PRIMARY KEY); BEGIN;")?;
    for (kind, declarations) in [
        (FileKind::LoadOrder, &ctx.adapter.load_order.paths),
        (FileKind::Log, &ctx.adapter.log_paths),
    ] {
        for declaration in declarations
            .iter()
            .filter(|p| p.platforms.contains(&ctx.platform))
        {
            check(cancel)?;
            let path = match ctx.resolve(declaration) {
                Ok(path) => path,
                Err(error) => {
                    pages.report.issue(declaration, error.to_string());
                    continue;
                }
            };
            if kind == FileKind::Log && path.is_dir() {
                let entries = stable_directory(&path).and_then(|_| {
                    let identity = safe_fs::identity(&path)?
                        .ok_or_else(|| AppError::Denied("无法验证日志目录".into()))?;
                    safe_fs::directory_entries(&path, &identity)
                });
                match entries {
                    Ok(entries) => {
                        for entry in entries {
                            check(cancel)?;
                            pages.scan_entries += 1;
                            if pages.scan_entries > options.max_directory_entries.min(2048) {
                                pages.scan_entries = 1;
                                pages.flush()?;
                            }
                            let (name, directory) = match entry {
                                Ok(entry) => entry,
                                Err(error) => {
                                    pages.report.issue(declaration, error.to_string());
                                    continue;
                                }
                            };
                            let file = path.join(name);
                            if directory
                                || !file.extension().and_then(|ext| ext.to_str()).is_some_and(
                                    |ext| {
                                        ext.eq_ignore_ascii_case("log")
                                            || ext.eq_ignore_ascii_case("txt")
                                    },
                                )
                            {
                                continue;
                            }
                            if ctx.adapter.slug == "baldurs-gate-3"
                                && declaration.root == "launcher-cache"
                                && !file
                                    .file_name()
                                    .unwrap_or_default()
                                    .to_string_lossy()
                                    .to_ascii_lowercase()
                                    .contains("alteredfiles")
                            {
                                continue;
                            }
                            if let Some(file_text) = file.to_str() {
                                if seen.execute(
                                    "INSERT OR IGNORE INTO seen(path) VALUES(?1)",
                                    [file_text],
                                )? > 0
                                {
                                    pages.file(&file, kind)?;
                                }
                            }
                        }
                    }
                    Err(error) => pages.report.issue(declaration, error.to_string()),
                }
            } else if let Some(path_text) = path.to_str() {
                if seen.execute("INSERT OR IGNORE INTO seen(path) VALUES(?1)", [path_text])? > 0 {
                    pages.file(&path, kind)?;
                }
            }
        }
    }
    Ok(pages.report)
}
