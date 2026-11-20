use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

const MAX_QUESTIONS: usize = 6;
const MAX_OPTIONS: usize = 6;
const MAX_TITLE: usize = 120;
const MAX_QUESTION: usize = 2000;
const MAX_OPTION: usize = 200;
const MAX_ANSWER: usize = 2000;

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
}

pub(super) fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "title": {"type": "string", "minLength": 1, "maxLength": MAX_TITLE},
            "questions": {
                "type": "array", "minItems": 1, "maxItems": MAX_QUESTIONS,
                "items": {
                    "type": "object", "required": ["question"], "additionalProperties": false,
                    "properties": {
                        "question": {"type": "string", "minLength": 1, "maxLength": MAX_QUESTION},
                        "options": {
                            "type": "array", "maxItems": MAX_OPTIONS, "uniqueItems": true,
                            "items": {"type": "string", "minLength": 1, "maxLength": MAX_OPTION}
                        }
                    }
                }
            },
            "question": {"type": "string", "minLength": 1, "maxLength": MAX_QUESTION, "description": "仅用于兼容旧版单题；新请求请用 questions，且不要与 questions 或 title 同时提供"}
        },
        "required": ["questions"],
        "additionalProperties": false
    })
}

fn bounded_text(value: &str, max: usize, field: &str) -> AppResult<String> {
    let clean = value.trim();
    if clean.is_empty() || clean.chars().count() > max || clean.contains('\0') {
        return Err(AppError::Invalid(format!(
            "{field}不能为空，且最多 {max} 字符"
        )));
    }
    Ok(clean.to_owned())
}

pub(super) fn preview(args: &Value) -> AppResult<Value> {
    let request: Request = serde_json::from_value(args.clone())
        .map_err(|_| AppError::Invalid("请提供有效的 questions 问题列表".into()))?;
    match request {
        Request::Legacy(request) => {
            Ok(json!({"question": bounded_text(&request.question, MAX_QUESTION, "问题")?}))
        }
        Request::Structured(mut request) => {
            if request.questions.is_empty() || request.questions.len() > MAX_QUESTIONS {
                return Err(AppError::Invalid(format!(
                    "一次可提交 1 至 {MAX_QUESTIONS} 个问题"
                )));
            }
            request.title = request
                .title
                .as_deref()
                .map(|text| bounded_text(text, MAX_TITLE, "标题"))
                .transpose()?;
            for item in &mut request.questions {
                item.question = bounded_text(&item.question, MAX_QUESTION, "问题")?;
                if item.options.len() > MAX_OPTIONS {
                    return Err(AppError::Invalid(format!("每题最多 {MAX_OPTIONS} 个选项")));
                }
                let mut unique = HashSet::new();
                for option in &mut item.options {
                    *option = bounded_text(option, MAX_OPTION, "选项")?;
                    if !unique.insert(option.as_str()) {
                        return Err(AppError::Invalid("同一题的选项不能重复".into()));
                    }
                }
            }
            Ok(serde_json::to_value(request)?)
        }
    }
}

pub(super) enum AnswerShape {
    Legacy,
    Structured(usize),
}

impl AnswerShape {
    pub(super) fn from_preview(preview: &Value) -> Self {
        match preview.get("questions").and_then(Value::as_array) {
            Some(questions) => Self::Structured(questions.len()),
            None => Self::Legacy,
        }
    }

    // Convert the frontend submission to a typed tool result before resolving the wait.
    pub(super) fn validate(&self, answer: &Value) -> AppResult<Value> {
        let invalid = || AppError::Invalid("请为每个问题提交文本回答或明确跳过".into());
        let text = answer.as_str().ok_or_else(invalid)?;
        match self {
            Self::Legacy => Ok(json!(bounded_text(text, MAX_ANSWER, "回答")?)),
            Self::Structured(count) => {
                // A UTF-16 textarea bound plus normal JSON escaping fits comfortably here.
                if text.len() > MAX_QUESTIONS * (MAX_ANSWER * 6 + 3) + 2 {
                    return Err(invalid());
                }
                let answers: Vec<Option<String>> =
                    serde_json::from_str(text).map_err(|_| invalid())?;
                if answers.len() != *count {
                    return Err(invalid());
                }
                let answers = answers
                    .into_iter()
                    .map(|answer| {
                        answer
                            .map(|answer| bounded_text(&answer, MAX_ANSWER, "回答"))
                            .transpose()
                    })
                    .collect::<AppResult<Vec<_>>>()?;
                Ok(json!(answers))
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
    fn validates_answer_count_type_and_skips() {
        let shape = AnswerShape::Structured(3);
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
