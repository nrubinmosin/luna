//! A tool call's arguments held to the tool's own schema before it runs.

use super::tools::{handoff_tool, tools};
use serde_json::Value;

/// A call's arguments held to its tool's schema (`tools`) before anything runs:
/// an object, of keys the tool has, each of the type it says (an `integer` a
/// whole number from 0, an `enum` one of its values), the `required` ones
/// there and not empty. A key it does not know is refused rather than passed
/// over, naming those it does: `read_only` for `readOnly` would otherwise
/// spawn a helper that writes, and `interrupt: "true"` would be no interrupt.
/// A null is a key left out. Not for an unknown tool, which `run` refuses by
/// name, nor `luna_permission`, which the CLI calls in its own shape.
pub(super) fn arguments_fit(name: &str, a: &Value) -> Result<(), String> {
    let mut list = tools();
    if let Some(all) = list.as_array_mut() {
        all.push(handoff_tool());
    }
    let Some(schema) = list.as_array().and_then(|t| t.iter().find(|t| t["name"] == name)).map(|t| t["inputSchema"].clone()) else {
        return Ok(());
    };
    let empty = serde_json::Map::new();
    let props = schema["properties"].as_object().unwrap_or(&empty);
    let given = match a {
        Value::Null => &empty,
        Value::Object(o) => o,
        _ => return Err(format!("{name} takes its arguments as an object")),
    };
    let known: Vec<&str> = props.keys().map(String::as_str).collect();
    for (key, v) in given.iter().filter(|(_, v)| !v.is_null()) {
        let Some(spec) = props.get(key) else {
            // `read_only`, `ReadOnly`, `read-only`: the one meant, by its spelling.
            let loose = |k: &str| k.replace(['_', '-'], "").to_lowercase();
            let meant = known.iter().find(|k| loose(k) == loose(key)).map(|k| format!(" (did you mean `{k}`?)")).unwrap_or_default();
            let takes = if known.is_empty() { "no arguments".to_string() } else { known.join(", ") };
            return Err(format!("{name} has no argument `{key}`{meant}; it takes: {takes}"));
        };
        fits(key, spec, v)?;
    }
    let present = |k: &str| given.get(k).is_some_and(|v| !v.is_null() && v.as_str() != Some(""));
    for key in schema["required"].as_array().into_iter().flatten().filter_map(Value::as_str) {
        if !present(key) {
            return Err(format!("{key} is required"));
        }
    }
    // `luna_session_wait` and `luna_session_delete` name theirs by one or the other.
    if props.contains_key("id") && props.contains_key("ids") && !present("id") && given.get("ids").is_none_or(Value::is_null) {
        return Err("id or ids is required".into());
    }
    Ok(())
}

/// Whether `v` is what `spec` (a property of a tool's schema) says `key` takes.
fn fits(key: &str, spec: &Value, v: &Value) -> Result<(), String> {
    if let Some(any) = spec["anyOf"].as_array() {
        if any.iter().any(|one| fits(key, one, v).is_ok()) {
            return Ok(());
        }
        return Err(format!("`{key}` takes {}", any.iter().map(kind_of).collect::<Vec<_>>().join(" or ")));
    }
    let ok = match spec["type"].as_str() {
        Some("string") => v.is_string(),
        Some("boolean") => v.is_boolean(),
        Some("integer") => v.is_u64(),
        Some("object") => v.is_object(),
        Some("array") => v.as_array().is_some_and(|items| items.iter().all(|x| fits(key, &spec["items"], x).is_ok())),
        _ => true,
    };
    if !ok {
        return Err(format!("`{key}` takes {}", kind_of(spec)));
    }
    if let (Some(allowed), Some(given)) = (spec["enum"].as_array(), v.as_str()) {
        if !allowed.iter().any(|x| x == given) {
            let names: Vec<&str> = allowed.iter().filter_map(Value::as_str).collect();
            return Err(format!("unknown {key} {given:?}; use {}", names.join(", ")));
        }
    }
    Ok(())
}

/// What a schema's type is, as an error says it.
fn kind_of(spec: &Value) -> String {
    match spec["type"].as_str() {
        Some("string") => "a string".into(),
        Some("boolean") => "true or false".into(),
        Some("integer") => "a whole number, 0 or more".into(),
        Some("object") => "an object".into(),
        Some("array") if spec["items"]["type"] == "string" => "a list of strings".into(),
        Some("array") => "a list".into(),
        _ => "a value".into(),
    }
}
