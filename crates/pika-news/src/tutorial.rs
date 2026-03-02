use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialDoc {
    pub executive_summary: String,
    pub media_links: Vec<String>,
    pub steps: Vec<TutorialStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TutorialStep {
    pub title: String,
    pub intent: String,
    pub affected_files: Vec<String>,
    pub evidence_snippets: Vec<String>,
    pub body_markdown: String,
}

pub fn heuristic_from_diff(diff: &str) -> TutorialDoc {
    let files = extract_files(diff);
    let (additions, deletions) = count_line_changes(diff);

    let executive_summary = if files.is_empty() {
        "No file-level diff was detected. Verify the selected base reference.".to_string()
    } else {
        format!(
            "This change touches {} file(s) with {} additions and {} deletions. Focus on intent first, then verify each file-level step against the evidence snippets.",
            files.len(), additions, deletions
        )
    };

    let steps = files
        .into_iter()
        .take(12)
        .map(|file| {
            let evidence = extract_hunks_for_file(diff, &file);
            TutorialStep {
                title: format!("Review {}", file),
                intent: format!(
                    "Understand why `{}` changed and how it contributes to the overall behavior.",
                    file
                ),
                affected_files: vec![file.clone()],
                evidence_snippets: if evidence.is_empty() {
                    vec![
                        "No hunk markers were found for this file in the unified diff.".to_string(),
                    ]
                } else {
                    evidence
                },
                body_markdown: "1. Read the hunk context.
2. Confirm expected behavior and edge cases.
3. Validate tests or add them if coverage is missing."
                    .to_string(),
            }
        })
        .collect();

    TutorialDoc {
        executive_summary,
        media_links: extract_media_links(diff),
        steps,
    }
}

fn extract_files(diff: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            if let Some((file, _)) = rest.split_once(" b/") {
                if files.iter().all(|existing| existing != file) {
                    files.push(file.to_string());
                }
            }
        }
    }
    files
}

fn count_line_changes(diff: &str) -> (usize, usize) {
    let mut additions = 0usize;
    let mut deletions = 0usize;

    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            additions += 1;
        } else if line.starts_with('-') {
            deletions += 1;
        }
    }

    (additions, deletions)
}

fn extract_hunks_for_file(diff: &str, file: &str) -> Vec<String> {
    let mut in_file = false;
    let mut hunks = Vec::new();

    for line in diff.lines() {
        if let Some(rest) = line.strip_prefix("diff --git a/") {
            in_file = rest.starts_with(&format!("{} b/", file));
            continue;
        }
        if in_file && line.starts_with("@@") {
            hunks.push(line.to_string());
            if hunks.len() >= 3 {
                break;
            }
        }
    }

    hunks
}

fn extract_media_links(diff: &str) -> Vec<String> {
    let mut links = Vec::new();
    for word in diff.split_whitespace() {
        if word.starts_with("https://")
            && (word.contains(".png")
                || word.contains(".jpg")
                || word.contains(".jpeg")
                || word.contains(".gif")
                || word.contains(".mp4"))
            && links.iter().all(|existing| existing != word)
        {
            links.push(word.to_string());
        }
    }
    links
}
