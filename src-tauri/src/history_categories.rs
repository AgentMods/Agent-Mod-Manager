//! Conservative classification of original intent, shared by insertion and the
//! one-time upgrade. Never derive a category from later assistant/tool text.
use crate::error::AppResult;
use regex::Regex;
use rusqlite::{params, Connection};
use std::sync::LazyLock;

pub(super) fn fixed_title_category(title: &str) -> Option<&'static str> {
    let title = title.trim();
    for (prefix, category) in [
        ("重新安装 Mod", "install"),
        ("安装 Mod", "install"),
        ("卸载 Mod", "uninstall"),
        ("删除 Mod", "remove"),
        ("创建适配包", "adapter"),
        ("修改适配包", "adapter"),
        ("更新适配包", "adapter"),
    ] {
        if let Some(rest) = title.strip_prefix(prefix) {
            if rest.is_empty() || rest.starts_with(['：', ':']) {
                return Some(category);
            }
        }
    }
    None
}

pub(super) fn request_category(content: &str) -> Option<&'static str> {
    // Only the opening request is evidence. Instructions/quotes later in a
    // question, large pasted logs and subsequent turns are not task intent.
    let opening: String = content.trim().chars().take(2048).collect();
    let opening = opening.split(['。', '！', '!', '\n']).next()?.trim();
    static INFORMATIONAL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)怎么|如何|怎样|为什么|为何|是否|能否|可不可以|可以吗|能不能|会不会|会影响|有什么|是什么|有哪些|[?？]|吗[，,；;\s]*$|不要|不想|别帮|无需|不用|先不|暂不|解释|介绍|区别|教程|原理|步骤|意味着|告诉我|知道|了解|分析|检查|排查|诊断|看看|评价|总结|建议|推荐|安装失败|卸载失败|what\b|how\b|why\b|explain\b|do not\b|don't\b").unwrap()
    });
    if INFORMATIONAL.is_match(opening) {
        return None;
    }
    // Exact desktop request templates predate categories. Removal includes an
    // uninstall step, so the final requested operation takes precedence.
    if opening.starts_with("我已在游戏详情中明确选择") {
        return if opening.contains("卸载并从管理列表删除") {
            Some("remove")
        } else if opening.contains("卸载并保留在管理中") {
            Some("uninstall")
        } else if opening.contains("为此游戏安装这一项 Mod") {
            Some("install")
        } else {
            None
        };
    }
    static POLITE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)^(?:(?:请|麻烦|帮我|替我|我想要|我想|我要|给我|please\s+)\s*)*").unwrap()
    });
    let opening = POLITE.replace(opening, "");
    static ADAPTER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:(?:为|给)[^，,；;：:]{1,40})?(?:创建|新建|制作|修改|更新|编辑)(?:一个)?(?:游戏)?(?:「[^」]{1,40}」)?适配包(?:[（(，,；;\s]|$)|^(?:把|将)「[^」]{1,40}」适配到管理器[，,；;\s]*$",
        )
        .unwrap()
    });
    if ADAPTER.is_match(&opening) {
        return Some("adapter");
    }
    // Parse a command, not a verb anywhere in a sentence. These are bounded
    // command connectors, including the screenshot requests used in history.
    static OPERATION: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)^(?:(?:在|从|根据)?截图(?:中|里|上)?)?(?:(?:下载|查找|搜索)并)?\s*(?P<action>卸载并删除|重新安装|安装|重装|卸载|删除|移除|reinstall\b|install\b|uninstall\b|remove\b|delete\b)\s*(?P<target>[^，,；;：:]{1,64})(?:[，,；;：:].*)?$"
    ).unwrap()
    });
    let command = OPERATION.captures(&opening)?;
    let target = command.name("target")?.as_str().trim();
    static NON_MOD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
        r"(?i)文件|日志|目录|文件夹|存档|适配包|驱动|浏览器|管理器|工具|\.log\b|\bfile\b|\bfolder\b|\bdirectory\b|\blogs?\b|列出|列举|列出来|列表|内容|流程|方式|指南|说明|后果|的记录"
    ).unwrap()
    });
    if target.starts_with(['后', '前', '过', '了', '的']) || NON_MOD.is_match(target) {
        return None;
    }
    static MOD_CONTEXT: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i)(?:^|[^a-z])mods?(?:$|[^a-z])|模组|归档|安装记录").unwrap()
    });
    let action = command.name("action")?.as_str().to_ascii_lowercase();
    if !MOD_CONTEXT.is_match(target) {
        return None;
    }
    match action.as_str() {
        "卸载并删除" | "删除" | "移除" | "remove" | "delete" => Some("remove"),
        "卸载" | "uninstall" => Some("uninstall"),
        _ => Some("install"),
    }
}

