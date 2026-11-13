//! The OpenAI Responses API translation (ferb/runtimes/openai_api.py; RUN-418, RUN-430, RUN-438, RUN-447):
//! FERB's instructions in a developer message, explicit cache breakpoints on GPT-5.6 and later, stateless (every
//! output item replayed), `text.format` strict for typed answers, non-strict tools.

use ferb_core::types::{ModelLimits, ModelRequest};
use serde_json::{json, Map, Value};

use super::{
    documented_limits, FeedError, HttpRequest, Provider, StreamParser, ToolCall, ToolOutcome,
    ToolSpec, Turn,
};
use crate::error::RuntimeError;
use crate::forms::Forms;

pub const URL: &str = "https://api.openai.com/v1/responses";
pub const EXPLICIT_CACHE: [&str; 2] = ["gpt-6", "gpt-5.6"];
pub const LIMITS_SOURCE: &str =
    "https://developers.openai.com/api/docs/models (checked 2026-10-14)";
pub const LIMITS: [(&str, i64, i64); 6] = [
    ("gpt-5.5", 1_050_000, 128_000),
    ("gpt-5.6-sol", 1_050_000, 128_000),
    ("gpt-5.6-luna", 1_050_000, 128_000),
    ("gpt-6-astra", 1_050_000, 128_000),
    ("gpt-6.1-sol", 1_050_000, 128_000),
    ("gpt-6-luna", 1_050_000, 128_000),
];
/// The image error codes of OpenAI's Responses API (RUN-418).
pub const IMAGE_CODES: [&str; 14] = [
    "invalid_image",
    "invalid_image_format",
    "invalid_base64_image",
    "invalid_image_url",
    "image_too_large",
    "image_too_small",
    "image_parse_error",
    "image_content_policy_violation",
    "invalid_image_mode",
    "image_file_too_large",
    "unsupported_image_media_type",
    "empty_image_file",
    "failed_to_download_image",
    "image_file_not_found",
];

pub fn limits_table() -> &'static [(&'static str, i64, i64)] {
    &LIMITS
}

pub fn limits_for(model: &str) -> ModelLimits {
    documented_limits(model, &LIMITS, LIMITS_SOURCE, "OpenAI", true)
}

pub struct OpenAIProvider {
    pub model: String,
    pub explicit: bool,
}

impl OpenAIProvider {
    pub fn new(model: Option<&str>) -> Self {
        let model = model
            .filter(|m| !m.is_empty())
            .unwrap_or(crate::catalog::default_model("openai"))
            .to_string();
        let explicit = EXPLICIT_CACHE.iter().any(|p| model.starts_with(p));
        OpenAIProvider { model, explicit }
    }

    fn text(&self, text: &str, mark: bool) -> Value {
        let mut block = json!({"type": "input_text", "text": text});
        if mark && self.explicit {
            block["prompt_cache_breakpoint"] = json!({"mode": "explicit"});
        }
        block
    }
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    }
}

impl Provider for OpenAIProvider {
    fn name(&self) -> &str {
        "openai"
    }

    fn model(&self) -> &str {
        &self.model
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
        cache_key: &str,
    ) -> Value {
        let mut items = Vec::new();
        if !request.system.is_empty() {
            items.push(json!({"role": "developer", "content": [self.text(&request.system, true)]}));
        }
        let mut content: Vec<Value> = request
            .parts
            .iter()
            .enumerate()
            .map(|(i, p)| self.text(p, request.cacheable > 0 && i == request.cacheable - 1))
            .collect();
        for i in &request.images {
            content.push(json!({"type": "input_image", "image_url": i.data_url()}));
        }
        items.push(json!({"role": "user", "content": content}));
        let mut body = Map::new();
        body.insert("model".into(), json!(self.model));
        body.insert("input".into(), Value::Array(items));
        body.insert("store".into(), json!(false));
        body.insert("prompt_cache_key".into(), json!(cache_key));
        body.insert("include".into(), json!(["reasoning.encrypted_content"]));
        if !tools.is_empty() {
            body.insert(
                "tools".into(),
                Value::Array(
                    tools
                        .iter()
                        .map(|t| {
                            json!({"type": "function", "name": t.name, "description": t.description,
                            "parameters": t.schema, "strict": false})
                        })
                        .collect(),
                ),
            );
        }
        if let Some(sch) = schema.filter(|s| ferb_py::value::truthy(s)) {
            body.insert(
                "text".into(),
                json!({"format": {"type": "json_schema", "name": "ferb_result", "schema": sch, "strict": true}}),
            );
        }
        if let Some(e) = effort.filter(|e| !e.is_empty()) {
            body.insert("reasoning".into(), json!({"effort": e}));
        }
        json!({"body": body})
    }

