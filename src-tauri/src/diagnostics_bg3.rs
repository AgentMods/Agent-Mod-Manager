use std::collections::HashSet;

use roxmltree::{Document, Error, Node, ParsingOptions};
use uuid::Uuid;

const MAX_BYTES: usize = 1024 * 1024;
const MAX_NODES: u32 = 32_768;
const LIMIT: &str = "modsettings.lsx 超出 XML 检查上限";
const DTD: &str = "modsettings.lsx 包含不支持的 XML DTD";
const MALFORMED: &str = "modsettings.lsx 不是完整、格式正确的 XML";
const UNSUPPORTED: &str = "modsettings.lsx 的 ModuleSettings 结构不受支持";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Finding {
    pub line: usize,
    pub summary: &'static str,
}

/// Checks only the structure and UUIDs visible in this complete, decoded LSX.
pub(super) fn inspect(text: &str, max_findings: usize) -> Result<Vec<Finding>, &'static str> {
    if text.len() > MAX_BYTES {
        return Err(LIMIT);
    }
    let doc = Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
            entity_resolver: None,
        },
    )
    .map_err(|error| match error {
        Error::DtdDetected => DTD,
        Error::NodesLimitReached => LIMIT,
        _ => MALFORMED,
    })?;

    let save = doc.root_element();
    if !tag(save, "save") || has_text(save) {
        return Err(UNSUPPORTED);
    }
    let region =
        unique_child(save, |n| named(n, "region", "ModuleSettings"))?.ok_or(UNSUPPORTED)?;
    let mut region_elements = elements(region);
    let root = region_elements.next().ok_or(UNSUPPORTED)?;
    if !named(root, "node", "root") || region_elements.next().is_some() || has_text(region) {
        return Err(UNSUPPORTED);
    }
    let lists = children_container(root)?.ok_or(UNSUPPORTED)?;
    if elements(lists).any(|n| !tag(n, "node") || n.attribute("id").is_none_or(str::is_empty)) {
        return Err(UNSUPPORTED);
    }

    // BG3MM 1.0.10.0 emits both lists; 1.0.12.9 emits only Mods.
    let mods = unique_child(lists, |n| named(n, "node", "Mods"))?.ok_or(UNSUPPORTED)?;
    let mods = list_entries(mods, "ModuleShortDesc")?;
    let order = unique_child(lists, |n| named(n, "node", "ModOrder"))?;
    let order = match order {
        Some(order) => list_entries(order, "Module")?,
        None => None,
    };

    let mut findings = Vec::new();
    if max_findings == 0 {
        return Ok(findings);
    }

    let mut mods_keys = HashSet::new();
    let mut mods_complete = true;
    if order.is_some() {
        for entry in entries(mods) {
            match uuid(entry) {
                Ok((_, id)) => {
                    mods_keys.insert(id);
                }
                Err(_) => mods_complete = false,
            }
        }
    }

    let mut lists = [(mods, false), (order, true)];
    lists.sort_by_key(|(children, _)| children.map(|n| n.range().start));
    let mut lines = PhysicalLines::new(text);
    for (children, is_order) in lists {
        let mut seen = HashSet::new();
        for entry in entries(children) {
            let (node, issue) = match uuid(entry) {
                Err(problem) => problem,
                Ok((field, id)) if !seen.insert(id) => (field, Issue::Duplicate),
                Ok((field, id)) if is_order && mods_complete && !mods_keys.contains(&id) => {
                    (field, Issue::NoMatch)
                }
                Ok(_) => continue,
            };
            findings.push(Finding {
                line: lines.at(node.range().start),
                summary: issue.summary(is_order),
            });
            if findings.len() == max_findings {
                return Ok(findings);
            }
        }
    }
    Ok(findings)
}

fn tag(node: Node<'_, '_>, tag: &str) -> bool {
    node.is_element() && node.tag_name().namespace().is_none() && node.tag_name().name() == tag
}

fn named(node: Node<'_, '_>, kind: &str, id: &str) -> bool {
    tag(node, kind) && node.attribute("id") == Some(id)
}

