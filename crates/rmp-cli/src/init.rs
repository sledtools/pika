use std::path::{Path, PathBuf};

use crate::cli::{human_log, json_print, CliError, JsonOk};
use crate::config::find_workspace_root;

pub fn init(
    cwd: &Path,
    json: bool,
    verbose: bool,
    args: crate::cli::InitArgs,
) -> Result<(), CliError> {
    let include_ios = resolve_toggle(args.ios, args.no_ios, true);
    let include_android = resolve_toggle(args.android, args.no_android, true);
    if !include_ios && !include_android {
        return Err(CliError::user(
            "at least one platform must be enabled (use --ios or --android)",
        ));
    }

    let requested = PathBuf::from(&args.name);
    let project_dir_name = requested
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .ok_or_else(|| CliError::user("project name must be a valid path segment"))?;

    let dest = if requested.is_absolute() {
        requested.clone()
    } else {
        cwd.join(&requested)
    };

    if dest.exists() {
        return Err(CliError::user(format!(
            "destination already exists: {}",
            dest.to_string_lossy()
        )));
    }

    let org = args.org.unwrap_or_else(|| "com.example".to_string());
    validate_org(&org)?;

    let id_segment = java_identifier_segment(&project_dir_name);
    let bundle_id = args
        .bundle_id
        .unwrap_or_else(|| format!("{org}.{id_segment}"));
    let app_id = args.app_id.unwrap_or_else(|| format!("{org}.{id_segment}"));

    validate_bundle_like("bundle id", &bundle_id)?;
    validate_bundle_like("app id", &app_id)?;

    let display_name = display_name(&project_dir_name);
    let template_root = template_root(cwd)?;

    human_log(
        verbose,
        format!(
            "initializing project '{}' at {}",
            project_dir_name,
            dest.to_string_lossy()
        ),
    );

    std::fs::create_dir_all(&dest)
        .map_err(|e| CliError::operational(format!("failed to create destination: {e}")))?;

    copy_file(&template_root.join(".gitignore"), &dest.join(".gitignore"))?;
    copy_file(
        &template_root.join(".env.example"),
        &dest.join(".env.example"),
    )?;
    copy_file(&template_root.join(".envrc"), &dest.join(".envrc"))?;
    copy_file(&template_root.join("flake.nix"), &dest.join("flake.nix"))?;
    copy_file(&template_root.join("flake.lock"), &dest.join("flake.lock"))?;

    copy_tree_filtered(
        &template_root.join("rust"),
        &dest.join("rust"),
        &|rel, _is_dir| rel.ends_with("target") || rel.to_string_lossy().contains("/.DS_Store"),
    )?;
    copy_tree_filtered(
        &template_root.join("uniffi-bindgen"),
        &dest.join("uniffi-bindgen"),
        &|rel, _is_dir| rel.to_string_lossy().contains("/.DS_Store"),
    )?;

    if include_android {
        copy_tree_filtered(
            &template_root.join("android"),
            &dest.join("android"),
            &|rel, _is_dir| {
                rel.starts_with("build")
                    || rel.starts_with(".gradle")
                    || rel.starts_with("app/src/main/jniLibs")
                    || rel == Path::new("local.properties")
                    || rel.to_string_lossy().contains("/.DS_Store")
            },
        )?;
    }

    if include_ios {
        copy_tree_filtered(
            &template_root.join("ios"),
            &dest.join("ios"),
            &|rel, _is_dir| {
                rel.starts_with("build")
                    || rel.starts_with(".build")
                    || rel.starts_with("Frameworks")
                    || rel.starts_with("Bindings")
                    || rel.starts_with("Pika.xcodeproj")
                    || rel.to_string_lossy().contains("/.DS_Store")
            },
        )?;
    }

    write_text_file(&dest.join("Cargo.toml"), &workspace_toml())?;
    write_text_file(
        &dest.join("justfile"),
        &generated_justfile(include_ios, include_android),
    )?;
    write_text_file(
        &dest.join("README.md"),
        &generated_readme(&project_dir_name, include_ios, include_android),
    )?;
    write_text_file(
        &dest.join("rmp.toml"),
        &generated_rmp_toml(
            &project_dir_name,
            &org,
            &bundle_id,
            &app_id,
            include_ios,
            include_android,
        ),
    )?;

    patch_text(
        &dest.join("flake.nix"),
        "description = \"Pika - Rust core + Android app dev environment\";",
        &format!(
            "description = \"{} - Rust core + Android app dev environment\";",
            display_name
        ),
    )?;
    patch_text(
        &dest.join("flake.nix"),
        "echo \"Pika dev environment ready\"",
        &format!("echo \"{} dev environment ready\"", display_name),
    )?;

    patch_text(
        &dest.join(".env.example"),
        "PIKA_IOS_BUNDLE_ID=com.justinmoon.pika.dev",
        &format!("PIKA_IOS_BUNDLE_ID={bundle_id}.dev"),
    )?;
    patch_text(
        &dest.join(".env.example"),
        "PIKA_ANDROID_APP_ID=com.justinmoon.pika.dev",
        &format!("PIKA_ANDROID_APP_ID={app_id}.dev"),
    )?;

    if include_android {
        patch_line_prefix(
            &dest.join("android/app/build.gradle.kts"),
            "applicationId = \"",
            &format!("        applicationId = \"{app_id}\""),
        )?;
        patch_text(
            &dest.join("android/app/src/main/res/values/strings.xml"),
            "<string name=\"app_name\">Pika</string>",
            &format!("<string name=\"app_name\">{display_name}</string>"),
        )?;
    }

    if include_ios {
        patch_text(
            &dest.join("ios/project.yml"),
            "PRODUCT_BUNDLE_IDENTIFIER: com.justinmoon.pika",
            &format!("PRODUCT_BUNDLE_IDENTIFIER: {bundle_id}"),
        )?;
        patch_text(
            &dest.join("ios/project.yml"),
            "PRODUCT_BUNDLE_IDENTIFIER: com.justinmoon.pika.dev",
            &format!("PRODUCT_BUNDLE_IDENTIFIER: {bundle_id}.dev"),
        )?;
        patch_text(
            &dest.join("ios/Info.plist"),
            "\t<string>Pika</string>",
            &format!("\t<string>{display_name}</string>"),
        )?;
    }

    if json {
        let mut platforms: Vec<&str> = vec![];
        if include_ios {
            platforms.push("ios");
        }
        if include_android {
            platforms.push("android");
        }
        json_print(&JsonOk {
            ok: true,
            data: serde_json::json!({
                "path": dest,
                "project": {
                    "name": project_dir_name,
                    "org": org,
                    "bundle_id": bundle_id,
                    "app_id": app_id,
                },
                "platforms": platforms,
            }),
        });
    } else {
        eprintln!("ok: initialized project at {}", dest.to_string_lossy());
        if include_ios {
            eprintln!("  ios bundle id: {bundle_id}");
        }
        if include_android {
            eprintln!("  android app id: {app_id}");
        }
        eprintln!("  next: cd {} && rmp doctor", dest.to_string_lossy());
    }

    Ok(())
}

