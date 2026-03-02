use std::fs;
use std::path::Path;

use anyhow::Context;

use crate::tutorial::TutorialDoc;

pub fn write_tutorial_html(
    output: &Path,
    page_title: &str,
    base_ref: &str,
    diff: &str,
    doc: &TutorialDoc,
) -> anyhow::Result<()> {
    let html = render_tutorial_html(page_title, base_ref, diff, doc);
    fs::write(output, html).with_context(|| format!("write HTML to {}", output.display()))?;
    Ok(())
}

pub fn render_tutorial_html(
    page_title: &str,
    base_ref: &str,
    diff: &str,
    doc: &TutorialDoc,
) -> String {
    let mut html = String::new();
    html.push_str("<!doctype html><html><head><meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str(&format!("<title>{}</title>\n", escape_html(page_title)));
    html.push_str("<style>body{font-family:-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;max-width:980px;margin:2rem auto;padding:0 1rem;line-height:1.5}pre{white-space:pre-wrap;background:#f5f5f5;padding:1rem;border-radius:8px;overflow:auto}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}.step{border:1px solid #ddd;border-radius:10px;padding:1rem;margin:1rem 0}</style></head><body>");
    html.push_str(&format!("<h1>{}</h1>", escape_html(page_title)));
    html.push_str(&format!(
        "<p><strong>Base ref:</strong> <code>{}</code></p>",
        escape_html(base_ref)
    ));
    html.push_str(&format!("<p>{}</p>", escape_html(&doc.executive_summary)));

    if !doc.media_links.is_empty() {
        html.push_str("<h2>Media Links</h2><ul>");
        for link in &doc.media_links {
            let escaped = escape_html(link);
            html.push_str(&format!("<li><a href=\"{0}\">{0}</a></li>", escaped));
        }
        html.push_str("</ul>");
    }

    html.push_str("<h2>Tutorial Steps</h2>");
    for (idx, step) in doc.steps.iter().enumerate() {
        html.push_str("<section class=\"step\">");
        html.push_str(&format!(
            "<h3>{}. {}</h3>",
            idx + 1,
            escape_html(&step.title)
        ));
        html.push_str(&format!(
            "<p><strong>Intent:</strong> {}</p>",
            escape_html(&step.intent)
        ));
        html.push_str("<p><strong>Affected files:</strong> ");
        for (file_idx, file) in step.affected_files.iter().enumerate() {
            if file_idx > 0 {
                html.push_str(", ");
            }
            html.push_str(&format!("<code>{}</code>", escape_html(file)));
        }
        html.push_str("</p><ul>");
        for evidence in &step.evidence_snippets {
            html.push_str(&format!("<li><code>{}</code></li>", escape_html(evidence)));
        }
        html.push_str("</ul><pre><code>");
        html.push_str(&escape_html(&step.body_markdown));
        html.push_str("</code></pre></section>");
    }

    html.push_str("<h2>Unified Diff</h2><pre><code>");
    html.push_str(&escape_html(diff));
    html.push_str("</code></pre></body></html>");

    html
}

pub fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use crate::tutorial::{TutorialDoc, TutorialStep};

    use super::{render_tutorial_html, write_tutorial_html};

    #[test]
    fn renders_local_artifact_html() {
        let doc = TutorialDoc {
            executive_summary: "summary".to_string(),
            media_links: vec![],
            steps: vec![TutorialStep {
                title: "Step".to_string(),
                intent: "Intent".to_string(),
                affected_files: vec!["src/main.rs".to_string()],
                evidence_snippets: vec!["@@ -1 +1 @@".to_string()],
                body_markdown: "body".to_string(),
            }],
        };

        let html = render_tutorial_html(
            "pika-news local tutorial",
            "origin/main",
            "@@ -1 +1 @@",
            &doc,
        );
        assert!(html.contains("pika-news local tutorial"));
        assert!(html.contains("origin/main"));
        assert!(html.contains("Step"));
    }

    #[test]
    fn writes_local_artifact_file() {
        let dir = tempfile::tempdir().expect("create tempdir");
        let output = dir.path().join("artifact.html");
        let doc = TutorialDoc {
            executive_summary: "summary".to_string(),
            media_links: vec![],
            steps: vec![],
        };

        write_tutorial_html(&output, "page", "base", "diff", &doc).expect("write tutorial HTML");
        let written = std::fs::read_to_string(&output).expect("read written artifact");
        assert!(written.contains("summary"));
    }
}
