use std::io::Write;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use crate::tutorial::{TutorialDoc, TutorialStep};

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
#[allow(dead_code)]
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

pub struct GenerationOutput {
    pub tutorial: TutorialDoc,
    pub session_id: Option<String>,
}

pub fn generate_with_anthropic(
    _model: &str,
    _api_key_env: &str,
    input: &PromptInput,
) -> Result<GenerationOutput, GenerationError> {
    let prompt = format!("{}\n\n{}", SYSTEM_PROMPT, build_user_prompt(input));

    let stdout = run_claude_cli(
        &["--output-format", "json", "--max-turns", "1"],
        Some(&prompt),
    )?;
    let envelope: ClaudeCliResponse = serde_json::from_str(&stdout).map_err(|err| {
        GenerationError::RetrySafe(format!("parse claude CLI JSON envelope: {}", err))
    })?;

    if envelope.is_error {
        return Err(GenerationError::RetrySafe(format!(
            "claude CLI returned error: {}",
            truncate(&envelope.result, 600)
        )));
    }

    let tutorial = parse_and_validate_tutorial(&envelope.result)?;
    Ok(GenerationOutput {
        tutorial,
        session_id: envelope.session_id,
    })
}

pub fn chat_with_session(
    base_session_id: &str,
    message: &str,
) -> Result<ChatResponse, GenerationError> {
    let stdout = run_claude_cli(
        &[
            "-r",
            base_session_id,
            "--output-format",
            "json",
            "--max-turns",
            "1",
        ],
        Some(message),
    )?;
    let envelope: ClaudeCliResponse = serde_json::from_str(&stdout).map_err(|err| {
        GenerationError::RetrySafe(format!("parse claude CLI JSON envelope: {}", err))
    })?;

    if envelope.is_error {
        return Err(GenerationError::RetrySafe(format!(
            "claude CLI returned error: {}",
            truncate(&envelope.result, 600)
        )));
    }

    Ok(ChatResponse {
        text: envelope.result,
        session_id: envelope.session_id.unwrap_or_default(),
    })
}

pub struct ChatResponse {
    pub text: String,
    pub session_id: String,
}

#[derive(Debug, Deserialize)]
struct ClaudeCliResponse {
    result: String,
    is_error: bool,
    session_id: Option<String>,
}

fn run_claude_cli(args: &[&str], stdin_data: Option<&str>) -> Result<String, GenerationError> {
    let mut cmd = Command::new("claude");
    cmd.args(args);
    if stdin_data.is_some() {
        cmd.stdin(Stdio::piped());
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|err| GenerationError::RetrySafe(format!("spawn claude CLI: {}", err)))?;

    if let Some(data) = stdin_data {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(data.as_bytes());
        }
    }

    let output = child
        .wait_with_output()
        .map_err(|err| GenerationError::RetrySafe(format!("wait claude CLI: {}", err)))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GenerationError::RetrySafe(format!(
            "claude CLI exited {}: {}",
            output.status,
            truncate(&stderr, 600)
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(GenerationError::RetrySafe(format!(
            "claude CLI returned empty stdout. stderr: {}",
            truncate(&stderr, 600)
        )));
    }

    Ok(stdout.into_owned())
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
        format!("{}...", safe_prefix(input, max))
    }
}

pub fn bounded_diff(diff: &str, max_chars: usize) -> String {
    if diff.len() <= max_chars {
        diff.to_string()
    } else {
        format!(
            "{}\n\n[diff truncated to {} characters]",
            safe_prefix(diff, max_chars),
            max_chars
        )
    }
}

fn safe_prefix(input: &str, max: usize) -> &str {
    let mut end = max.min(input.len());
    while end > 0 && !input.is_char_boundary(end) {
        end -= 1;
    }
    &input[..end]
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
