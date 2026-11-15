//! The Anthropic Messages API translation (ferb/runtimes/anthropic_api.py; RUN-408, RUN-418, RUN-430, RUN-436,
//! RUN-437, RUN-447, RUN-448): breakpoints on the system block and the last stable part with the 1-hour TTL, the
//! automatic 5-minute tail breakpoint, `output_config.format` for typed answers, `tool_choice` auto, effort except
//! on Haiku 4.5, max_tokens required and a cut-off reply an error.

use std::collections::BTreeMap;

use ferb_core::types::{ModelLimits, ModelRequest};
use serde_json::{json, Map, Value};

use super::{
    documented_limits, FeedError, HttpRequest, Provider, StreamParser, ToolCall, ToolOutcome,
    ToolSpec, Turn,
};
use crate::error::RuntimeError;
use crate::forms::{Forms, SchemaTooComplex, Shape};

pub const URL: &str = "https://api.anthropic.com/v1/messages";
pub const VERSION: &str = "2023-06-01";
pub const MAX_TOKENS: i64 = 32000;
pub const NO_EFFORT: [&str; 1] = ["claude-haiku-4-5"];
pub const LIMITS_SOURCE: &str =
    "https://platform.claude.com/docs/en/about-claude/models/overview (checked 2026-10-14)";
/// L28: each model's documented context window and max output.
pub const LIMITS: [(&str, i64, i64); 6] = [
    ("claude-fable-5-1", 1_000_000, 128_000),
    ("claude-opus-5-5", 1_000_000, 128_000),
    ("claude-sonnet-5-5", 1_000_000, 128_000),
    ("claude-haiku-5-5", 1_000_000, 128_000),
    ("claude-haiku-4-5", 200_000, 64_000),
    ("claude-haiku-4-5-20251001", 200_000, 64_000),
];
/// The API's over-window rejection prefixes, lowercased.
pub const WINDOW_EXCEEDED: [&str; 2] = [
    "prompt is too long",
    "input length and `max_tokens` exceed context limit",
];
/// The image rejections as Anthropic's own client classifies them, lowercased (RUN-418).
pub const IMAGE_REFUSALS: [&[&str]; 3] = [
    &["image exceeds", "maximum"],
    &["image dimensions exceed", "many-image"],
    &["could not process image"],
];

pub fn cache() -> Value {
    json!({"type": "ephemeral"})
}

pub fn cache_1h() -> Value {
    json!({"type": "ephemeral", "ttl": "1h"})
}

pub fn limits_for(model: &str) -> ModelLimits {
    documented_limits(model, &LIMITS, LIMITS_SOURCE, "Anthropic", true)
}

pub struct AnthropicProvider {
    pub model: String,
}

impl AnthropicProvider {
    pub fn new(model: Option<&str>) -> Self {
        AnthropicProvider {
            model: model
                .filter(|m| !m.is_empty())
                .unwrap_or(crate::catalog::default_model("anthropic"))
                .to_string(),
        }
    }
}