fn resolve_toggle(include_flag: bool, exclude_flag: bool, default_value: bool) -> bool {
    if exclude_flag {
        false
    } else if include_flag {
        true
    } else {
        default_value
    }
}

fn template_root(cwd: &Path) -> Result<PathBuf, CliError> {
    if let Ok(v) = std::env::var("RMP_INIT_TEMPLATE_ROOT") {
        let p = PathBuf::from(v);
        validate_template_root(&p)?;
        return Ok(p);
    }

    if let Ok(root) = find_workspace_root(cwd) {
        validate_template_root(&root)?;
        return Ok(root);
    }

    Err(CliError::user(
        "could not locate template root (set RMP_INIT_TEMPLATE_ROOT or run from an rmp workspace)",
    ))
}

fn validate_template_root(root: &Path) -> Result<(), CliError> {
    for required in ["rust", "android", "ios", "uniffi-bindgen", "flake.nix"] {
        if !root.join(required).exists() {
            return Err(CliError::user(format!(
                "template root is missing `{required}`: {}",
                root.to_string_lossy()
            )));
        }
    }
    Ok(())
}

fn copy_tree_filtered<F>(src: &Path, dst: &Path, skip: &F) -> Result<(), CliError>
where
    F: Fn(&Path, bool) -> bool,
{
    if !src.is_dir() {
        return Err(CliError::operational(format!(
            "template source directory missing: {}",
            src.to_string_lossy()
        )));
    }

    std::fs::create_dir_all(dst)
        .map_err(|e| CliError::operational(format!("failed to create {}: {e}", dst.display())))?;

    copy_tree_filtered_inner(src, dst, Path::new(""), skip)
}

