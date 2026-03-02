use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{anyhow, bail, Context};

use crate::cli::LocalArgs;
use crate::config;
use crate::model::{self, GenerationError, PromptInput, PromptPrMetadata};
use crate::tutorial::{self, TutorialDoc};

pub fn run(args: &LocalArgs) -> anyhow::Result<()> {
    let cwd = env::current_dir().context("resolve current working directory")?;
    let base_ref = resolve_base_ref(args.base.as_deref(), &cwd)?;

    let mut diff = git(
        &cwd,
        ["diff", "--unified=3", &format!("{}...HEAD", base_ref)],
    )
    .with_context(|| format!("collect diff for {}...HEAD", base_ref))?;

    if args.include_uncommitted {
        let staged =
            git(&cwd, ["diff", "--cached", "--unified=3"]).context("collect staged diff")?;
        let unstaged = git(&cwd, ["diff", "--unified=3"]).context("collect unstaged diff")?;

        if !staged.trim().is_empty() {
            diff.push_str("\n\n# staged changes\n");
            diff.push_str(&staged);
        }
        if !unstaged.trim().is_empty() {
            diff.push_str("\n\n# unstaged changes\n");
            diff.push_str(&unstaged);
        }
    }

    let files = tutorial::extract_files(&diff);
    let prompt_input = PromptInput {
        pr: PromptPrMetadata {
            repo: detect_repo_slug(&cwd).unwrap_or_else(|| "local/repo".to_string()),
            number: None,
            title: "Local worktree analysis".to_string(),
            head_sha: git(&cwd, ["rev-parse", "HEAD"])
                .ok()
                .map(|sha| sha.trim().to_string())
                .filter(|sha| !sha.is_empty()),
            base_ref: base_ref.clone(),
        },
        files,
        unified_diff: model::bounded_diff(&diff, 60_000),
    };

    let doc = match model::generate_with_anthropic(
        config::DEFAULT_MODEL,
        config::DEFAULT_API_KEY_ENV,
        &prompt_input,
    ) {
        Ok(generated) => generated,
        Err(GenerationError::MissingApiKey { env_var }) => {
            eprintln!(
                "warning: {} is not set; using local heuristic tutorial generation",
                env_var
            );
            tutorial::heuristic_from_diff(&diff)
        }
        Err(GenerationError::RetrySafe(message)) => {
            return Err(anyhow!("retry-safe generation error: {}", message));
        }
        Err(GenerationError::Fatal(message)) => {
            return Err(anyhow!("fatal generation error: {}", message));
        }
    };
    let output = args
        .out
        .clone()
        .unwrap_or_else(|| cwd.join("pika-news-local.html"));

    write_local_html(&output, &base_ref, &diff, &doc)?;

    if !args.no_open {
        open_in_browser(&output)?;
    }

    println!("wrote local tutorial to {}", output.display());
    Ok(())
}

fn resolve_base_ref(explicit_base: Option<&str>, cwd: &Path) -> anyhow::Result<String> {
    if let Some(base) = explicit_base {
        return Ok(base.to_string());
    }

    if git_ref_exists(cwd, "origin/main")? {
        return Ok("origin/main".to_string());
    }
    if git_ref_exists(cwd, "main")? {
        return Ok("main".to_string());
    }
    // Compatibility fallback for repositories that still use master.
    if git_ref_exists(cwd, "origin/master")? {
        return Ok("origin/master".to_string());
    }
    if git_ref_exists(cwd, "master")? {
        return Ok("master".to_string());
    }

    bail!(
        "could not resolve base ref: none of origin/main, main, origin/master, or master exists. Pass --base <ref>."
    )
}

fn git_ref_exists(cwd: &Path, r#ref: &str) -> anyhow::Result<bool> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--verify")
        .arg(r#ref)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("check git ref {}", r#ref))?;
    Ok(output.status.success())
}

fn detect_repo_slug(cwd: &Path) -> Option<String> {
    let remote = git(cwd, ["config", "--get", "remote.origin.url"]).ok()?;
    let remote = remote.trim();
    if remote.is_empty() {
        return None;
    }

    let without_suffix = remote.trim_end_matches(".git");
    if let Some((_, tail)) = without_suffix.rsplit_once(':') {
        if tail.contains('/') {
            return Some(tail.to_string());
        }
    }
    if let Some((_, tail)) = without_suffix.split_once("github.com/") {
        if tail.contains('/') {
            return Some(tail.to_string());
        }
    }
    None
}

fn git<const N: usize>(cwd: &Path, args: [&str; N]) -> anyhow::Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .context("run git command")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git command failed: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn write_local_html(
    output: &PathBuf,
    base_ref: &str,
    diff: &str,
    doc: &TutorialDoc,
) -> anyhow::Result<()> {
    let mut html = String::new();
    html.push_str("<!doctype html><html><head><meta charset=\"utf-8\">\n");
    html.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    html.push_str("<title>pika-news local tutorial</title>\n");
    html.push_str("<style>body{font-family:-apple-system,BlinkMacSystemFont,Segoe UI,sans-serif;max-width:980px;margin:2rem auto;padding:0 1rem;line-height:1.5}pre{white-space:pre-wrap;background:#f5f5f5;padding:1rem;border-radius:8px;overflow:auto}code{font-family:ui-monospace,SFMono-Regular,Menlo,monospace}.step{border:1px solid #ddd;border-radius:10px;padding:1rem;margin:1rem 0}</style></head><body>");
    html.push_str("<h1>pika-news local tutorial</h1>");
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

    fs::write(output, html).with_context(|| format!("write local HTML to {}", output.display()))?;
    Ok(())
}

fn open_in_browser(path: &Path) -> anyhow::Result<()> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else if cfg!(target_os = "linux") {
        "xdg-open"
    } else {
        bail!("unsupported OS for auto-open. Use --no-open and open the file manually.");
    };

    let status = Command::new(opener)
        .arg(path)
        .status()
        .with_context(|| format!("run browser opener `{}`", opener))?;

    if !status.success() {
        bail!("browser opener `{}` failed with status {}", opener, status);
    }
    Ok(())
}

fn escape_html(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::resolve_base_ref;
    use std::path::Path;

    #[test]
    fn explicit_base_is_used() {
        let base = resolve_base_ref(Some("feature/base"), Path::new(".")).expect("resolve base");
        assert_eq!(base, "feature/base");
    }
}
