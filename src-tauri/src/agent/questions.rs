use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{collections::HashSet, io};

struct PreviewBudget(usize);

impl io::Write for PreviewBudget {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| io::Error::other("questionnaire payload exceeds its byte budget"))?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, untagged)]
enum Request {
    Structured(Questionnaire),
    Legacy(Legacy),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Legacy {
    question: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Questionnaire {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    questions: Vec<Question>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Question {
    question: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    options: Vec<String>,
    #[serde(
        rename = "multiSelect",
        default,
        deserialize_with = "deserialize_present_bool",
        skip_serializing_if = "Option::is_none"
    )]
    multi_select: Option<bool>,
}

// An omitted flag allows the historical text hint, but a present flag must be a boolean.
fn deserialize_present_bool<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<bool>, D::Error> {
    bool::deserialize(deserializer).map(Some)
}

fn effective_multi_select(question: &str, has_options: bool, explicit: Option<bool>) -> bool {
    has_options
        && explicit.unwrap_or_else(|| {
            // Match the frontend's JS whitespace and punctuation class, not substrings like 不可多选.
            question
                .split(|c: char| {
                    (c.is_whitespace() && c != '\u{0085}')
                        || c == '\u{FEFF}'
                        || "()（）【】[]，,。.!！?？:：;；、".contains(c)
                })
                .any(|word| matches!(word, "多选" | "可多选" | "可以多选"))
        })
}

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string", "minLength": 1},
            "questions": {
                "type": "array", "minItems": 1,
                "items": {
                    "type": "object", "required": ["question"], "additionalProperties": false,
                    "properties": {
                        "question": {"type": "string", "minLength": 1},
                        "options": {
                            "type": "array", "uniqueItems": true,
                            "items": {"type": "string", "minLength": 1}
                        },
                        "multiSelect": {
                            "type": "boolean", "default": false,
                            "description": "仅在提供 options 且多个选项可同时选择时设为 true；默认单选"
                        }
                    }
                }
            },
            "question": {"type": "string", "minLength": 1, "description": "仅用于兼容旧版单题；新请求请用 questions，且不要与 questions 或 title 同时提供"}
        },
        "required": ["questions"],
        "additionalProperties": false
    })
}

fn clean_text<'a>(value: &'a str, field: &str) -> AppResult<&'a str> {
    let clean = value.trim();
    if clean.is_empty() || clean.contains('\0') {
        return Err(AppError::Invalid(format!("{field}不能为空或包含空字符")));
    }
    Ok(clean)
}

fn normalize_text(value: &mut String, field: &str) -> AppResult<()> {
    let clean = clean_text(value, field)?;
    if clean.len() != value.len() {
        *value = clean.to_owned();
    }
    Ok(())
}

pub(super) fn preview(args: &Value) -> AppResult<Value> {
    let capacity = super::sse::response_payload_capacity();
    serde_json::to_writer(PreviewBudget(capacity), args)
        .map_err(|_| AppError::Other("可用内存不足以显示此问卷，请分批提问".into()))?;
    let request = Request::deserialize(args)
        .map_err(|_| AppError::Invalid("请提供有效的 questions 问题列表".into()))?;
    match request {
        Request::Legacy(request) => Ok(json!({"question": clean_text(&request.question, "问题")?})),
        Request::Structured(mut request) => {
            if request.questions.is_empty() {
                return Err(AppError::Invalid("请至少提交一个问题".into()));
            }
            if let Some(title) = &mut request.title {
                normalize_text(title, "标题")?;
            }
            for item in &mut request.questions {
                normalize_text(&mut item.question, "问题")?;
                let mut unique = HashSet::new();
                for option in &mut item.options {
                    normalize_text(option, "选项")?;
                    if !unique.insert(option.as_str()) {
                        return Err(AppError::Invalid("同一题的选项不能重复".into()));
                    }
                }
                let multi_select = effective_multi_select(
                    &item.question,
                    !item.options.is_empty(),
                    item.multi_select,
                );
                if multi_select || item.multi_select.is_some() {
                    item.multi_select = Some(multi_select);
                }
            }
            Ok(serde_json::to_value(request)?)
        }
    }
}

pub(super) enum AnswerShape {
    Legacy,
    Structured(Vec<bool>),
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum StructuredAnswer {
    Text(String),
    Multiple(Vec<String>),
}

impl AnswerShape {
    pub(super) fn from_preview(preview: &Value) -> Self {
        match preview.get("questions").and_then(Value::as_array) {
            Some(questions) => Self::Structured(
                questions
                    .iter()
                    .map(|item| {
                        let has_options = item["options"].as_array().is_some_and(|options| {
                            options.iter().any(|option| {
                                option
                                    .as_str()
                                    .is_some_and(|text| clean_text(text, "选项").is_ok())
                            })
                        });
                        effective_multi_select(
                            item["question"].as_str().unwrap_or_default(),
                            has_options,
                            item.get("multiSelect")
                                .map(|flag| flag.as_bool().unwrap_or(false)),
                        )
                    })
                    .collect(),
            ),
            None => Self::Legacy,
        }
    }

