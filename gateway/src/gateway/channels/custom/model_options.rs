use serde_json::{json, Value};
use crate::gateway::{channels::UpstreamModel, protocol::cir::CanonicalRequest};
use super::UpstreamProtocol;

/// Only map explicitly configured levels; retain protocol defaults otherwise.
pub(super) fn apply_reasoning(body: &mut Value, request: &CanonicalRequest, model: &UpstreamModel, protocol: UpstreamProtocol) {
    if !model.capabilities.reasoning || model.reasoning_effort_map.is_empty() { return; }
    let effort = request.reasoning.as_ref().and_then(|reasoning| {
        if reasoning.enabled == Some(false) { Some("off") } else { reasoning.effort.as_deref() }
    }).or(model.reasoning_default_effort.as_deref());
    let Some(effort) = effort else { return; };
    let key = if matches!(effort, "none" | "disabled") { "off" } else { effort };
    let Some(mapped) = model.reasoning_effort_map.get(key).or_else(|| model.reasoning_effort_map.get(effort)) else { return; };
    match protocol {
        UpstreamProtocol::OpenAi => {
            body.as_object_mut().unwrap().remove("reasoning");
            body.as_object_mut().unwrap().remove("thinking");
            body["reasoning_effort"] = json!(mapped);
        }
        UpstreamProtocol::Responses => {
            if !body.get("reasoning").is_some_and(Value::is_object) { body["reasoning"] = json!({}); }
            body["reasoning"]["effort"] = json!(mapped);
        }
        UpstreamProtocol::Anthropic => {
            if key == "off" || matches!(mapped.as_str(), "none" | "off" | "disabled") {
                body["thinking"] = json!({"type": "disabled"});
                if let Some(config) = body.get_mut("output_config").and_then(Value::as_object_mut) { config.remove("effort"); }
            } else if let Ok(budget) = mapped.parse::<u64>() {
                body["thinking"] = json!({"type": "enabled", "budget_tokens": budget});
                if let Some(config) = body.get_mut("output_config").and_then(Value::as_object_mut) { config.remove("effort"); }
            } else {
                body["thinking"] = json!({"type": "adaptive"});
                if !body.get("output_config").is_some_and(Value::is_object) { body["output_config"] = json!({}); }
                body["output_config"]["effort"] = json!(mapped);
            }
        }
    }
}