impl Provider for AnthropicProvider {
    fn name(&self) -> &str {
        "anthropic"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn shape(&self, forms: &dyn Forms, schema: &Value) -> Option<Result<Shape, SchemaTooComplex>> {
        Some(forms.anthropic(schema))
    }

    fn limits(&self) -> Option<ModelLimits> {
        Some(limits_for(&self.model))
    }

    fn begin(
        &self,
        request: &ModelRequest,
        tools: &[ToolSpec],
        schema: Option<&Value>,
        effort: Option<&str>,
        _cache_key: &str,
    ) -> Value {
        let mut content: Vec<Value> = request
            .parts
            .iter()
            .map(|p| json!({"type": "text", "text": p}))
            .collect();
        if 0 < request.cacheable && request.cacheable <= content.len() {
            content[request.cacheable - 1]["cache_control"] = cache_1h();
        }
        for i in &request.images {
            content.push(
                json!({"type": "image", "source": {"type": "base64", "media_type": i.mime,
                "data": i.base64()}}),
            );
        }
        let mut body = Map::new();
        body.insert("model".into(), json!(self.model));
        body.insert("max_tokens".into(), json!(MAX_TOKENS));
        body.insert("cache_control".into(), cache());
        body.insert(
            "messages".into(),
            json!([{"role": "user", "content": content}]),
        );
        if !request.system.is_empty() {
            body.insert(
                "system".into(),
                json!([{"type": "text", "text": request.system, "cache_control": cache_1h()}]),
            );
        }
        if !tools.is_empty() {
            body.insert(
                "tools".into(),
                Value::Array(
                    tools
                        .iter()
                        .map(|t| json!({"name": t.name, "description": t.description, "input_schema": t.schema}))
                        .collect(),
                ),
            );
            body.insert("tool_choice".into(), json!({"type": "auto"}));
        }
        let mut output = Map::new();
        if let Some(s) = schema.filter(|s| ferb_py::value::truthy(s)) {
            output.insert("format".into(), json!({"type": "json_schema", "schema": s}));
        }
        if let Some(e) = effort.filter(|e| !e.is_empty()) {
            if !NO_EFFORT.iter().any(|p| self.model.starts_with(p)) {
                output.insert("effort".into(), json!(e));
            }
        }
        if !output.is_empty() {
            body.insert("output_config".into(), Value::Object(output));
        }
        json!({"body": body})
    }

    fn http(&self, state: &Value) -> HttpRequest {
        let mut body = state["body"].clone();
        // a 1h marker on the request's last block covers the tail itself: the API refuses a differing TTL there
        let last_is_hour = body
            .get("messages")
            .and_then(Value::as_array)
            .and_then(|m| m.last())
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
            .and_then(|c| c.last())
            .and_then(|b| b.get("cache_control"))
            == Some(&cache_1h());
        if last_is_hour {
            if let Value::Object(m) = &mut body {
                m.shift_remove("cache_control");
            }
        }
        (
            URL.into(),
            vec![
                ("anthropic-version".into(), VERSION.into()),
                ("content-type".into(), "application/json".into()),
            ],
            body,
        )
    }

    fn stream_http(&self, state: &Value) -> Option<HttpRequest> {
        let (url, headers, mut body) = self.http(state);
        body["stream"] = json!(true);
        Some((url, headers, body))
    }

    fn stream_parser(&self, answer: &str) -> Option<Box<dyn StreamParser>> {
        Some(Box::new(AnthropicStream::new(answer)))
    }

    fn auth(&self, key: &str) -> Vec<(String, String)> {
        vec![("x-api-key".into(), key.into())]
    }

    fn parse(&self, state: &mut Value, data: &Value, today: &str) -> Result<Turn, RuntimeError> {
        let content: Vec<Value> = match data.get("content") {
            Some(Value::Array(c)) => c.clone(),
            _ => Vec::new(),
        };
        let stop = match data.get("stop_reason") {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => String::new(),
        };
        match stop.as_str() {
            "max_tokens" => {
                return Err(RuntimeError::provider(
                    "anthropic: the reply was cut off at the output limit",
                ))
            }
            "refusal" => {
                return Err(RuntimeError::provider(
                    "anthropic: the model refused to answer",
                ))
            }
            "end_turn" | "tool_use" | "stop_sequence" => {}
            other => {
                return Err(RuntimeError::Provider {
                    message: format!("anthropic: the reply stopped early ({other})"),
                    overflow: other == "model_context_window_exceeded",
                    account: String::new(),
                    image: false,
                    status: 0,
                })
            }
        }
        let mut calls = Vec::new();
        for b in &content {
            if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                let input = b.get("input");
                let (args, err) = match input {
                    Some(v @ Value::Object(_)) => (v.clone(), ""),
                    Some(Value::String(_)) => (json!({}), "not valid JSON"),
                    _ => (json!({}), "not a JSON object"),
                };
                calls.push(ToolCall {
                    id: ferb_py::value::py_str(b.get("id").unwrap_or(&Value::Null)),
                    name: ferb_py::value::py_str(b.get("name").unwrap_or(&Value::Null)),
                    arguments: args,
                    arguments_error: err.into(),
                });
            }
        }
        let replay: Vec<Value> = content
            .iter()
            .map(|b| {
                if b.get("type").and_then(Value::as_str) == Some("tool_use") {
                    let mut out = b.clone();
                    if !b.get("input").is_some_and(Value::is_object) {
                        out["input"] = json!({});
                    }
                    out
                } else {
                    b.clone()
                }
            })
            .collect();
        if let Some(messages) = state["body"]["messages"].as_array_mut() {
            messages.push(json!({"role": "assistant", "content": replay}));
        }
        let text: String = content
            .iter()
            .filter(|b| b.get("type").and_then(Value::as_str) == Some("text"))
            .map(|b| ferb_py::value::py_str(b.get("text").unwrap_or(&json!(""))))
            .collect();
        let usage = match data.get("usage") {
            Some(Value::Object(u)) => u.clone(),
            _ => Map::new(),
        };
        let usage = crate::usage::from_anthropic(&self.model, &usage, "api", None, today)?;
        Ok(Turn {
            final_text: if calls.is_empty() { Some(text) } else { None },
            calls,
            usage,
            stop,
            blocks: content
                .iter()
                .map(|b| ferb_py::value::py_str(b.get("type").unwrap_or(&Value::Null)))
                .collect(),
        })
    }