    fn http(&self, state: &Value) -> HttpRequest {
        (
            URL.into(),
            vec![("content-type".into(), "application/json".into())],
            state["body"].clone(),
        )
    }

    fn stream_http(&self, state: &Value) -> Option<HttpRequest> {
        let (url, headers, mut body) = self.http(state);
        body["stream"] = json!(true);
        Some((url, headers, body))
    }

    fn stream_parser(&self, answer: &str) -> Option<Box<dyn StreamParser>> {
        Some(Box::new(OpenAIStream {
            response: None,
            answer: answer.into(),
            answer_item: None,
            answer_began: false,
        }))
    }

    fn auth(&self, key: &str) -> Vec<(String, String)> {
        vec![("authorization".into(), format!("Bearer {key}"))]
    }

    fn parse(&self, state: &mut Value, data: &Value, today: &str) -> Result<Turn, RuntimeError> {
        let status = data.get("status").cloned().unwrap_or(Value::Null);
        match status.as_str() {
            Some("incomplete") => {
                let reason = match data.get("incomplete_details") {
                    Some(v) if ferb_py::value::truthy(v) => v
                        .get("reason")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_else(|| "unknown".into()),
                    _ => "unknown".into(),
                };
                return Err(RuntimeError::provider(format!(
                    "openai: the reply is incomplete ({reason}); it was cut off"
                )));
            }
            Some("failed") => {
                let failure = match data.get("error") {
                    Some(Value::Object(m)) => m.clone(),
                    _ => Map::new(),
                };
                let detail = ferb_py::value::prefix(&s(failure.get("message")), 300).to_string();
                let message = if detail.is_empty() {
                    "openai: the reply failed".to_string()
                } else {
                    format!("openai: the reply failed: {detail}")
                };
                return Err(RuntimeError::Provider {
                    message,
                    overflow: false,
                    account: String::new(),
                    image: self.image_refused(0, &failure),
                    status: 0,
                });
            }
            Some("completed") => {}
            _ => {
                return Err(RuntimeError::provider(format!(
                    "openai: the reply did not complete ({})",
                    ferb_py::value::py_str(&status)
                )))
            }
        }
        let output: Vec<Value> = match data.get("output") {
            Some(Value::Array(o)) => o.clone(),
            _ => Vec::new(),
        };
        if let Some(input) = state["body"]["input"].as_array_mut() {
            input.extend(output.iter().cloned());
        }
        let mut calls = Vec::new();
        for item in &output {
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                continue;
            }
            let raw = s(item.get("arguments"));
            let raw = ferb_py::value::strip(&raw);
            let raw = if raw.is_empty() { "{}" } else { raw };
            let (args, error) = match ferb_py::json::loads(raw) {
                Ok(v @ Value::Object(_)) => (v, ""),
                _ => (json!({}), "not a JSON object"),
            };
            calls.push(ToolCall {
                id: ferb_py::value::py_str(item.get("call_id").unwrap_or(&Value::Null)),
                name: ferb_py::value::py_str(item.get("name").unwrap_or(&Value::Null)),
                arguments: args,
                arguments_error: error.into(),
            });
        }
        let parts: Vec<&Value> = output
            .iter()
            .filter(|i| i.get("type").and_then(Value::as_str) == Some("message"))
            .flat_map(|i| {
                i.get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .collect();
        if parts
            .iter()
            .any(|c| c.get("type").and_then(Value::as_str) == Some("refusal"))
        {
            return Err(RuntimeError::provider(
                "openai: the model refused to answer",
            ));
        }
        let text: String = parts
            .iter()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("output_text"))
            .map(|c| ferb_py::value::py_str(c.get("text").unwrap_or(&json!(""))))
            .collect();
        let usage = match data.get("usage") {
            Some(Value::Object(u)) => u.clone(),
            _ => Map::new(),
        };
        Ok(Turn {
            final_text: if calls.is_empty() { Some(text) } else { None },
            calls,
            usage: crate::usage::from_openai(&self.model, &usage, "api", today)?,
            stop: s(Some(&status)),
            blocks: output
                .iter()
                .map(|i| ferb_py::value::py_str(i.get("type").unwrap_or(&Value::Null)))
                .collect(),
        })
    }