fn elements<'a, 'input>(node: Node<'a, 'input>) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children().filter(Node::is_element)
}

fn has_text(node: Node<'_, '_>) -> bool {
    node.children().any(|child| {
        child.is_text()
            && child
                .text()
                .is_some_and(|s| !s.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r' | b'\n')))
    })
}

fn unique_child<'a, 'input>(
    node: Node<'a, 'input>,
    matches: impl Fn(Node<'a, 'input>) -> bool,
) -> Result<Option<Node<'a, 'input>>, &'static str> {
    let mut children = elements(node).filter(|&child| matches(child));
    let child = children.next();
    if children.next().is_some() {
        return Err(UNSUPPORTED);
    }
    Ok(child)
}

fn children_container<'a, 'input>(
    node: Node<'a, 'input>,
) -> Result<Option<Node<'a, 'input>>, &'static str> {
    if has_text(node) || elements(node).any(|n| !tag(n, "children") && !tag(n, "attribute")) {
        return Err(UNSUPPORTED);
    }
    let children = unique_child(node, |n| tag(n, "children"))?;
    if children.is_some_and(has_text) {
        return Err(UNSUPPORTED);
    }
    Ok(children)
}

fn entries<'a, 'input>(
    children: Option<Node<'a, 'input>>,
) -> impl Iterator<Item = Node<'a, 'input>> {
    children.into_iter().flat_map(elements)
}

fn list_entries<'a, 'input>(
    node: Node<'a, 'input>,
    id: &str,
) -> Result<Option<Node<'a, 'input>>, &'static str> {
    // Norbyte's LSX writer omits <children> when a node has no child nodes.
    let children = children_container(node)?;
    if entries(children).any(|n| !named(n, "node", id)) {
        return Err(UNSUPPORTED);
    }
    Ok(children)
}

#[derive(Clone, Copy)]
enum Issue {
    Missing,
    Invalid,
    RepeatedField,
    Duplicate,
    NoMatch,
}

impl Issue {
    fn summary(self, is_order: bool) -> &'static str {
        match (self, is_order) {
            (Self::Missing, false) => "Mods 条目缺少 UUID 属性",
            (Self::Missing, true) => "ModOrder 条目缺少 UUID 属性",
            (Self::Invalid, false) => "Mods 条目的 UUID 值缺失或格式无效",
            (Self::Invalid, true) => "ModOrder 条目的 UUID 值缺失或格式无效",
            (Self::RepeatedField, false) => "Mods 的同一条目包含多个 UUID 属性",
            (Self::RepeatedField, true) => "ModOrder 的同一条目包含多个 UUID 属性",
            (Self::Duplicate, false) => "Mods 中的 UUID 在多个条目间重复",
            (Self::Duplicate, true) => "ModOrder 中的 UUID 在多个条目间重复",
            (Self::NoMatch, _) => "此文件的 ModOrder UUID 在 Mods 中没有匹配条目",
        }
    }
}

fn uuid<'a, 'input>(
    entry: Node<'a, 'input>,
) -> Result<(Node<'a, 'input>, Uuid), (Node<'a, 'input>, Issue)> {
    let mut fields = elements(entry).filter(|&n| named(n, "attribute", "UUID"));
    let field = fields.next().ok_or((entry, Issue::Missing))?;
    if let Some(repeated) = fields.next() {
        return Err((repeated, Issue::RepeatedField));
    }
    let mut value = field
        .attribute("value")
        .ok_or((field, Issue::Invalid))?
        .trim();
    if let Some(enclosed) = value.strip_prefix('(').and_then(|s| s.strip_suffix(')')) {
        value = enclosed;
    }
    let id = Uuid::try_parse(value).map_err(|_| (field, Issue::Invalid))?;
    Ok((field, id))
}

// Findings arrive in offset order, so finding all physical lines is one pass.
struct PhysicalLines<'a> {
    bytes: &'a [u8],
    offset: usize,
    line: usize,
}