    fn add_message(&self, state: &mut Value, message: &str) {
        if let Some(m) = state["body"]["messages"].as_array_mut() {
            m.push(json!({"role": "user", "content": [{"type": "text", "text": message}]}));
        }
    }

    fn window_exceeded(&self, status: u16, error: &Map<String, Value>) -> bool {
        let message = match error.get("message") {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => String::new(),
        }
        .to_lowercase();
        status == 400
            && error.get("type").and_then(Value::as_str) == Some("invalid_request_error")
            && WINDOW_EXCEEDED.iter().any(|p| message.starts_with(p))
    }

    fn image_refused(&self, status: u16, error: &Map<String, Value>) -> bool {
        let message = match error.get("message") {
            Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
            _ => String::new(),
        }
        .to_lowercase();
        status == 400
            && IMAGE_REFUSALS
                .iter()
                .any(|shape| shape.iter().all(|part| message.contains(part)))
    }

    fn has_without_format(&self) -> bool {
        true
    }

    fn without_format(
        &self,
        _forms: &dyn Forms,
        state: &mut Value,
        statement: &str,
        tool: Option<&ToolSpec>,
    ) -> bool {
        let body = &mut state["body"];
        if let Some(t) = tool {
            let tools = body
                .as_object_mut()
                .map(|b| b.entry("tools").or_insert_with(|| json!([])));
            if let Some(Value::Array(list)) = tools {
                list.push(
                    json!({"name": t.name, "description": t.description, "input_schema": t.schema}),
                );
            }
            body["tool_choice"] = json!({"type": "auto"});
        }
        if let Some(Value::Object(output)) = body.get_mut("output_config") {
            output.shift_remove("format");
        }
        let drop_output = match body.get("output_config") {
            Some(v) => !ferb_py::value::truthy(v),
            None => false,
        };
        if drop_output {
            if let Value::Object(b) = body {
                b.shift_remove("output_config");
            }
        }
        match body.get_mut("system") {
            Some(Value::Array(system)) if !system.is_empty() => {
                let last = system.last_mut().expect("not empty");
                let text = ferb_py::value::py_str(last.get("text").unwrap_or(&json!("")));
                last["text"] = json!(format!("{text}\n\n{statement}"));
            }
            _ => {
                body["system"] =
                    json!([{"type": "text", "text": statement, "cache_control": cache_1h()}]);
            }
        }
        true
    }

    fn add_results(&self, state: &mut Value, _turn: &Turn, results: &[ToolOutcome]) {
        let blocks: Vec<Value> = results
            .iter()
            .map(|r| {
                let mut content = vec![json!({"type": "text", "text": r.text})];
                if let Some(img) = &r.image {
                    content.push(json!({"type": "image", "source": {"type": "base64",
                        "media_type": img.get("mime"), "data": img.get("data")}}));
                }
                json!({"type": "tool_result", "tool_use_id": r.call.id, "content": content,
                    "is_error": r.is_error})
            })
            .collect();
        if let Some(m) = state["body"]["messages"].as_array_mut() {
            m.push(json!({"role": "user", "content": blocks}));
        }
    }
}

/// anthropic_api.py `AnthropicStream`: indexed blocks assembled; thinking and tool arguments never stream as answer
/// text, except the answer tool's arguments (RUN-447), and text blocks after it don't.
pub struct AnthropicStream {
    message: Option<Map<String, Value>>,
    blocks: BTreeMap<i64, Value>,
    arguments: BTreeMap<i64, String>,
    stopped: bool,
    answer: String,
    answer_index: Option<i64>,
    answer_began: bool,
}

impl AnthropicStream {
    pub fn new(answer: &str) -> Self {
        AnthropicStream {
            message: None,
            blocks: BTreeMap::new(),
            arguments: BTreeMap::new(),
            stopped: false,
            answer: answer.into(),
            answer_index: None,
            answer_began: false,
        }
    }
}

fn index_of(event: &Value) -> Result<i64, FeedError> {
    let v = event.get("index").ok_or(FeedError::Malformed)?;
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().filter(|f| f.fract() == 0.0).map(|f| f as i64))
            .ok_or(FeedError::Malformed),
        Value::Bool(b) => Ok(i64::from(*b)),
        _ => Err(FeedError::Malformed),
    }
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    }
}