    // Convert the frontend submission to a typed tool result before resolving the wait.
    pub(super) fn validate(&self, answer: &Value) -> AppResult<Value> {
        let invalid = || {
            AppError::Invalid(
                match self {
                    Self::Legacy => "请提交文本回答",
                    Self::Structured(_) => {
                        "请按题目顺序提交回答：单选或自填用文本，多选用非空文本数组，跳过用 null"
                    }
                }
                .into(),
            )
        };
        let text = answer.as_str().ok_or_else(invalid)?;
        if text.len() > super::sse::response_payload_capacity() {
            return Err(AppError::Other("可用内存不足以处理此回答".into()));
        }
        match self {
            Self::Legacy => Ok(json!(clean_text(text, "回答")?)),
            Self::Structured(multi_select) => {
                let mut answers: Vec<Option<StructuredAnswer>> =
                    serde_json::from_str(text).map_err(|_| invalid())?;
                if answers.len() != multi_select.len() {
                    return Err(invalid());
                }
                for (answer, &is_multi) in answers.iter_mut().zip(multi_select) {
                    let Some(answer) = answer else { continue };
                    match answer {
                        StructuredAnswer::Text(text) => {
                            normalize_text(text, "回答")?;
                            if is_multi {
                                // Older clients could select one choice or submit a custom value.
                                // Preserve that text as one value; separators can be part of it.
                                *answer = StructuredAnswer::Multiple(vec![std::mem::take(text)]);
                            }
                        }
                        StructuredAnswer::Multiple(values) if is_multi && !values.is_empty() => {
                            let mut unique = HashSet::new();
                            for value in values {
                                normalize_text(value, "回答")?;
                                if !unique.insert(value.as_str()) {
                                    return Err(AppError::Invalid(
                                        "同一题的多选回答不能重复".into(),
                                    ));
                                }
                            }
                        }
                        StructuredAnswer::Multiple(_) => return Err(invalid()),
                    }
                }
                Ok(serde_json::to_value(answers)?)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_legacy_and_structured_questions_and_rejects_invalid_options() {
        assert_eq!(
            preview(&json!({"question":" 确认？ "})).unwrap(),
            json!({"question":"确认？"})
        );
        let input = json!({"title": " 适配信息 ", "questions": [{"question": " 是哪个平台？ ", "options": [" Steam ", "其他"]}, {"question": "安装目录？"}]});
        let normalized = preview(&input).unwrap();
        assert_eq!(normalized["title"], "适配信息");
        assert_eq!(
            normalized["questions"][0]["options"],
            json!(["Steam", "其他"])
        );
        for invalid in [
            json!({"questions": []}),
            json!({"question": "兼容", "questions": [{"question": "新题"}]}),
            json!({"questions": [{"question": "重复选项？", "options": ["A", " A "]}]}),
            json!({"questions": [{"question": "   "}]}),
            json!({"questions": [{"question": "选项格式？", "options": [true]}]}),
        ] {
            assert!(preview(&invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn multi_select_requires_options_and_infers_only_standalone_hints() {
        for (question, enabled) in [
            ("安装哪些 Mod？（可多选）如果只能选一个，优先选 A。", true),
            ("选 Mod【多选】", true),
            ("请选择: 可以多选。", true),
            ("选择 Mod (可多选)", true),
            ("选 Mod？　多选", true),
            ("哪些 Mod？不可多选", false),
            ("选择 Mod（不可以多选）", false),
            ("请单选（仅选一项）", false),
            ("请选择一个多选方案", false),
            ("安装哪些 Mod？", false),
        ] {
            let args = json!({"questions":[{"question":question,"options":["A","B"]}]});
            let normalized = preview(&args).unwrap();
            assert_eq!(
                normalized["questions"][0]["multiSelect"].as_bool(),
                enabled.then_some(true),
                "{question}"
            );
            for input in [&args, &normalized] {
                let result = AnswerShape::from_preview(input).validate(&json!(r#"[["A","B"]]"#));
                assert_eq!(result.is_ok(), enabled, "{question}");
            }
        }

        let normalized = preview(&json!({"questions":[
            {"question":"选 Mod（可多选）","options":["A","B"],"multiSelect":false},
            {"question":"补充说明（可多选）"},
            {"question":"选择 Mod","options":[],"multiSelect":true},
            {"question":"安装哪些 Mod？","options":["A","B"],"multiSelect":true}
        ]}))
        .unwrap();
        assert_eq!(normalized["questions"][0]["multiSelect"], false);
        assert!(normalized["questions"][1].get("multiSelect").is_none());
        assert_eq!(normalized["questions"][2]["multiSelect"], false);
        assert_eq!(normalized["questions"][3]["multiSelect"], true);
        let shape = AnswerShape::from_preview(&normalized);
        assert_eq!(
            shape
                .validate(&json!(r#"["A","补充","选 A",["B"]]"#))
                .unwrap(),
            json!(["A", "补充", "选 A", ["B"]])
        );
        for invalid in [
            r#"[["A"],null,null,null]"#,
            r#"[null,["A"],null,null]"#,
            r#"[null,null,["A"],null]"#,
        ] {
            assert!(shape.validate(&json!(invalid)).is_err(), "{invalid}");
        }
    }

    #[test]
    fn multi_select_flag_rejects_non_booleans() {
        for flag in [json!(null), json!("true"), json!(1), json!([]), json!({})] {
            let args = json!({"questions":[{"question":"选 Mod（可多选）","options":["A","B"],"multiSelect":flag}]});
            assert!(preview(&args).is_err(), "{args}");
            // A malformed stored flag also must not be treated as an omitted one.
            assert!(AnswerShape::from_preview(&args)
                .validate(&json!(r#"[["A","B"]]"#))
                .is_err());
        }
    }

    #[test]
    fn normalizes_multi_select_and_legacy_text_without_splitting() {
        let historical = json!({"questions":[
            {"question":"请选择（可多选）","options":["A","B"]},
            {"question":"请选择","options":["A","B"],"multiSelect":true},
            {"question":"目标目录？"}
        ]});
        let normalized = preview(&historical).unwrap();
        for preview in [&historical, &normalized] {
            let shape = AnswerShape::from_preview(preview);
            assert_eq!(
                shape
                    .validate(&json!(r#"[[" B "," A "],null," 路径 "]"#))
                    .unwrap(),
                json!([["B", "A"], null, "路径"])
            );
            assert_eq!(
                shape
                    .validate(&json!(r#"[" A, B "," 自填选项 ",null]"#))
                    .unwrap(),
                json!([["A, B"], ["自填选项"], null])
            );
            for invalid in [
                r#"[[],null,null]"#,
                r#"[["A",null],null,null]"#,
                r#"[["A",true],null,null]"#,
                r#"[[["A"]],null,null]"#,
                r#"[["A"," "],null,null]"#,
                r#"[["A","\u0000"],null,null]"#,
                r#"[["A"," A "],null,null]"#,
                r#"[" ",null,null]"#,
                r#"[null,{"value":"A"},null]"#,
            ] {
                assert!(shape.validate(&json!(invalid)).is_err(), "{invalid}");
            }
        }
    }

    #[test]
    fn schema_and_preview_allow_long_fields_and_many_questions_and_options() {
        let schema = schema();
        let questions = &schema["properties"]["questions"];
        let items = &questions["items"]["properties"];
        assert_eq!(items["multiSelect"]["type"], "boolean");
        assert_eq!(items["multiSelect"]["default"], false);
        assert_eq!(questions["items"]["required"], json!(["question"]));
        assert!(questions.get("maxItems").is_none());
        assert!(items["options"].get("maxItems").is_none());
        assert!(items["options"]["items"].get("maxLength").is_none());
        assert!(items["question"].get("maxLength").is_none());

        let title = "标题".repeat(150);
        let question = "问题".repeat(1500);
        let options: Vec<_> = (0..70)
            .map(|i| format!("{i} {}", "选项".repeat(120)))
            .collect();
        let mut entries = vec![json!({ "question": question, "options": options })];
        entries.extend((1..12).map(|i| json!({ "question": format!("第 {i} 项？") })));
        let input = json!({ "title": title, "questions": entries });
        assert_eq!(preview(&input).unwrap(), input);
    }

    #[test]
    fn accepts_questionnaires_and_answers_past_previous_byte_quotas() {
        let question = "问".repeat(100_000);
        assert!(preview(&json!({"question": &question})).is_ok());
        assert!(preview(&json!({"questions":[{"question":question}]})).is_ok());
        let answer = "答".repeat(400_000);
        assert_eq!(
            AnswerShape::Legacy.validate(&json!(&answer)).unwrap(),
            answer
        );
        let actual = json!([answer, null]);
        assert_eq!(
            AnswerShape::Structured(vec![false; 2])
                .validate(&json!(actual.to_string()))
                .unwrap(),
            actual
        );
    }

    #[test]
    fn validates_answer_count_type_and_skips() {
        let shape = AnswerShape::Structured(vec![false; 3]);
        assert_eq!(
            shape
                .validate(&json!(r#"["自动检测",null," 自己填写 "]"#))
                .unwrap(),
            json!(["自动检测", null, "自己填写"])
        );
        for invalid in [
            json!("[]"),
            json!(r#"[true,null,null]"#),
            json!(r#"[" ",null,null]"#),
            json!([null, null, null]),
        ] {
            assert!(shape.validate(&invalid).is_err());
        }
        assert_eq!(AnswerShape::Legacy.validate(&json!(" 是 ")).unwrap(), "是");
    }
}