pub(crate) fn backfill_conversation_categories(conn: &Connection) -> AppResult<()> {
    // The partial user-message index bounds each lookup to one row; never load
    // whole transcripts. This runs inside the schema migration transaction.
    let mut stmt = conn.prepare(
        "SELECT c.id,c.title,(SELECT substr(m.content,1,2048) FROM messages m
         WHERE m.conversation_id=c.id AND m.role='user' ORDER BY m.rowid LIMIT 1)
         FROM conversations c WHERE c.category IS NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (id, title, request) = row?;
        if let Some(category) = request
            .as_deref()
            .and_then(request_category)
            .or_else(|| fixed_title_category(&title))
        {
            conn.execute(
                "UPDATE conversations SET category=?2 WHERE id=?1 AND category IS NULL",
                params![id, category],
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::Db, history::History};

    #[test]
    fn original_request_templates_and_direct_operations() {
        for (request, category) in [
            ("我已在游戏详情中明确选择为此游戏安装这一项 Mod。目标（精确 ID）：{}", "install"),
            ("我已在游戏详情中明确选择卸载并保留在管理中这一条安装记录。目标：{}", "uninstall"),
            ("我已在游戏详情中明确选择卸载并从管理列表删除这一条安装记录。", "remove"),
            ("请为管理中的游戏「Minecraft」创建适配包，包含游戏封面和游戏 Skill。校验通过后安装并绑定到这款游戏。", "adapter"),
            ("我想创建一个游戏适配包，包含游戏封面和游戏 Skill。若目标游戏不明确，请先向我确认。", "adapter"),
            ("我想修改「Minecraft」适配包（minecraft）。请先读取现有适配包，再和我确认要修改的内容。", "adapter"),
            ("帮我重新安装这个 Mod", "install"),
            ("请重装这个 Mod", "install"),
            ("请在截图中查找并安装这些 Mod", "install"),
            ("请下载并安装这个 Mod", "install"),
            ("卸载这个 Mod", "uninstall"),
            ("请卸载并删除这个 Mod", "remove"),
            ("删除这条安装记录", "remove"),
            ("Please reinstall this mod", "install"),
            ("更新适配包", "adapter"),
            ("将「Dungeons」适配到管理器", "adapter"),
            ("安装这个模组", "install"),
            ("卸载这条安装记录", "uninstall"),
            ("删除这个归档", "remove"),
        ] {
            assert_eq!(request_category(request), Some(category), "{request}");
        }
    }

    #[test]
    fn questions_negation_and_mentions_remain_uncategorized() {
        for request in [
            "你好",
            "如何安装 Mod？",
            "安装 Mod 的原理是什么",
            "卸载和删除的区别",
            "请介绍适配包",
            "我想知道怎么创建适配包",
            "请不要删除这个文件",
            "不要安装",
            "安装后为什么游戏闪退",
            "帮我分析安装失败的日志",
            "请检查卸载记录",
            "安装 Mod 有什么风险",
            "删除 Mod 会影响存档",
            "请把安装流程总结一下",
            "截图中有哪些 Mod？",
            "请搜索 Sodium",
            "请搜索已安装的 Mod",
            "请创建安装记录",
            "请下载之前删除的 Mod",
            "删除这个文件",
            "删除日志",
            "安装 Python",
            "把刚安装的内容列出来",
            "把刚安装的 Mod 列出来",
            "请删除 Mod 日志",
            "请安装 Mod 管理器",
            "请安装 Python 的 Mod 工具",
            "请下载并安装 Python",
            "请删除 Sodium.log",
            "请下载并安装一些好玩的游戏",
            "请为这个游戏检查数据然后考虑安装 Mod",
            "请将这个文件复制到 Mod 目录并安装 Python",
            "请创建一个文件来描述适配包",
            "我想修改文件里的适配包说明",
            "请更新适配包日志",
            "安装这个 Mod 的日志",
            "卸载后保留 Mod",
            "这是文档：安装 Mod",
            "请解释安装步骤。帮我安装 Mod",
            "先介绍游戏。请安装 Mod",
            "\"帮我安装 Mod\"是什么意思？",
        ] {
            assert_eq!(request_category(request), None, "{request}");
        }
        assert_eq!(fixed_title_category("普通安装问答"), None);
    }

    #[test]
    fn explicit_category_validation_wire_format_and_legacy_creation() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(Db::open(&dir.path().join("history.db")).unwrap());
        for category in ["install", "uninstall", "remove", "adapter"] {
            let conv = history
                .create_conversation_with_category("安装 Mod：A", None, Some(category))
                .unwrap();
            assert_eq!(conv.category.as_deref(), Some(category));
            assert_eq!(serde_json::to_value(conv).unwrap()["category"], category);
        }
        let conv = history.create_conversation("", None).unwrap();
        assert!(serde_json::to_value(conv).unwrap()["category"].is_null());
        assert!(history
            .create_conversation_with_category("", None, Some("invalid"))
            .is_err());
        assert!(history
            .create_conversation_with_category("", None, Some(""))
            .is_err());
    }

    #[test]
    fn first_user_intent_is_durable_across_rename_archive_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let id;
        {
            let history = History::new(Db::open(&path).unwrap());
            id = history.create_conversation("", None).unwrap().id;
            history
                .append_message(&id, "assistant", "删除 Mod", None, None)
                .unwrap();
            history
                .append_message(&id, "user", "请在截图中查找并安装这些 Mod", None, None)
                .unwrap();
            history.rename_conversation(&id, "闲聊").unwrap();
            history.archive_conversation(&id, true).unwrap();
            history
                .append_message(&id, "user", "删除 Mod", None, None)
                .unwrap();
            assert_eq!(
                history.list_conversations(true).unwrap()[0]
                    .category
                    .as_deref(),
                Some("install")
            );
        }
        let history = History::new(Db::open(&path).unwrap());
        assert_eq!(
            history
                .get_conversation(&id)
                .unwrap()
                .conversation
                .category
                .as_deref(),
            Some("install")
        );
        let plain = history.create_conversation("", None).unwrap();
        history
            .append_message(&plain.id, "user", "如何安装 Mod？", None, None)
            .unwrap();
        history
            .append_message(&plain.id, "user", "请安装 Mod", None, None)
            .unwrap();
        history
            .rename_conversation(&plain.id, "安装 Mod：A")
            .unwrap();
        assert_eq!(
            history
                .get_conversation(&plain.id)
                .unwrap()
                .conversation
                .category,
            None
        );
    }

    #[test]
    fn explicit_intent_and_rewound_first_message_keep_the_selected_category() {
        let dir = tempfile::tempdir().unwrap();
        let history = History::new(Db::open(&dir.path().join("history.db")).unwrap());
        let conv = history
            .create_conversation_with_category("", None, Some("adapter"))
            .unwrap();
        let user = history
            .append_message(&conv.id, "user", "请安装 Mod", None, None)
            .unwrap();
        {
            let mut conn = history.db.conn();
            let tx = conn.transaction().unwrap();
            History::restart_user_message(&tx, &conv.id, &user.id, Some(("请删除 Mod", None)))
                .unwrap();
            tx.commit().unwrap();
        }
        assert_eq!(
            history
                .get_conversation(&conv.id)
                .unwrap()
                .conversation
                .category
                .as_deref(),
            Some("adapter")
        );
        // Classification participates in the caller's transaction.
        let pending = history.create_conversation("", None).unwrap();
        {
            let mut conn = history.db.conn();
            let tx = conn.transaction().unwrap();
            History::insert_message(&tx, &pending.id, "user", "请安装 Mod", None, None, vec![])
                .unwrap();
            tx.rollback().unwrap();
        }
        let detail = history.get_conversation(&pending.id).unwrap();
        assert!(detail.conversation.category.is_none());
        assert!(detail.messages.is_empty());
    }

    #[test]
    fn upgrade_backfills_original_requests_and_fixed_titles_once() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.db");
        let mut expected = Vec::new();
        {
            let history = History::new(Db::open(&path).unwrap());
            for (title, request, category) in [
                ("自定义名称", "请为我的游戏创建适配包", Some("adapter")),
                ("闲聊", "帮我重新安装 Mod", Some("install")),
                ("卸载 Mod：A", "", Some("uninstall")),
                ("删除 Mod：B", "", Some("remove")),
                ("新对话", "如何安装 Mod？", None),
                ("自定义名称", "你好", None),
                ("新对话", "删除这个文件", None),
                ("新对话", "删除日志", None),
                ("新对话", "安装 Python", None),
                ("新对话", "把刚安装的内容列出来", None),
                ("闲聊", "将「Dungeons」适配到管理器", Some("adapter")),
            ] {
                let conv = history.create_conversation(title, None).unwrap();
                if !request.is_empty() {
                    history
                        .append_message(&conv.id, "user", request, None, None)
                        .unwrap();
                }
                // Later user/assistant messages must not be migration evidence.
                history
                    .append_message(&conv.id, "assistant", "请删除所有 Mod", None, None)
                    .unwrap();
                if !request.is_empty() {
                    history
                        .append_message(&conv.id, "user", "请卸载 Mod", None, None)
                        .unwrap();
                }
                history.archive_conversation(&conv.id, true).unwrap();
                expected.push((conv.id, category));
            }
            history
                .db
                .conn()
                .execute_batch(
                    "ALTER TABLE conversations DROP COLUMN category; PRAGMA user_version=19;",
                )
                .unwrap();
        }
        {
            let history = History::new(Db::open(&path).unwrap());
            for (id, category) in &expected {
                assert_eq!(
                    history
                        .get_conversation(id)
                        .unwrap()
                        .conversation
                        .category
                        .as_deref(),
                    *category
                );
                history.rename_conversation(id, "安装 Mod：改名").unwrap();
            }
            let version: i64 = history
                .db
                .conn()
                .pragma_query_value(None, "user_version", |r| r.get(0))
                .unwrap();
            assert_eq!(version, crate::db::SCHEMA_VERSION);
        }
        let history = History::new(Db::open(&path).unwrap());
        for (id, category) in expected {
            assert_eq!(
                history
                    .get_conversation(&id)
                    .unwrap()
                    .conversation
                    .category
                    .as_deref(),
                category
            );
        }
    }
}