impl StreamParser for AnthropicStream {
    fn feed(&mut self, event: &Value) -> Result<String, FeedError> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        match kind {
            "message_start" => match event.get("message") {
                Some(Value::Object(m)) => self.message = Some(m.clone()),
                _ => return Err(FeedError::Malformed),
            },
            "content_block_start" => {
                let Some(Value::Object(started)) = event.get("content_block") else {
                    return Err(FeedError::Malformed);
                };
                let index = index_of(event)?;
                let started = Value::Object(started.clone());
                self.blocks.insert(index, started.clone());
                if !self.answer.is_empty()
                    && self.answer_index.is_none()
                    && started.get("type").and_then(Value::as_str) == Some("tool_use")
                    && started.get("name").and_then(Value::as_str) == Some(&self.answer)
                {
                    self.answer_index = Some(index);
                    self.answer_began = true;
                }
                return Ok(
                    if started.get("type").and_then(Value::as_str) == Some("text")
                        && self.answer_index.is_none()
                    {
                        s(started.get("text"))
                    } else {
                        String::new()
                    },
                );
            }
            "content_block_delta" => {
                let index = index_of(event)?;
                let delta = event.get("delta").ok_or(FeedError::Malformed)?.clone();
                if !delta.is_object() {
                    return Err(FeedError::Malformed);
                }
                let Some(block) = self.blocks.get_mut(&index) else {
                    return Err(FeedError::Provider(
                        "anthropic: a response fragment had no content block".into(),
                    ));
                };
                let dtype = delta.get("type").and_then(Value::as_str).unwrap_or("");
                let append = |block: &mut Value, key: &str, add: String| -> Result<(), FeedError> {
                    let before = match block.get(key) {
                        None => String::new(),
                        Some(Value::String(s)) => s.clone(),
                        Some(_) => return Err(FeedError::Malformed),
                    };
                    block[key] = json!(before + &add);
                    Ok(())
                };
                if dtype == "text_delta"
                    && block.get("type").and_then(Value::as_str) == Some("text")
                {
                    let text = s(delta.get("text"));
                    append(block, "text", text.clone())?;
                    return Ok(if self.answer_index.is_none() {
                        text
                    } else {
                        String::new()
                    });
                }
                match dtype {
                    "thinking_delta" => append(block, "thinking", s(delta.get("thinking")))?,
                    "signature_delta" => append(block, "signature", s(delta.get("signature")))?,
                    "input_json_delta" => {
                        let partial = s(delta.get("partial_json"));
                        self.arguments.entry(index).or_default().push_str(&partial);
                        if Some(index) == self.answer_index {
                            return Ok(partial);
                        }
                    }
                    "citations_delta" => {
                        let citation = delta.get("citation").cloned().unwrap_or(Value::Null);
                        let list = block
                            .as_object_mut()
                            .map(|b| b.entry("citations").or_insert_with(|| json!([])));
                        match list {
                            Some(Value::Array(l)) => l.push(citation),
                            _ => return Err(FeedError::Malformed),
                        }
                    }
                    _ => {}
                }
            }
            "content_block_stop" => {
                let index = index_of(event)?;
                if let Some(raw) = self.arguments.get(&index).cloned() {
                    let block = self.blocks.get_mut(&index).ok_or(FeedError::Malformed)?;
                    if ferb_py::value::strip(&raw).is_empty() {
                        // a tool called with no arguments: its block's own input, else {}
                        if !block.get("input").is_some_and(Value::is_object) {
                            block["input"] = json!({});
                        }
                    } else {
                        block["input"] = match ferb_py::json::loads(&raw) {
                            Ok(v) => v,
                            // told back to the model as the call's error (its one repair), not fatal
                            Err(_) => json!(raw),
                        };
                    }
                }
            }
            "message_delta" if self.message.is_some() => {
                let m = self.message.as_mut().expect("checked");
                if let Some(Value::Object(d)) = event.get("delta") {
                    for (k, v) in d {
                        m.insert(k.clone(), v.clone());
                    }
                }
                let usage = m.entry("usage").or_insert_with(|| json!({}));
                if let (Value::Object(u), Some(Value::Object(add))) = (usage, event.get("usage")) {
                    for (k, v) in add {
                        u.insert(k.clone(), v.clone());
                    }
                }
            }
            "message_stop" => self.stopped = true,
            "error" => {
                return Err(FeedError::Provider(
                    "anthropic: the response stream failed".into(),
                ))
            }
            _ => {}
        }
        Ok(String::new())
    }

    fn take_answer_began(&mut self) -> bool {
        std::mem::take(&mut self.answer_began)
    }

    fn finish(&mut self) -> Result<Value, RuntimeError> {
        let Some(message) = self.message.clone().filter(|_| self.stopped) else {
            return Err(RuntimeError::provider(
                "anthropic: the response stream ended before completion",
            ));
        };
        let mut out = message;
        out.insert(
            "content".into(),
            Value::Array(self.blocks.values().cloned().collect()),
        );
        Ok(Value::Object(out))
    }
}
