//! The Gemini API (generateContent) translation (ferb/runtimes/google_api.py; RUN-408, RUN-438, RUN-447):
//! parametersJsonSchema declarations, responseJsonSchema for typed answers, thought signatures sent back exactly,
//! implicit caching, and only finishReason STOP (or none) as a normal end.

use ferb_core::types::{ModelLimits, ModelRequest};
use serde_json::{json, Map, Value};

use super::{
    documented_limits, FeedError, HttpRequest, Provider, StreamParser, ToolCall, ToolOutcome,
    ToolSpec, Turn,
};
use crate::error::RuntimeError;
use crate::forms::{Forms, SchemaTooComplex, Shape};

pub const BASE: &str = "https://generativelanguage.googleapis.com/v1beta/models";
/// A function call Gemini sent without an id gets FERB's own; only Gemini's own ids go back with results.
pub const LOCAL_ID: &str = "ferb-local:";
pub const LIMITS_SOURCE: &str = "https://ai.google.dev/gemini-api/docs/models (checked 2026-10-14)";
pub const LIMITS: [(&str, i64, i64); 2] = [
    ("gemini-3.8-flash", 1_048_576, 65_536),
    ("gemini-3.1-pro-preview", 1_048_576, 65_536),
];

pub fn limits_for(model: &str) -> ModelLimits {
    documented_limits(model, &LIMITS, LIMITS_SOURCE, "Google", false)
}

pub struct GoogleProvider {
    pub model: String,
}

impl GoogleProvider {
    pub fn new(model: Option<&str>) -> Self {
        GoogleProvider {
            model: model
                .filter(|m| !m.is_empty())
                .unwrap_or(crate::catalog::default_model("google"))
                .to_string(),
        }
    }
}

fn s(v: Option<&Value>) -> String {
    match v {
        Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
        _ => String::new(),
    }
}

impl Provider for GoogleProvider {
    fn name(&self) -> &str {
        "google"
    }

    fn model(&self) -> &str {
        &self.model
    }

    fn shape(&self, forms: &dyn Forms, schema: &Value) -> Option<Result<Shape, SchemaTooComplex>> {
        Some(forms.gemini(schema))
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
        let mut parts: Vec<Value> = request.parts.iter().map(|p| json!({"text": p})).collect();
        for i in &request.images {
            parts.push(json!({"inlineData": {"mimeType": i.mime, "data": i.base64()}}));
        }
        let mut body = Map::new();
        body.insert("contents".into(), json!([{"role": "user", "parts": parts}]));
        if !request.system.is_empty() {
            body.insert(
                "systemInstruction".into(),
                json!({"parts": [{"text": request.system}]}),
            );
        }
        if !tools.is_empty() {
            body.insert(
                "tools".into(),
                json!([{"functionDeclarations": tools.iter().map(|t| json!({"name": t.name,
                    "description": t.description, "parametersJsonSchema": t.schema})).collect::<Vec<_>>()}]),
            );
        }
        let mut config = Map::new();
        if let Some(sch) = schema.filter(|s| ferb_py::value::truthy(s)) {
            config.insert("responseMimeType".into(), json!("application/json"));
            config.insert("responseJsonSchema".into(), sch.clone());
        }
        if let Some(e) = effort.filter(|e| !e.is_empty()) {
            config.insert("thinkingConfig".into(), json!({"thinkingLevel": e}));
        }
        if !config.is_empty() {
            body.insert("generationConfig".into(), Value::Object(config));
        }
        json!({"body": body})
    }

    fn http(&self, state: &Value) -> HttpRequest {
        (
            format!("{BASE}/{}:generateContent", self.model),
            vec![("content-type".into(), "application/json".into())],
            state["body"].clone(),
        )
    }

    fn stream_http(&self, state: &Value) -> Option<HttpRequest> {
        Some((
            format!("{BASE}/{}:streamGenerateContent?alt=sse", self.model),
            vec![("content-type".into(), "application/json".into())],
            state["body"].clone(),
        ))
    }

    fn stream_parser(&self, answer: &str) -> Option<Box<dyn StreamParser>> {
        Some(Box::new(GoogleStream {
            parts: Vec::new(),
            candidate: Map::new(),
            metadata: Map::new(),
            answer: answer.into(),
            answering: false,
            answer_began: false,
        }))
    }

    fn auth(&self, key: &str) -> Vec<(String, String)> {
        vec![("x-goog-api-key".into(), key.into())]
    }

