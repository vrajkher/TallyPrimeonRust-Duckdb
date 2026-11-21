//! The JSON Schema subset the MCP tool catalogue uses, checked before a tool runs,
//! as the Node connector checks its Zod schemas.
//!
//! Supported: `type`, `properties`, `required`, `additionalProperties: false`, `enum`,
//! `pattern`, `minLength`, `maxLength`, `minimum`, `maximum`, `exclusiveMinimum`,
//! `items`, `minItems`, `maxItems` and `anyOf`. `format` and `description` are ignored
//! (every `format: uuid` in the catalogue also has a `pattern`).

use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

use regex::Regex;
use serde_json::Value;

/// Up to this many problems are reported for one call.
const MAX_ISSUES: usize = 8;

fn compiled(pattern: &str) -> Option<Regex> {
    static CACHE: OnceLock<Mutex<HashMap<String, Option<Regex>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    let mut cache = cache.lock().ok()?;
    cache
        .entry(pattern.to_owned())
        .or_insert_with(|| Regex::new(pattern).ok())
        .clone()
}

/// The problems with `value`, as `field. message` strings; empty when it is valid.
pub fn validate(schema: &Value, value: &Value) -> Vec<String> {
    let mut issues = Vec::new();
    check(schema, value, "", &mut issues);
    issues.truncate(MAX_ISSUES);
    issues
}

fn push(issues: &mut Vec<String>, path: &str, message: impl Into<String>) {
    let message = message.into();
    issues.push(if path.is_empty() {
        message
    } else {
        format!("{path}. {message}")
    });
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() {
        key.to_owned()
    } else {
        format!("{path}.{key}")
    }
}

fn check(schema: &Value, value: &Value, path: &str, issues: &mut Vec<String>) {
    if issues.len() >= MAX_ISSUES {
        return;
    }
    if let Some(options) = schema.get("anyOf").and_then(Value::as_array) {
        let mut best: Option<Vec<String>> = None;
        for option in options {
            let mut found = Vec::new();
            check(option, value, path, &mut found);
            if found.is_empty() {
                return;
            }
            if best.as_ref().is_none_or(|b| found.len() < b.len()) {
                best = Some(found);
            }
        }
        issues.extend(best.unwrap_or_default());
        return;
    }
    if let Some(kind) = schema.get("type").and_then(Value::as_str) {
        let matches = match kind {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "boolean" => value.is_boolean(),
            "integer" => value.as_f64().is_some_and(|n| n.fract() == 0.0),
            "number" => value.is_number(),
            "null" => value.is_null(),
            _ => true,
        };
        if !matches {
            push(issues, path, format!("Expected {kind}."));
            return;
        }
    }
    if let Some(allowed) = schema.get("enum").and_then(Value::as_array)
        && !allowed.contains(value)
    {
        let names: Vec<String> = allowed.iter().map(Value::to_string).collect();
        push(issues, path, format!("Use one of {}.", names.join(", ")));
    }
    match value {
        Value::Object(object) => {
            let properties = schema.get("properties").and_then(Value::as_object);
            if let Some(required) = schema.get("required").and_then(Value::as_array) {
                for name in required.iter().filter_map(Value::as_str) {
                    if !object.contains_key(name) {
                        push(issues, &join(path, name), "Required.");
                    }
                }
            }
            for (key, item) in object {
                match properties.and_then(|p| p.get(key)) {
                    Some(property) => check(property, item, &join(path, key), issues),
                    None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                        push(issues, &join(path, key), "Unrecognized field.");
                    }
                    None => {}
                }
            }
        }
        Value::Array(items) => {
            let count = items.len() as u64;
            if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
                && count < min
            {
                push(issues, path, format!("Give at least {min} items."));
            }
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
                && count > max
            {
                push(issues, path, format!("Give at most {max} items."));
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    check(item_schema, item, &join(path, &index.to_string()), issues);
                }
            }
        }
        Value::String(text) => {
            let length = text.chars().count() as u64;
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
                && length < min
            {
                push(issues, path, format!("Use at least {min} characters."));
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
                && length > max
            {
                push(issues, path, format!("Use at most {max} characters."));
            }
            if let Some(pattern) = schema.get("pattern").and_then(Value::as_str)
                && !compiled(pattern).is_some_and(|re| re.is_match(text))
            {
                push(issues, path, "Invalid format.");
            }
        }
        Value::Number(number) => {
            let n = number.as_f64().unwrap_or(f64::NAN);
            if let Some(min) = schema.get("minimum").and_then(Value::as_f64)
                && n < min
            {
                push(issues, path, format!("Use {min} or more."));
            }
            if let Some(min) = schema.get("exclusiveMinimum").and_then(Value::as_f64)
                && n <= min
            {
                push(issues, path, format!("Use more than {min}."));
            }
            if let Some(max) = schema.get("maximum").and_then(Value::as_f64)
                && n > max
            {
                push(issues, path, format!("Use {max} or less."));
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn checks_the_catalogue_subset() {
        let schema = json!({
            "type": "object",
            "properties": {
                "from": {"type": "string", "pattern": "^\\d{4}-\\d{2}-\\d{2}$"},
                "limit": {"type": "integer", "minimum": 1, "maximum": 100},
                "id": {"type": "integer", "exclusiveMinimum": 0},
                "kind": {"type": "string", "enum": ["payment", "receipt"]},
                "name": {"type": "string", "minLength": 1, "maxLength": 3},
                "ids": {"type": "array", "items": {"type": "integer"}, "minItems": 1, "maxItems": 2}
            },
            "required": ["from"],
            "additionalProperties": false
        });
        assert!(
            validate(
                &schema,
                &json!({"from": "2025-09-01", "limit": 5, "ids": [1]})
            )
            .is_empty()
        );
        assert_eq!(validate(&schema, &json!({})), vec!["from. Required."]);
        let issues = validate(
            &schema,
            &json!({"from": "Sept", "limit": 0, "id": 0, "kind": "x", "name": "long", "ids": [], "extra": 1}),
        );
        for expected in [
            "from. Invalid format.",
            "limit. Use 1 or more.",
            "id. Use more than 0.",
            "kind. Use one of \"payment\", \"receipt\".",
            "name. Use at most 3 characters.",
            "ids. Give at least 1 items.",
            "extra. Unrecognized field.",
        ] {
            assert!(
                issues.iter().any(|i| i == expected),
                "{expected} in {issues:?}"
            );
        }
        assert_eq!(
            validate(&schema, &json!({"from": "2025-09-01", "limit": 1.5})),
            vec!["limit. Expected integer."]
        );
    }

    #[test]
    fn any_of_accepts_one_match_and_reports_the_closest() {
        let schema = json!({"anyOf": [
            {"type": "object", "properties": {"amount": {"type": "string"}}, "required": ["amount"], "additionalProperties": false},
            {"type": "object", "properties": {"postings": {"type": "array"}}, "required": ["postings"], "additionalProperties": false}
        ]});
        assert!(validate(&schema, &json!({"amount": "1"})).is_empty());
        assert!(validate(&schema, &json!({"postings": []})).is_empty());
        assert_eq!(
            validate(&schema, &json!({"amount": 1})),
            vec!["amount. Expected string."]
        );
    }
}