impl<'a> PhysicalLines<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            bytes: text.as_bytes(),
            offset: 0,
            line: 1,
        }
    }

    fn at(&mut self, offset: usize) -> usize {
        for i in self.offset..offset {
            if self.bytes[i] == b'\r'
                || (self.bytes[i] == b'\n' && (i == 0 || self.bytes[i - 1] != b'\r'))
            {
                self.line += 1;
            }
        }
        self.offset = offset;
        self.line
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "28ac9ce2-2aba-8cda-b3b5-6e922f71b6b8";
    const B: &str = "cb555efe-2d9e-131f-8195-a89329d218ea";
    const OPEN: &str = concat!(
        r#"<?xml version="1.0" encoding="UTF-8"?>"#,
        r#"<save><version major="4" minor="8" revision="0" build="100"/>"#,
        r#"<region id="ModuleSettings"><node id="root"><children>"#,
    );
    const CLOSE: &str = "</children></node></region></save>";

    fn lsx(lists: &str) -> String {
        format!("{OPEN}{lists}{CLOSE}")
    }

    fn entry(node: &str, ty: &str, id: &str) -> String {
        format!(r#"<node id="{node}"><attribute id="UUID" type="{ty}" value="{id}"/></node>"#)
    }

    fn list(name: &str, content: &str) -> String {
        format!(r#"<node id="{name}"><children>{content}</children></node>"#)
    }

    #[test]
    fn supports_both_export_layouts_without_assuming_a_shared_order() {
        let older_mods = list(
            "Mods",
            &format!(
                "{}{}",
                entry("ModuleShortDesc", "FixedString", A),
                entry("ModuleShortDesc", "FixedString", B)
            ),
        );
        let older_order = list("ModOrder", &entry("Module", "FixedString", B));
        let current = list("Mods", &entry("ModuleShortDesc", "guid", B));

        for text in [lsx(&current), lsx(&format!("{older_order}{older_mods}"))] {
            assert_eq!(inspect(&text, 10), Ok(vec![]));
        }
    }

    #[test]
    fn accepts_empty_lists_with_or_without_children_wrappers() {
        for lists in [
            r#"<node id="Mods"/>"#,
            r#"<node id="Mods"><children/></node><node id="ModOrder"><children/></node>"#,
            r#"<node id="Mods"/><node id="ModOrder"/>"#,
        ] {
            assert_eq!(inspect(&lsx(lists), 10), Ok(vec![]));
        }
    }

    #[test]
    fn rejects_malformed_truncated_and_unsupported_input() {
        for text in ["", "<save></wrong>", OPEN, "<save/><save/>"] {
            assert_eq!(inspect(text, 10), Err(MALFORMED), "{text:?}");
        }
        for lists in [
            r#"<node id="ModOrder"/>"#,
            r#"<node id="Mods"/><node id="Mods"/>"#,
            r#"<node id="Mods"/><node id="ModOrder"/><node id="ModOrder"/>"#,
            r#"<node id="Mods"><children/><children/></node>"#,
            r#"<node id="Mods"><node id="ModuleShortDesc"/></node>"#,
            r#"<node id="Mods"><children><node id="Other"/></children></node>"#,
            r#"<node id="Mods"><children>not a list</children></node>"#,
        ] {
            assert_eq!(inspect(&lsx(lists), 10), Err(UNSUPPORTED), "{lists}");
        }
        assert_eq!(inspect("<other/>", 0), Err(UNSUPPORTED));
        assert_eq!(
            inspect(
                r#"<save xmlns="urn:other"><region id="ModuleSettings"/></save>"#,
                10
            ),
            Err(UNSUPPORTED)
        );
    }

    #[test]
    fn refuses_empty_internal_and_external_dtds() {
        for dtd in [
            "<!DOCTYPE save>",
            r#"<!DOCTYPE save [<!ENTITY something "value">]>"#,
            r#"<!DOCTYPE save SYSTEM "file:///never-read">"#,
        ] {
            assert_eq!(inspect(&format!("{dtd}<save/>"), 10), Err(DTD));
        }
        assert_eq!(
            inspect(&lsx(r#"<!-- <!DOCTYPE save> --><node id="Mods"/>"#), 10),
            Ok(vec![])
        );
    }

    #[test]
    fn enforces_independent_byte_node_and_finding_limits() {
        assert_eq!(inspect(&" ".repeat(MAX_BYTES + 1), 10), Err(LIMIT));
        let many_nodes = format!("<save>{}</save>", "<!--x-->".repeat(MAX_NODES as usize));
        assert!(many_nodes.len() < MAX_BYTES);
        assert_eq!(inspect(&many_nodes, 10), Err(LIMIT));

        let mods = list("Mods", &r#"<node id="ModuleShortDesc"/>"#.repeat(4));
        let text = lsx(&mods);
        assert_eq!(inspect(&text, 0), Ok(vec![]));
        assert_eq!(inspect(&text, 2).unwrap().len(), 2);
        assert_eq!(inspect(&text, 5).unwrap().len(), 4);
    }

    #[test]
    fn localizes_uuid_conflicts_in_document_order_for_all_newlines() {
        let upper = A.to_ascii_uppercase();
        let source = [
            OPEN,
            r#"<node id="ModOrder"><children>"#,
            &entry("Module", "FixedString", A),
            &entry("Module", "FixedString", &upper),
            &entry("Module", "FixedString", B),
            "</children></node>",
            r#"<node id="Mods"><children>"#,
            &entry("ModuleShortDesc", "guid", A),
            r#"<node id="ModuleShortDesc">"#,
            "<!-- \u{2603}: the UUID tag starts before its value -->",
            r#"<attribute id="UUID" type="guid""#,
            &format!(r#"value="{upper}"/>"#),
            "</node></children></node>",
            CLOSE,
        ];
        let expected = vec![
            Finding {
                line: 4,
                summary: Issue::Duplicate.summary(true),
            },
            Finding {
                line: 5,
                summary: Issue::NoMatch.summary(true),
            },
            Finding {
                line: 11,
                summary: Issue::Duplicate.summary(false),
            },
        ];
        for newline in ["\n", "\r\n", "\r"] {
            assert_eq!(inspect(&source.join(newline), 10), Ok(expected.clone()));
        }
        let mixed = source
            .iter()
            .enumerate()
            .fold(String::new(), |mut s, (i, line)| {
                s.push_str(line);
                s.push_str(["\n", "\r\n", "\r"][i % 3]);
                s
            });
        assert_eq!(inspect(&mixed, 2).unwrap(), expected[..2]);
    }

    #[test]
    fn observes_bad_fields_but_does_not_infer_unmatched_order_from_them() {
        let text = lsx(&format!(
            "{}{}",
            list("ModOrder", &entry("Module", "guid", B)),
            list(
                "Mods",
                &format!(
                    r#"
<node id="ModuleShortDesc"/>
{}
<node id="ModuleShortDesc"><attribute id="UUID" type="guid"/></node>
<node id="ModuleShortDesc">
<attribute id="UUID" type="guid" value="{A}"/>
<attribute id="UUID" type="guid" value="{B}"/>
</node>"#,
                    entry("ModuleShortDesc", "FixedString", "invalid")
                )
            )
        ));
        let findings = inspect(&text, 10).unwrap();
        assert_eq!(
            findings
                .iter()
                .map(|f| (f.line, f.summary))
                .collect::<Vec<_>>(),
            [
                (2, Issue::Missing.summary(false)),
                (3, Issue::Invalid.summary(false)),
                (4, Issue::Invalid.summary(false)),
                (7, Issue::RepeatedField.summary(false)),
            ]
        );
    }

    #[test]
    fn reports_unmatched_order_against_an_explicit_empty_mods_list() {
        let text = lsx(&format!(
            "<node id=\"Mods\"/>{}",
            list("ModOrder", &entry("Module", "FixedString", A)),
        ));
        assert_eq!(
            inspect(&text, 10).unwrap()[0].summary,
            Issue::NoMatch.summary(true)
        );
    }
}