fn copy_tree_filtered_inner<F>(src: &Path, dst: &Path, rel: &Path, skip: &F) -> Result<(), CliError>
where
    F: Fn(&Path, bool) -> bool,
{
    let dir = if rel.as_os_str().is_empty() {
        src.to_path_buf()
    } else {
        src.join(rel)
    };

    let entries = std::fs::read_dir(&dir)
        .map_err(|e| CliError::operational(format!("failed to read {}: {e}", dir.display())))?;

    for entry in entries {
        let entry =
            entry.map_err(|e| CliError::operational(format!("failed to read dir entry: {e}")))?;
        let name = entry.file_name();
        let rel_child = if rel.as_os_str().is_empty() {
            PathBuf::from(name)
        } else {
            rel.join(name)
        };

        let ft = entry.file_type().map_err(|e| {
            CliError::operational(format!("failed to stat {}: {e}", entry.path().display()))
        })?;

        if skip(&rel_child, ft.is_dir()) {
            continue;
        }

        let src_child = src.join(&rel_child);
        let dst_child = dst.join(&rel_child);

        if ft.is_dir() {
            std::fs::create_dir_all(&dst_child).map_err(|e| {
                CliError::operational(format!("failed to create {}: {e}", dst_child.display()))
            })?;
            copy_tree_filtered_inner(src, dst, &rel_child, skip)?;
        } else if ft.is_file() {
            if let Some(parent) = dst_child.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CliError::operational(format!("failed to create {}: {e}", parent.display()))
                })?;
            }
            std::fs::copy(&src_child, &dst_child).map_err(|e| {
                CliError::operational(format!(
                    "failed to copy {} -> {}: {e}",
                    src_child.display(),
                    dst_child.display()
                ))
            })?;
        }
    }

    Ok(())
}

fn copy_file(src: &Path, dst: &Path) -> Result<(), CliError> {
    if !src.is_file() {
        return Err(CliError::operational(format!(
            "missing template file: {}",
            src.to_string_lossy()
        )));
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::operational(format!("failed to create {}: {e}", parent.display()))
        })?;
    }
    std::fs::copy(src, dst).map_err(|e| {
        CliError::operational(format!(
            "failed to copy {} -> {}: {e}",
            src.display(),
            dst.display()
        ))
    })?;
    Ok(())
}

fn write_text_file(path: &Path, content: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::operational(format!("failed to create {}: {e}", parent.display()))
        })?;
    }
    std::fs::write(path, content)
        .map_err(|e| CliError::operational(format!("failed to write {}: {e}", path.display())))?;
    Ok(())
}

fn patch_text(path: &Path, needle: &str, replacement: &str) -> Result<(), CliError> {
    if !path.exists() {
        return Ok(());
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| CliError::operational(format!("failed to read {}: {e}", path.display())))?;
    let patched = content.replace(needle, replacement);
    std::fs::write(path, patched)
        .map_err(|e| CliError::operational(format!("failed to write {}: {e}", path.display())))?;
    Ok(())
}

fn patch_line_prefix(path: &Path, prefix: &str, replacement_line: &str) -> Result<(), CliError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| CliError::operational(format!("failed to read {}: {e}", path.display())))?;

    let mut changed = false;
    let lines: Vec<String> = content
        .lines()
        .map(|line| {
            if !changed && line.trim_start().starts_with(prefix) {
                changed = true;
                replacement_line.to_string()
            } else {
                line.to_string()
            }
        })
        .collect();

    if !changed {
        return Err(CliError::operational(format!(
            "could not patch {}: prefix `{prefix}` not found",
            path.display()
        )));
    }

    let mut out = lines.join("\n");
    if content.ends_with('\n') {
        out.push('\n');
    }

    std::fs::write(path, out)
        .map_err(|e| CliError::operational(format!("failed to write {}: {e}", path.display())))?;
    Ok(())
}

fn validate_org(org: &str) -> Result<(), CliError> {
    if org.trim().is_empty() || !org.contains('.') {
        return Err(CliError::user(
            "--org must be reverse-DNS style (for example: com.example)",
        ));
    }
    validate_bundle_like("org", org)
}