    fn add_message(&self, state: &mut Value, message: &str) {
        let block = self.text(message, false);
        if let Some(input) = state["body"]["input"].as_array_mut() {
            input.push(json!({"role": "user", "content": [block]}));
        }
    }

    fn window_exceeded(&self, _status: u16, error: &Map<String, Value>) -> bool {
        error.get("code").and_then(Value::as_str) == Some("context_length_exceeded")
    }

    fn image_refused(&self, _status: u16, error: &Map<String, Value>) -> bool {
        error
            .get("code")
            .and_then(Value::as_str)
            .is_some_and(|c| IMAGE_CODES.contains(&c))
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
        let first_is_dev = state["body"]["input"]
            .as_array()
            .and_then(|i| i.first())
            .and_then(|i| i.get("role"))
            .and_then(Value::as_str)
            == Some("developer");
        let marker = self.text(statement, true);
        let body = &mut state["body"];
        if let Value::Object(b) = body {
            b.shift_remove("text");
            if let Some(t) = tool {
                if let Value::Array(tools) = b.entry("tools").or_insert_with(|| json!([])) {
                    tools.push(
                        json!({"type": "function", "name": t.name, "description": t.description,
                        "parameters": t.schema, "strict": false}),
                    );
                }
            }
        }
        if let Some(items) = body["input"].as_array_mut() {
            if first_is_dev {
                let text = ferb_py::value::py_str(&items[0]["content"][0]["text"]);
                items[0]["content"][0]["text"] = json!(format!("{text}\n\n{statement}"));
            } else {
                items.insert(0, json!({"role": "developer", "content": [marker]}));
            }
        }
        true
    }

    fn add_results(&self, state: &mut Value, _turn: &Turn, results: &[ToolOutcome]) {
        let Some(input) = state["body"]["input"].as_array_mut() else {
            return;
        };
        for r in results {
            let text = if r.is_error {
                format!("Tool error: {}", r.text)
            } else {
                r.text.clone()
            };
            let out = match &r.image {
                Some(img) => json!([{"type": "input_text", "text": text},
                    {"type": "input_image", "image_url": format!("data:{};base64,{}",
                        ferb_py::value::py_str(&img["mime"]), ferb_py::value::py_str(&img["data"]))}]),
                None => json!(text),
            };
            input
                .push(json!({"type": "function_call_output", "call_id": r.call.id, "output": out}));
        }
    }
}

/// openai_api.py `OpenAIStream`.
pub struct OpenAIStream {
    response: Option<Value>,
    answer: String,
    answer_item: Option<String>,
    answer_began: bool,
}

impl StreamParser for OpenAIStream {
    fn feed(&mut self, event: &Value) -> Result<String, FeedError> {
        let kind = event.get("type").and_then(Value::as_str).unwrap_or("");
        if kind == "response.output_item.added"
            && !self.answer.is_empty()
            && self.answer_item.is_none()
        {
            let item = match event.get("item") {
                Some(v) if ferb_py::value::truthy(v) => v.clone(),
                _ => json!({}),
            };
            if item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("name").and_then(Value::as_str) == Some(&self.answer)
            {
                self.answer_item = Some(s(item.get("id")));
                self.answer_began = true;
            }
            return Ok(String::new());
        }
        match kind {
            "response.function_call_arguments.delta" => Ok(match &self.answer_item {
                Some(id) if &s(event.get("item_id")) == id => s(event.get("delta")),
                _ => String::new(),
            }),
            "response.output_text.delta" => Ok(if self.answer_item.is_none() {
                s(event.get("delta"))
            } else {
                String::new()
            }),
            "response.completed" | "response.failed" | "response.incomplete" => {
                self.response = event.get("response").cloned();
                Ok(String::new())
            }
            "error" => Err(FeedError::Provider(
                "openai: the response stream failed".into(),
            )),
            _ => Ok(String::new()),
        }
    }

    fn take_answer_began(&mut self) -> bool {
        std::mem::take(&mut self.answer_began)
    }

    fn finish(&mut self) -> Result<Value, RuntimeError> {
        match &self.response {
            Some(v @ Value::Object(_)) => Ok(v.clone()),
            _ => Err(RuntimeError::provider(
                "openai: the response stream ended before completion",
            )),
        }
    }
}
