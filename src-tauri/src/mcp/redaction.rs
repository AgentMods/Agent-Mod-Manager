use crate::error::{AppError, AppResult};
use regex::Regex;
use serde_json::Value;

const REDACTED: &str = "[REDACTED]";

/// Session-local credential material. Never derive Debug or Serialize here.
pub(super) struct ConnectionSecrets {
    pattern: Option<Regex>,
}

impl ConnectionSecrets {
    pub(super) fn new(values: Vec<String>) -> AppResult<Self> {
        let mut patterns = Vec::new();
        for value in values.into_iter().filter(|value| !value.is_empty()) {
            // Text blocks often contain an inner JSON document, not just prose.
            let encoded = serde_json::to_string(&value)
                .map_err(|_| AppError::Other("Cannot initialize MCP credential filter".into()))?;
            patterns.push(encoded[1..encoded.len() - 1].to_owned());
            patterns.push(value);
        }
        patterns.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        patterns.dedup();
        let pattern = if patterns.is_empty() {
            None
        } else {
            Some(
                Regex::new(
                    &patterns
                        .iter()
                        .map(|value| regex::escape(value))
                        .collect::<Vec<_>>()
                        .join("|"),
                )
                .map_err(|_| AppError::Other("Cannot initialize MCP credential filter".into()))?,
            )
        };
        Ok(Self { pattern })
    }

    pub(super) fn contains(&self, text: &str) -> bool {
        self.pattern
            .as_ref()
            .is_some_and(|pattern| pattern.is_match(text))
    }

    pub(super) fn redact_text(&self, text: &str) -> String {
        match &self.pattern {
            Some(pattern) => pattern.replace_all(text, REDACTED).into_owned(),
            None => text.to_owned(),
        }
    }

    pub(super) fn redact_value(&self, mut value: Value) -> AppResult<Value> {
        if self.pattern.is_none() {
            return Ok(value);
        }
        self.visit(&mut value);
        let encoded = serde_json::to_string(&value)
            .map_err(|_| AppError::Other("Cannot serialize filtered MCP result".into()))?;
        // Fail closed if encoding, replacement boundaries or a very short
        // credential still match. Never perform a replacement on JSON syntax.
        if self.contains(&encoded) {
            return Err(AppError::Other(
                "MCP result withheld by credential filter".into(),
            ));
        }
        Ok(value)
    }

    fn visit(&self, value: &mut Value) {
        match value {
            Value::String(text) => *text = self.redact_text(text),
            Value::Array(items) => {
                for item in items {
                    self.visit(item);
                }
            }
            Value::Object(object) => {
                *object = std::mem::take(object)
                    .into_iter()
                    .map(|(key, mut value)| {
                        self.visit(&mut value);
                        (self.redact_text(&key), value)
                    })
                    .collect();
            }
            scalar => {
                let text = scalar.to_string();
                if self.contains(&text) {
                    *scalar = Value::String(self.redact_text(&text));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn redacts_exact_credentials_in_keys_nested_json_and_scalars() {
        let quoted = "mock-\"quoted\\credential";
        let secrets = ConnectionSecrets::new(vec![
            "mock-secret".into(),
            "mock-secret-longer".into(),
            quoted.into(),
            "482913".into(),
        ])
        .unwrap();
        let value = json!({
            "mock-secret-longer": ["Bearer mock-secret", quoted, 482913],
            "nested": json!({"credential": quoted}).to_string(),
            "ordinary": {"answer": 42, "success": true}
        });
        let filtered = secrets.redact_value(value).unwrap();
        assert_eq!(
            filtered[REDACTED],
            json!(["Bearer [REDACTED]", REDACTED, REDACTED])
        );
        assert_eq!(filtered["ordinary"], json!({"answer":42,"success":true}));
        let inner: Value = serde_json::from_str(filtered["nested"].as_str().unwrap()).unwrap();
        assert_eq!(inner["credential"], REDACTED);
        let encoded = serde_json::to_string(&filtered).unwrap();
        for secret in ["mock-secret", quoted, "482913"] {
            assert!(!encoded.contains(secret));
        }
    }

    #[test]
    fn empty_credentials_are_ignored_and_replacement_matches_fail_closed() {
        let secrets = ConnectionSecrets::new(vec![String::new()]).unwrap();
        let value = json!({"ordinary":"unchanged"});
        assert_eq!(secrets.redact_value(value.clone()).unwrap(), value);
        let secrets = ConnectionSecrets::new(vec!["REDACTED".into()]).unwrap();
        assert!(secrets.redact_value(json!("REDACTED")).is_err());
    }
}