    fn parse(&self, state: &mut Value, data: &Value, today: &str) -> Result<Turn, RuntimeError> {
        let candidates = match data.get("candidates") {
            Some(Value::Array(c)) if !c.is_empty() => c.clone(),
            _ => {
                let reason = match data.get("promptFeedback") {
                    Some(f) if ferb_py::value::truthy(f) => f
                        .get("blockReason")
                        .map(ferb_py::value::py_str)
                        .unwrap_or_else(|| "no candidates".into()),
                    _ => "no candidates".into(),
                };
                return Err(RuntimeError::provider(format!(
                    "google: no answer ({reason})"
                )));
            }
        };
        let first = &candidates[0];
        let finish = s(first.get("finishReason"));
        if finish == "MAX_TOKENS" {
            return Err(RuntimeError::provider(
                "google: the reply was cut off at the output limit",
            ));
        }
        if !(finish.is_empty() || finish == "STOP") {
            return Err(RuntimeError::provider(format!(
                "google: the model stopped ({finish})"
            )));
        }
        let content = match first.get("content") {
            Some(c) if ferb_py::value::truthy(c) => c.clone(),
            _ => json!({"role": "model", "parts": []}),
        };
        let parts: Vec<Value> = match content.get("parts") {
            Some(Value::Array(p)) => p.clone(),
            _ => Vec::new(),
        };
        if !parts.is_empty() {
            // thought signatures go back exactly as received
            if let Some(c) = state["body"]["contents"].as_array_mut() {
                c.push(content.clone());
            }
        }
        let mut calls: Vec<ToolCall> = Vec::new();
        for p in &parts {
            let Some(fc) = p.get("functionCall").filter(|f| ferb_py::value::truthy(f)) else {
                continue;
            };
            let args = fc.get("args");
            let ok = matches!(args, Some(Value::Object(_)) | Some(Value::Null) | None);
            let name = ferb_py::value::py_str(fc.get("name").unwrap_or(&Value::Null));
            let ident = match fc.get("id") {
                Some(v) if ferb_py::value::truthy(v) => ferb_py::value::py_str(v),
                _ => format!("{LOCAL_ID}{}:{name}", calls.len()),
            };
            let arguments = match args {
                Some(v @ Value::Object(_)) if ok => v.clone(),
                _ => json!({}),
            };
            calls.push(ToolCall {
                id: ident,
                name,
                arguments,
                arguments_error: if ok { "" } else { "not a JSON object" }.into(),
            });
        }
        let text: String = parts
            .iter()
            .filter(|p| {
                p.get("text").is_some() && !p.get("thought").is_some_and(ferb_py::value::truthy)
            })
            .map(|p| ferb_py::value::py_str(p.get("text").unwrap_or(&json!(""))))
            .collect();
        let kinds = parts
            .iter()
            .map(|p| {
                if p.get("thought").is_some_and(ferb_py::value::truthy) {
                    "thought".to_string()
                } else if p.get("functionCall").is_some_and(ferb_py::value::truthy) {
                    "function_call".to_string()
                } else if p.get("text").is_some() {
                    "text".to_string()
                } else {
                    p.as_object()
                        .and_then(|m| m.keys().next().cloned())
                        .unwrap_or_default()
                }
            })
            .collect();
        let meta = match data.get("usageMetadata") {
            Some(Value::Object(m)) => m.clone(),
            _ => Map::new(),
        };
        Ok(Turn {
            final_text: if calls.is_empty() { Some(text) } else { None },
            calls,
            usage: crate::usage::from_gemini(&self.model, &meta, "api", today)?,
            stop: finish,
            blocks: kinds,
        })
    }

    fn add_message(&self, state: &mut Value, message: &str) {
        if let Some(c) = state["body"]["contents"].as_array_mut() {
            c.push(json!({"role": "user", "parts": [{"text": message}]}));
        }
    }

    fn window_exceeded(&self, _status: u16, _error: &Map<String, Value>) -> bool {
        false
    }

    fn image_refused(&self, _status: u16, _error: &Map<String, Value>) -> bool {
        false
    }

    fn has_without_format(&self) -> bool {
        true
    }

    fn without_format(
        &self,
        forms: &dyn Forms,
        state: &mut Value,
        statement: &str,
        tool: Option<&ToolSpec>,
    ) -> bool {
        let Value::Object(body) = &mut state["body"] else {
            return true;
        };
        if let Some(t) = tool {
            if !body.get("tools").is_some_and(ferb_py::value::truthy) {
                body.insert("tools".into(), json!([{"functionDeclarations": []}]));
            }
            let wire = forms
                .gemini(&t.schema)
                .map(|s| s.wire)
                .unwrap_or_else(|_| t.schema.clone());
            if let Some(decls) = body["tools"][0]["functionDeclarations"].as_array_mut() {
                decls.push(json!({"name": t.name, "description": t.description,
                    "parametersJsonSchema": wire}));
            }
        }
        if let Some(Value::Object(config)) = body.get_mut("generationConfig") {
            config.shift_remove("responseMimeType");
            config.shift_remove("responseJsonSchema");
        }
        if body
            .get("generationConfig")
            .is_some_and(|c| !ferb_py::value::truthy(c))
        {
            body.shift_remove("generationConfig");
        }
        let instruction = body
            .entry("systemInstruction")
            .or_insert_with(|| json!({"parts": []}));
        if let Some(parts) = instruction["parts"].as_array_mut() {
            parts.push(json!({"text": statement}));
        }
        true
    }

