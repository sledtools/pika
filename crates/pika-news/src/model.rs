use std::env;

use reqwest::blocking::Client;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};

use crate::tutorial::{TutorialDoc, TutorialStep};

const ANTHROPIC_API_URL: &str = "https://api.anthropic.com/v1/messages";

#[derive(Debug, Clone, Serialize)]
pub struct PromptInput {
    pub pr: PromptPrMetadata,
    pub files: Vec<String>,
    pub unified_diff: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct PromptPrMetadata {
    pub repo: String,
    pub number: Option<u64>,
    pub title: String,
    pub head_sha: Option<String>,
    pub base_ref: String,
}

#[derive(Debug)]
pub enum GenerationError {
    MissingApiKey { env_var: String },
    RetrySafe(String),
    Fatal(String),
}

impl std::fmt::Display for GenerationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingApiKey { env_var } => {
                write!(f, "missing API key in environment variable {}", env_var)
            }
            Self::RetrySafe(msg) => write!(f, "retry-safe generation error: {}", msg),
            Self::Fatal(msg) => write!(f, "fatal generation error: {}", msg),
        }
    }
}

impl std::error::Error for GenerationError {}

pub fn generate_with_anthropic(
    model: &str,
    api_key_env: &str,
    input: &PromptInput,
) -> Result<TutorialDoc, GenerationError> {
    let api_key = env::var(api_key_env).map_err(|_| GenerationError::MissingApiKey {
        env_var: api_key_env.to_string(),
    })?;

    let body = AnthropicRequest {
        model,
        max_tokens: 2400,
        temperature: 0.0,
        top_p: 1.0,
        system: SYSTEM_PROMPT,
        messages: vec![AnthropicMessage {
            role: "user",
            content: vec![AnthropicContent {
                r#type: "text",
                text: build_user_prompt(input),
            }],
        }],
    };

    let client = Client::builder()
        .build()
        .map_err(|err| GenerationError::RetrySafe(format!("build HTTP client: {}", err)))?;

    let response = client
        .post(ANTHROPIC_API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .header("content-type", "application/json")
        .json(&body)
        .send()
        .map_err(|err| GenerationError::RetrySafe(format!("send Anthropic request: {}", err)))?;

    let status = response.status();
    let response_body = response.text().map_err(|err| {
        GenerationError::RetrySafe(format!("read Anthropic response body: {}", err))
    })?;

    if !status.is_success() {
        return Err(classify_http_error(status, response_body));
    }

    let parsed: AnthropicResponse = serde_json::from_str(&response_body).map_err(|err| {
        GenerationError::RetrySafe(format!("parse Anthropic envelope JSON: {}", err))
    })?;

    let text = parsed
        .content
        .iter()
        .find(|part| part.r#type == "text")
        .map(|part| part.text.clone())
        .ok_or_else(|| {
            GenerationError::RetrySafe("Anthropic response had no text content block".to_string())
        })?;

    parse_and_validate_tutorial(&text)
}

fn classify_http_error(status: StatusCode, body: String) -> GenerationError {
    let trimmed = truncate(&body, 600);
    if status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error() {
        GenerationError::RetrySafe(format!("Anthropic HTTP {}: {}", status, trimmed))
    } else {
        GenerationError::Fatal(format!("Anthropic HTTP {}: {}", status, trimmed))
    }
}

fn parse_and_validate_tutorial(raw_output: &str) -> Result<TutorialDoc, GenerationError> {
    let json_payload = extract_json_payload(raw_output);
    let doc: TutorialDoc = serde_json::from_str(&json_payload).map_err(|err| {
        GenerationError::RetrySafe(format!("malformed model output (invalid JSON): {}", err))
    })?;

    validate_doc(&doc)?;
    Ok(doc)
}

fn validate_doc(doc: &TutorialDoc) -> Result<(), GenerationError> {
    if doc.executive_summary.trim().is_empty() {
        return Err(GenerationError::RetrySafe(
            "malformed model output: `executive_summary` was empty".to_string(),
        ));
    }
    if doc.steps.is_empty() {
        return Err(GenerationError::RetrySafe(
            "malformed model output: `steps` was empty".to_string(),
        ));
    }

    for (idx, step) in doc.steps.iter().enumerate() {
        validate_step(idx, step)?;
    }

    Ok(())
}

fn validate_step(index: usize, step: &TutorialStep) -> Result<(), GenerationError> {
    if step.title.trim().is_empty() {
        return Err(GenerationError::RetrySafe(format!(
            "malformed model output: `steps[{index}].title` was empty"
        )));
    }
    if step.intent.trim().is_empty() {
        return Err(GenerationError::RetrySafe(format!(
            "malformed model output: `steps[{index}].intent` was empty"
        )));
    }
    if step.affected_files.is_empty() {
        return Err(GenerationError::RetrySafe(format!(
            "malformed model output: `steps[{index}].affected_files` was empty"
        )));
    }
    if step.evidence_snippets.is_empty() {
        return Err(GenerationError::RetrySafe(format!(
            "malformed model output: `steps[{index}].evidence_snippets` was empty"
        )));
    }

    Ok(())
}

fn extract_json_payload(raw_output: &str) -> String {
    let trimmed = raw_output.trim();
    if let Some(rest) = trimmed.strip_prefix("```") {
        let without_lang = rest
            .strip_prefix("json")
            .or_else(|| rest.strip_prefix("JSON"))
            .unwrap_or(rest);
        let without_newline = without_lang.trim_start_matches('\n');
        if let Some(inner) = without_newline.strip_suffix("```") {
            return inner.trim().to_string();
        }
    }
    trimmed.to_string()
}

fn build_user_prompt(input: &PromptInput) -> String {
    let encoded = serde_json::to_string_pretty(input)
        .unwrap_or_else(|_| "{\"error\":\"failed to encode prompt input\"}".to_string());

    format!(
        "Create a PR tutorial as strict JSON with this exact schema:\n{{\n  \"executive_summary\": \"string\",\n  \"media_links\": [\"https://...\"],\n  \"steps\": [\n    {{\n      \"title\": \"string\",\n      \"intent\": \"string\",\n      \"affected_files\": [\"path\"],\n      \"evidence_snippets\": [\"@@ ... @@\"],\n      \"body_markdown\": \"markdown string\"\n    }}\n  ]\n}}\n\nRules:\n- Output JSON only (no prose, no markdown fences).\n- Include exactly one executive summary paragraph.\n- Each step must include clear intent, affected files, and evidence snippets linked to diff hunks.\n- Keep evidence snippets compact and factual.\n\nInput payload:\n{}",
        encoded
    )
}

fn truncate(input: &str, max: usize) -> String {
    if input.len() <= max {
        input.to_string()
    } else {
        format!("{}...", &input[..max])
    }
}

pub fn bounded_diff(diff: &str, max_chars: usize) -> String {
    if diff.len() <= max_chars {
        diff.to_string()
    } else {
        format!(
            "{}\n\n[diff truncated to {} characters]",
            &diff[..max_chars],
            max_chars
        )
    }
}

#[derive(Debug, Serialize)]
struct AnthropicRequest<'a> {
    model: &'a str,
    max_tokens: u32,
    temperature: f32,
    top_p: f32,
    system: &'a str,
    messages: Vec<AnthropicMessage>,
}

#[derive(Debug, Serialize)]
struct AnthropicMessage {
    role: &'static str,
    content: Vec<AnthropicContent>,
}

#[derive(Debug, Serialize)]
struct AnthropicContent {
    r#type: &'static str,
    text: String,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponse {
    content: Vec<AnthropicResponseContent>,
}

#[derive(Debug, Deserialize)]
struct AnthropicResponseContent {
    r#type: String,
    text: String,
}

const SYSTEM_PROMPT: &str =
    "You produce high-quality engineering tutorials from pull request diffs.";

#[cfg(test)]
mod tests {
    use super::{extract_json_payload, parse_and_validate_tutorial, PromptInput, PromptPrMetadata};

    #[test]
    fn extracts_fenced_json_payload() {
        let payload = extract_json_payload(
            "```json\n{\"executive_summary\":\"ok\",\"media_links\":[],\"steps\":[]}\n```",
        );
        assert!(payload.starts_with('{'));
    }

    #[test]
    fn invalid_schema_returns_retry_safe_error() {
        let err = parse_and_validate_tutorial(
            "{\"executive_summary\":\"\",\"media_links\":[],\"steps\":[]}",
        )
        .expect_err("expected invalid output to fail");
        assert!(format!("{}", err).contains("retry-safe"));
    }

    #[test]
    fn prompt_input_serializes() {
        let input = PromptInput {
            pr: PromptPrMetadata {
                repo: "sledtools/pika".to_string(),
                number: Some(1),
                title: "Demo".to_string(),
                head_sha: Some("abc123".to_string()),
                base_ref: "origin/main".to_string(),
            },
            files: vec!["src/main.rs".to_string()],
            unified_diff: "@@ -1 +1 @@".to_string(),
        };

        let encoded = serde_json::to_string(&input).expect("serialize prompt input");
        assert!(encoded.contains("sledtools/pika"));
    }
}
