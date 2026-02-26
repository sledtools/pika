use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::collections::{BTreeSet, HashMap};

pub const HYPERNOTE_KIND: u16 = 9467;
pub const HYPERNOTE_ACTION_RESPONSE_KIND: u16 = 9468;
pub const HYPERNOTE_ACTION_REPLY_TAG: &str = "e";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentPropSpec {
    pub name: String,
    pub kind: String,
    pub required: bool,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ComponentSpec {
    pub name: String,
    pub category: String,
    pub description: String,
    #[serde(default)]
    pub design_principles: Vec<String>,
    pub props: Vec<ComponentPropSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActionSpec {
    pub name: String,
    pub trigger_components: Vec<String>,
    pub response_kind: u16,
    pub required_tags: Vec<String>,
    pub payload_schema: String,
    pub visibility: String,
    pub dedupe_rule: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HypernoteCatalog {
    pub protocol_version: u16,
    pub kinds: HypernoteKinds,
    #[serde(default)]
    pub design_principles: Vec<String>,
    pub components: Vec<ComponentSpec>,
    pub actions: Vec<ActionSpec>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HypernoteKinds {
    pub note: u16,
    pub action_response: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HypernoteActionResponse {
    pub action: String,
    pub form: Map<String, Value>,
}

pub fn component_registry() -> Vec<ComponentSpec> {
    vec![
        ComponentSpec {
            name: "Card".to_string(),
            category: "layout".to_string(),
            description: "Visual container with rounded background.".to_string(),
            design_principles: vec![
                "No scrolling. Keep total content brief or content can be cut off.".to_string(),
            ],
            props: vec![],
        },
        ComponentSpec {
            name: "VStack".to_string(),
            category: "layout".to_string(),
            description: "Vertical stack layout container.".to_string(),
            design_principles: vec!["Default layout when unsure.".to_string()],
            props: vec![ComponentPropSpec {
                name: "gap".to_string(),
                kind: "number".to_string(),
                required: false,
                description: "Spacing between child elements (defaults to 8).".to_string(),
            }],
        },
        ComponentSpec {
            name: "HStack".to_string(),
            category: "layout".to_string(),
            description: "Horizontal stack layout container.".to_string(),
            design_principles: vec![
                "Only use for short, fixed-width items like buttons.".to_string(),
                "Never put ChecklistItem or variable-length text in HStack; use VStack instead."
                    .to_string(),
            ],
            props: vec![ComponentPropSpec {
                name: "gap".to_string(),
                kind: "number".to_string(),
                required: false,
                description: "Spacing between child elements (defaults to 8).".to_string(),
            }],
        },
        ComponentSpec {
            name: "Heading".to_string(),
            category: "typography".to_string(),
            description: "Headline text.".to_string(),
            design_principles: vec![],
            props: vec![],
        },
        ComponentSpec {
            name: "Body".to_string(),
            category: "typography".to_string(),
            description: "Body text.".to_string(),
            design_principles: vec![],
            props: vec![],
        },
        ComponentSpec {
            name: "Caption".to_string(),
            category: "typography".to_string(),
            description: "Secondary caption text.".to_string(),
            design_principles: vec![],
            props: vec![],
        },
        ComponentSpec {
            name: "TextInput".to_string(),
            category: "interactive".to_string(),
            description: "Single-line text input bound to form state.".to_string(),
            design_principles: vec![
                "Full-width by default; do not nest inside HStack.".to_string(),
            ],
            props: vec![
                ComponentPropSpec {
                    name: "name".to_string(),
                    kind: "string".to_string(),
                    required: true,
                    description: "Key used in action response form payload.".to_string(),
                },
                ComponentPropSpec {
                    name: "placeholder".to_string(),
                    kind: "string".to_string(),
                    required: false,
                    description: "Placeholder hint text.".to_string(),
                },
            ],
        },
        ComponentSpec {
            name: "ChecklistItem".to_string(),
            category: "interactive".to_string(),
            description: "Boolean checklist item bound to form state.".to_string(),
            design_principles: vec![
                "Labels wrap at narrow widths; keep labels around 3 words when possible."
                    .to_string(),
                "For longer labels, stack ChecklistItems vertically in VStack.".to_string(),
            ],
            props: vec![
                ComponentPropSpec {
                    name: "name".to_string(),
                    kind: "string".to_string(),
                    required: true,
                    description: "Key used in action response form payload.".to_string(),
                },
                ComponentPropSpec {
                    name: "checked".to_string(),
                    kind: "boolean".to_string(),
                    required: false,
                    description: "Initial checked state when no default state is provided."
                        .to_string(),
                },
            ],
        },
        ComponentSpec {
            name: "SubmitButton".to_string(),
            category: "interactive".to_string(),
            description: "Submits current form state as a hypernote action response.".to_string(),
            design_principles: vec![],
            props: vec![
                ComponentPropSpec {
                    name: "action".to_string(),
                    kind: "string".to_string(),
                    required: true,
                    description: "Action name returned in response payload.".to_string(),
                },
                ComponentPropSpec {
                    name: "variant".to_string(),
                    kind: "enum(primary|secondary|danger)".to_string(),
                    required: false,
                    description: "Visual emphasis variant.".to_string(),
                },
            ],
        },
    ]
}

pub fn action_registry() -> Vec<ActionSpec> {
    vec![ActionSpec {
        name: "submit".to_string(),
        trigger_components: vec!["SubmitButton".to_string()],
        response_kind: HYPERNOTE_ACTION_RESPONSE_KIND,
        required_tags: vec![HYPERNOTE_ACTION_REPLY_TAG.to_string()],
        payload_schema: r#"{\"action\":\"string\",\"form\":\"object<string,string>\"}"#.to_string(),
        visibility: "hidden_from_chat_timeline".to_string(),
        dedupe_rule: "latest response per (sender, target_hypernote)".to_string(),
    }]
}

pub fn hypernote_catalog() -> HypernoteCatalog {
    HypernoteCatalog {
        protocol_version: 1,
        kinds: HypernoteKinds {
            note: HYPERNOTE_KIND,
            action_response: HYPERNOTE_ACTION_RESPONSE_KIND,
        },
        design_principles: vec![
            "When in doubt, use VStack.".to_string(),
            "HStack is only for short, pill-shaped items side by side.".to_string(),
        ],
        components: component_registry(),
        actions: action_registry(),
    }
}

pub fn hypernote_catalog_value() -> Value {
    serde_json::to_value(hypernote_catalog()).unwrap_or_else(|_| json!({}))
}

pub fn hypernote_catalog_json() -> String {
    serde_json::to_string_pretty(&hypernote_catalog()).unwrap_or_else(|_| "{}".to_string())
}

pub fn parse_action_response(content: &str) -> Option<HypernoteActionResponse> {
    let value: Value = serde_json::from_str(content).ok()?;
    let obj = value.as_object()?;
    let action = obj.get("action")?.as_str()?.trim();
    if action.is_empty() {
        return None;
    }
    let form = match obj.get("form") {
        Some(v) => v.as_object()?.clone(),
        None => Map::new(),
    };
    Some(HypernoteActionResponse {
        action: action.to_string(),
        form,
    })
}

pub fn build_action_response_payload(action_name: &str, form: &HashMap<String, String>) -> Value {
    json!({
        "action": action_name,
        "form": form,
    })
}

pub fn extract_submit_actions_from_ast_json(ast_json: &str) -> Vec<String> {
    let root: Value = match serde_json::from_str(ast_json) {
        Ok(v) => v,
        Err(_) => return vec![],
    };
    let mut seen = BTreeSet::new();
    let mut out = Vec::new();
    collect_submit_actions(&root, &mut seen, &mut out);
    out
}

fn collect_submit_actions(node: &Value, seen: &mut BTreeSet<String>, out: &mut Vec<String>) {
    if let Some(node_obj) = node.as_object() {
        let node_type = node_obj.get("type").and_then(|v| v.as_str()).unwrap_or("");
        let node_name = node_obj.get("name").and_then(|v| v.as_str()).unwrap_or("");

        if (node_type == "mdx_jsx_element" || node_type == "mdx_jsx_self_closing")
            && node_name == "SubmitButton"
            && let Some(attrs) = node_obj.get("attributes").and_then(|v| v.as_array())
        {
            for attr in attrs {
                let Some(attr_obj) = attr.as_object() else {
                    continue;
                };
                let name = attr_obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
                if name != "action" {
                    continue;
                }
                let value = attr_obj
                    .get("value")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .trim();
                if value.is_empty() {
                    continue;
                }
                if seen.insert(value.to_string()) {
                    out.push(value.to_string());
                }
            }
        }

        if let Some(children) = node_obj.get("children").and_then(|v| v.as_array()) {
            for child in children {
                collect_submit_actions(child, seen, out);
            }
        }
    }
}

pub fn build_poll_hypernote(question: &str, options: &[String]) -> Option<String> {
    let question = question.trim();
    if question.is_empty() {
        return None;
    }
    let valid_options: Vec<String> = options
        .iter()
        .map(|o| o.trim())
        .filter(|o| !o.is_empty())
        .map(ToString::to_string)
        .collect();
    if valid_options.len() < 2 {
        return None;
    }

    let mut out = String::new();
    out.push_str("# ");
    out.push_str(&escape_mdx_text(question));
    out.push_str("\n\n");
    for (index, option) in valid_options.iter().enumerate() {
        let action = format!("option_{index}");
        out.push_str("<SubmitButton action=\"");
        out.push_str(&escape_mdx_attr(&action));
        out.push_str("\">");
        out.push_str(&escape_mdx_text(option));
        out.push_str("</SubmitButton>\n");
    }
    Some(out)
}

fn escape_mdx_text(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_mdx_attr(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_action_response_rejects_missing_action() {
        assert!(parse_action_response(r#"{"form":{}}"#).is_none());
    }

    #[test]
    fn parse_action_response_accepts_shape() {
        let parsed = parse_action_response(r#"{"action":"vote","form":{"x":"1"}}"#)
            .expect("valid action response");
        assert_eq!(parsed.action, "vote");
        assert_eq!(parsed.form.get("x").and_then(|v| v.as_str()), Some("1"));
    }

    #[test]
    fn extract_submit_actions_reads_mdx_ast() {
        let ast = json!({
            "type": "root",
            "children": [
                {
                    "type": "mdx_jsx_element",
                    "name": "SubmitButton",
                    "attributes": [
                        {"name":"action","type":"literal","value":"yes"}
                    ]
                },
                {
                    "type": "mdx_jsx_element",
                    "name": "SubmitButton",
                    "attributes": [
                        {"name":"action","type":"literal","value":"no"}
                    ]
                },
                {
                    "type": "mdx_jsx_element",
                    "name": "SubmitButton",
                    "attributes": [
                        {"name":"action","type":"literal","value":"yes"}
                    ]
                }
            ]
        });
        let out = extract_submit_actions_from_ast_json(&ast.to_string());
        assert_eq!(out, vec!["yes".to_string(), "no".to_string()]);
    }

    #[test]
    fn build_poll_hypernote_rejects_invalid_input() {
        assert!(build_poll_hypernote("", &["yes".into(), "no".into()]).is_none());
        assert!(build_poll_hypernote("Question", &["yes".into()]).is_none());
    }

    #[test]
    fn build_poll_hypernote_escapes_values() {
        let body = build_poll_hypernote(
            "Do <you> & me?",
            &["yes".to_string(), "no & never".to_string()],
        )
        .expect("valid");
        assert!(body.contains("# Do &lt;you&gt; &amp; me?"));
        assert!(body.contains("<SubmitButton action=\"option_0\">yes</SubmitButton>"));
        assert!(body.contains("no &amp; never"));
    }

    #[test]
    fn catalog_contains_design_principles() {
        let catalog = hypernote_catalog();
        assert!(
            catalog
                .design_principles
                .iter()
                .any(|p| p.contains("When in doubt, use VStack"))
        );

        let hstack = catalog
            .components
            .iter()
            .find(|c| c.name == "HStack")
            .expect("HStack exists");
        assert!(
            hstack
                .design_principles
                .iter()
                .any(|p| p.contains("Only use for short, fixed-width items"))
        );

        let checklist = catalog
            .components
            .iter()
            .find(|c| c.name == "ChecklistItem")
            .expect("ChecklistItem exists");
        assert!(
            checklist
                .design_principles
                .iter()
                .any(|p| p.contains("Labels wrap at narrow widths"))
        );
    }
}