    fn add_results(&self, state: &mut Value, _turn: &Turn, results: &[ToolOutcome]) {
        let parts: Vec<Value> = results
            .iter()
            .map(|r| {
                let mut response = Map::new();
                response.insert("name".into(), json!(r.call.name));
                response.insert(
                    "response".into(),
                    json!({(if r.is_error { "error" } else { "result" }): r.text}),
                );
                if !r.call.id.starts_with(LOCAL_ID) {
                    response.insert("id".into(), json!(r.call.id));
                }
                if let Some(img) = &r.image {
                    response.insert(
                        "parts".into(),
                        json!([{"inlineData": {"mimeType": img.get("mime"), "data": img.get("data")}}]),
                    );
                }
                json!({"functionResponse": response})
            })
            .collect();
        if let Some(c) = state["body"]["contents"].as_array_mut() {
            c.push(json!({"role": "user", "parts": parts}));
        }
    }
}

/// google_api.py `GoogleStream`: complete candidate chunks; the answer tool's call comes whole.
pub struct GoogleStream {
    parts: Vec<Value>,
    candidate: Map<String, Value>,
    metadata: Map<String, Value>,
    answer: String,
    answering: bool,
    answer_began: bool,
}

impl StreamParser for GoogleStream {
    fn feed(&mut self, event: &Value) -> Result<String, FeedError> {
        if event.get("error").is_some_and(ferb_py::value::truthy) {
            return Err(FeedError::Provider(
                "google: the response stream failed".into(),
            ));
        }
        for field in ["usageMetadata", "promptFeedback"] {
            if let Some(v) = event.get(field) {
                self.metadata.insert(field.into(), v.clone());
            }
        }
        let candidates = match event.get("candidates") {
            Some(Value::Array(c)) if !c.is_empty() => c.clone(),
            Some(v) if ferb_py::value::truthy(v) => return Err(FeedError::Malformed),
            _ => return Ok(String::new()),
        };
        let Value::Object(candidate) = &candidates[0] else {
            return Err(FeedError::Malformed);
        };
        let index = candidate.get("index").cloned().unwrap_or(json!(0));
        if !ferb_py::value::py_eq(&index, &json!(0)) {
            return Ok(String::new());
        }
        for (k, v) in candidate {
            self.candidate.insert(k.clone(), v.clone());
        }
        let parts: Vec<Value> = candidate
            .get("content")
            .filter(|c| ferb_py::value::truthy(c))
            .and_then(|c| c.get("parts"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        self.parts.extend(parts.iter().cloned());
        let mut out: Vec<String> = Vec::new();
        for part in &parts {
            let call = match part.get("functionCall") {
                Some(c) if ferb_py::value::truthy(c) => c.clone(),
                _ => json!({}),
            };
            if !self.answer.is_empty()
                && !self.answering
                && call.get("name").and_then(Value::as_str) == Some(&self.answer)
            {
                self.answering = true;
                self.answer_began = true;
                let args = match call.get("args") {
                    Some(a) if ferb_py::value::truthy(a) => a.clone(),
                    _ => json!({}),
                };
                // text before it isn't the answer
                out = vec![ferb_py::json::dumps_utf8(&args)];
            } else if !self.answering && !part.get("thought").is_some_and(ferb_py::value::truthy) {
                out.push(s(part.get("text")));
            }
        }
        Ok(out.concat())
    }

    fn take_answer_began(&mut self) -> bool {
        std::mem::take(&mut self.answer_began)
    }

    fn finish(&mut self) -> Result<Value, RuntimeError> {
        if self.candidate.is_empty() {
            return Ok(Value::Object(self.metadata.clone()));
        }
        if !self
            .candidate
            .get("finishReason")
            .is_some_and(ferb_py::value::truthy)
        {
            return Err(RuntimeError::provider(
                "google: the response stream ended before completion",
            ));
        }
        let mut content = match self.candidate.get("content") {
            Some(Value::Object(c)) => c.clone(),
            _ => Map::new(),
        };
        content.insert("parts".into(), Value::Array(self.parts.clone()));
        let mut candidate = self.candidate.clone();
        candidate.insert("content".into(), Value::Object(content));
        let mut out = self.metadata.clone();
        out.insert("candidates".into(), json!([candidate]));
        Ok(Value::Object(out))
    }
}