fn validate_bundle_like(label: &str, value: &str) -> Result<(), CliError> {
    if value.trim().is_empty() || !value.contains('.') {
        return Err(CliError::user(format!(
            "{label} must be dot-separated (for example: com.example.app)",
        )));
    }

    for seg in value.split('.') {
        if seg.is_empty()
            || !seg
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(CliError::user(format!(
                "{label} has invalid segment `{seg}` in `{value}`",
            )));
        }
    }

    Ok(())
}

fn java_identifier_segment(input: &str) -> String {
    let mut out = String::new();
    for c in input.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        }
    }
    if out.is_empty() {
        "app".to_string()
    } else if out.chars().next().unwrap().is_ascii_digit() {
        format!("app{out}")
    } else {
        out
    }
}

fn display_name(input: &str) -> String {
    let mut parts: Vec<String> = vec![];
    for tok in input
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|s| !s.is_empty())
    {
        let mut chars = tok.chars();
        if let Some(first) = chars.next() {
            let mut part = String::new();
            part.push(first.to_ascii_uppercase());
            for ch in chars {
                part.push(ch.to_ascii_lowercase());
            }
            parts.push(part);
        }
    }
    if parts.is_empty() {
        "App".to_string()
    } else {
        parts.join(" ")
    }
}

fn workspace_toml() -> String {
    [
        "[workspace]",
        "resolver = \"2\"",
        "members = [",
        "  \"rust\",",
        "  \"uniffi-bindgen\",",
        "]",
        "",
    ]
    .join("\n")
}

fn generated_justfile(include_ios: bool, include_android: bool) -> String {
    let mut lines = vec![
        "set shell := [\"bash\", \"-c\"]".to_string(),
        "".to_string(),
        "default:".to_string(),
        "  @just --list".to_string(),
        "".to_string(),
        "rmp *ARGS:".to_string(),
        "  rmp {{ARGS}}".to_string(),
        "".to_string(),
        "doctor:".to_string(),
        "  rmp doctor".to_string(),
        "".to_string(),
        "devices:".to_string(),
        "  rmp devices list".to_string(),
        "".to_string(),
        "bindings:".to_string(),
        "  rmp bindings all".to_string(),
    ];

    if include_ios {
        lines.push("".to_string());
        lines.push("run-ios:".to_string());
        lines.push("  rmp run ios".to_string());
    }

    if include_android {
        lines.push("".to_string());
        lines.push("run-android:".to_string());
        lines.push("  rmp run android".to_string());
    }

    lines.push("".to_string());
    lines.join("\n")
}

fn generated_readme(name: &str, include_ios: bool, include_android: bool) -> String {
    let mut lines = vec![
        format!("# {}", display_name(name)),
        "".to_string(),
        "Generated by `rmp init`.".to_string(),
        "".to_string(),
        "## Quick Start".to_string(),
        "".to_string(),
        "```bash".to_string(),
        "nix develop".to_string(),
        "rmp doctor".to_string(),
        "rmp bindings all".to_string(),
    ];

    if include_ios {
        lines.push("rmp run ios".to_string());
    }
    if include_android {
        lines.push("rmp run android".to_string());
    }

    lines.push("```".to_string());
    lines.push("".to_string());
    lines.push(
        "Note: this MVP template keeps internal target/module names aligned with current RMP tooling (for fast iteration)."
            .to_string(),
    );
    lines.push("".to_string());
    lines.join("\n")
}

fn generated_rmp_toml(
    project_name: &str,
    org: &str,
    bundle_id: &str,
    app_id: &str,
    include_ios: bool,
    include_android: bool,
) -> String {
    let mut out = vec![
        "[project]".to_string(),
        format!("name = \"{}\"", project_name),
        format!("org = \"{}\"", org),
        "".to_string(),
        "[core]".to_string(),
        "crate = \"pika_core\"".to_string(),
        "bindings = \"uniffi\"".to_string(),
    ];

    if include_ios {
        out.push("".to_string());
        out.push("[ios]".to_string());
        out.push(format!("bundle_id = \"{}\"", bundle_id));
        out.push("scheme = \"Pika\"".to_string());
    }

    if include_android {
        out.push("".to_string());
        out.push("[android]".to_string());
        out.push(format!("app_id = \"{}\"", app_id));
        out.push("avd_name = \"pika_api35\"".to_string());
    }

    out.push("".to_string());
    out.join("\n")
}
