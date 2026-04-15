use crate::{AgentRpcPayload, decode_prefixed_envelope};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionMode {
    /// Show only assistant text (final or streamed). Hide tool calls, capabilities, and protocol framing.
    Chat,
    /// Show assistant text and tool call summaries (name + status). Hide raw input/output.
    Coding,
    /// Show everything: assistant text, tool calls with full input/output, capabilities, errors.
    Debug,
    /// Pass through raw content without any interpretation.
    Raw,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectedContent {
    /// Displayable text for the user.
    Text(String),
    /// A status line (e.g. tool call progress). Typically rendered dimmer or as a one-liner.
    Status(String),
    /// Content should be suppressed in this projection mode.
    Hidden,
}

/// Project a raw MLS application message content string through the given mode.
pub fn project_message(content: &str, mode: ProjectionMode) -> ProjectedContent {
    if mode == ProjectionMode::Raw {
        return ProjectedContent::Text(content.to_string());
    }

    let Some(envelope) = decode_prefixed_envelope(content) else {
        return ProjectedContent::Text(content.to_string());
    };

    match (&envelope.payload, mode) {
        (AgentRpcPayload::AssistantText { text }, _) => ProjectedContent::Text(text.clone()),

        (AgentRpcPayload::TextDelta { delta }, _) => ProjectedContent::Text(delta.clone()),

        (AgentRpcPayload::Error { message }, _) => {
            ProjectedContent::Text(format!("[error] {message}"))
        }

        (AgentRpcPayload::ToolCall { tool_name, .. }, ProjectionMode::Coding) => {
            ProjectedContent::Status(format!("[tool] {tool_name}"))
        }
        (
            AgentRpcPayload::ToolCall {
                tool_name,
                call_id,
                input,
            },
            ProjectionMode::Debug,
        ) => ProjectedContent::Text(format!(
            "[tool_call] {tool_name} id={call_id} input={input}"
        )),
        (AgentRpcPayload::ToolCall { .. }, ProjectionMode::Chat) => ProjectedContent::Hidden,

        (
            AgentRpcPayload::ToolCallUpdate {
                status, call_id, ..
            },
            ProjectionMode::Coding,
        ) => ProjectedContent::Status(format!("[tool:{call_id}] {status}")),
        (
            AgentRpcPayload::ToolCallUpdate {
                call_id,
                status,
                output,
            },
            ProjectionMode::Debug,
        ) => {
            let out = output.as_ref().map(|v| v.to_string()).unwrap_or_default();
            ProjectedContent::Text(format!(
                "[tool_update] id={call_id} status={status} output={out}"
            ))
        }
        (AgentRpcPayload::ToolCallUpdate { .. }, ProjectionMode::Chat) => ProjectedContent::Hidden,

        (AgentRpcPayload::Capability { capabilities }, ProjectionMode::Debug) => {
            ProjectedContent::Status(format!("[capabilities] {}", capabilities.join(", ")))
        }
        (AgentRpcPayload::Capability { .. }, _) => ProjectedContent::Hidden,

        (AgentRpcPayload::Done, ProjectionMode::Debug) => {
            ProjectedContent::Status("[done]".to_string())
        }
        (AgentRpcPayload::Done, _) => ProjectedContent::Hidden,

        // User-originated payloads echoed back should be hidden
        (AgentRpcPayload::Prompt { .. }, _)
        | (AgentRpcPayload::Steer { .. }, _)
        | (AgentRpcPayload::FollowUp { .. }, _)
        | (AgentRpcPayload::Abort, _) => ProjectedContent::Hidden,

        // Catch-all for Raw mode was handled above
        (_, ProjectionMode::Raw) => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AGENT_RPC_VERSION, AgentRpcEnvelope, encode_prefixed_envelope};

    fn make_envelope(payload: AgentRpcPayload) -> String {
        let envelope = AgentRpcEnvelope {
            v: AGENT_RPC_VERSION,
            protocol: crate::AgentProtocol::Acp,
            session_id: "test".to_string(),
            idempotency_key: None,
            payload,
        };
        encode_prefixed_envelope(&envelope).unwrap()
    }

    #[test]
    fn raw_mode_passes_through() {
        let content = make_envelope(AgentRpcPayload::Done);
        assert!(matches!(
            project_message(&content, ProjectionMode::Raw),
            ProjectedContent::Text(_)
        ));
    }

    #[test]
    fn plain_text_passes_through_all_modes() {
        for mode in [
            ProjectionMode::Chat,
            ProjectionMode::Coding,
            ProjectionMode::Debug,
        ] {
            let result = project_message("hello world", mode);
            assert_eq!(result, ProjectedContent::Text("hello world".to_string()));
        }
    }

    #[test]
    fn assistant_text_visible_in_all_modes() {
        let content = make_envelope(AgentRpcPayload::AssistantText {
            text: "hi".to_string(),
        });
        for mode in [
            ProjectionMode::Chat,
            ProjectionMode::Coding,
            ProjectionMode::Debug,
        ] {
            assert_eq!(
                project_message(&content, mode),
                ProjectedContent::Text("hi".to_string())
            );
        }
    }

    #[test]
    fn tool_call_hidden_in_chat_status_in_coding_full_in_debug() {
        let content = make_envelope(AgentRpcPayload::ToolCall {
            call_id: "c1".to_string(),
            tool_name: "read_file".to_string(),
            input: serde_json::json!({"path": "/tmp"}),
        });

        assert_eq!(
            project_message(&content, ProjectionMode::Chat),
            ProjectedContent::Hidden
        );
        assert_eq!(
            project_message(&content, ProjectionMode::Coding),
            ProjectedContent::Status("[tool] read_file".to_string())
        );
        assert!(matches!(
            project_message(&content, ProjectionMode::Debug),
            ProjectedContent::Text(t) if t.contains("read_file") && t.contains("c1")
        ));
    }

    #[test]
    fn done_hidden_in_chat_and_coding_status_in_debug() {
        let content = make_envelope(AgentRpcPayload::Done);
        assert_eq!(
            project_message(&content, ProjectionMode::Chat),
            ProjectedContent::Hidden
        );
        assert_eq!(
            project_message(&content, ProjectionMode::Coding),
            ProjectedContent::Hidden
        );
        assert_eq!(
            project_message(&content, ProjectionMode::Debug),
            ProjectedContent::Status("[done]".to_string())
        );
    }

    #[test]
    fn error_visible_in_all_modes() {
        let content = make_envelope(AgentRpcPayload::Error {
            message: "timeout".to_string(),
        });
        for mode in [
            ProjectionMode::Chat,
            ProjectionMode::Coding,
            ProjectionMode::Debug,
        ] {
            assert_eq!(
                project_message(&content, mode),
                ProjectedContent::Text("[error] timeout".to_string())
            );
        }
    }

    #[test]
    fn user_payloads_hidden_in_all_modes() {
        let content = make_envelope(AgentRpcPayload::Prompt {
            message: "hello".to_string(),
        });
        for mode in [
            ProjectionMode::Chat,
            ProjectionMode::Coding,
            ProjectionMode::Debug,
        ] {
            assert_eq!(project_message(&content, mode), ProjectedContent::Hidden);
        }
    }
}
