use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run_capture(cmd: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(s.trim().to_string())
}

fn run_capture_in_path(cmd: &str, args: &[&str]) -> Option<String> {
    let out = Command::new(cmd).args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    Some(s.trim().to_string())
}

fn emit_rerun_tree(path: &Path) {
    if path.is_file() {
        println!("cargo:rerun-if-changed={}", path.display());
        return;
    }
    if !path.is_dir() {
        return;
    }

    let entries = match std::fs::read_dir(path) {
        Ok(v) => v,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        emit_rerun_tree(&entry.path());
    }
}

fn rust_target_to_zig(target: &str) -> Option<&'static str> {
    match target {
        // iOS
        "aarch64-apple-ios" => Some("aarch64-ios"),
        "aarch64-apple-ios-sim" => Some("aarch64-ios-simulator"),
        "x86_64-apple-ios" => Some("x86_64-ios-simulator"),

        // Android
        "aarch64-linux-android" => Some("aarch64-linux-android"),
        "armv7-linux-androideabi" => Some("arm-linux-androideabi"),
        "x86_64-linux-android" => Some("x86_64-linux-android"),

        // Common desktop targets
        "aarch64-apple-darwin" => Some("aarch64-macos"),
        "x86_64-apple-darwin" => Some("x86_64-macos"),
        "aarch64-unknown-linux-gnu" => Some("aarch64-linux-gnu"),
        "x86_64-unknown-linux-gnu" => Some("x86_64-linux-gnu"),
        _ => None,
    }
}

fn zigmdx_source_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(
        env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| panic!("missing CARGO_MANIFEST_DIR")),
    );

    if let Ok(raw) = env::var("PIKA_ZIG_MDX_PATH") {
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            return p;
        }
        return manifest_dir.join(p);
    }

    manifest_dir.join("../third_party/zig-mdx")
}

fn build_and_link_zigmdx() {
    println!("cargo:rustc-check-cfg=cfg(zig_mdx_disabled)");
    println!("cargo:rerun-if-env-changed=PIKA_ZIG_MDX_SKIP");
    println!("cargo:rerun-if-env-changed=PIKA_ZIG_MDX_PATH");
    println!("cargo:rerun-if-env-changed=PIKA_ZIG_BIN");
    println!("cargo:rerun-if-env-changed=TARGET");
    println!("cargo:rerun-if-env-changed=HOST");

    if env::var("PIKA_ZIG_MDX_SKIP")
        .ok()
        .as_deref()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
    {
        println!("cargo:warning=PIKA_ZIG_MDX_SKIP is set; zig-mdx markdown parsing is disabled");
        println!("cargo:rustc-cfg=zig_mdx_disabled");
        return;
    }

    let source_dir = zigmdx_source_dir();
    if !source_dir.exists() {
        panic!(
            "zig-mdx source not found at {} (set PIKA_ZIG_MDX_PATH to override)",
            source_dir.display()
        );
    }

    emit_rerun_tree(&source_dir.join("src"));
    println!(
        "cargo:rerun-if-changed={}",
        source_dir.join("build.zig").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        source_dir.join("build.zig.zon").display()
    );

    let target = env::var("TARGET").unwrap_or_default();
    let host = env::var("HOST").unwrap_or_default();
    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string()));
    let install_prefix = out_dir.join("zigmdx-out");

    let zig_bin = env::var("PIKA_ZIG_BIN").unwrap_or_else(|_| "zig".to_string());
    let mut cmd = Command::new(&zig_bin);
    cmd.current_dir(&source_dir)
        .arg("build")
        .arg("static")
        .arg("-Doptimize=ReleaseSmall")
        .arg("-p")
        .arg(&install_prefix);

    if target != host {
        let zig_target = rust_target_to_zig(&target).unwrap_or_else(|| {
            panic!(
                "unsupported cross-compilation target for zig-mdx: {target}; set PIKA_ZIG_MDX_SKIP=1 to bypass"
            )
        });
        cmd.arg(format!("-Dtarget={zig_target}"));
    }

    let output = cmd
        .output()
        .unwrap_or_else(|e| panic!("failed to spawn `{zig_bin}` for zig-mdx build: {e}"));

    if !output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        panic!(
            "zig-mdx build failed (status: {}):\nstdout:\n{}\nstderr:\n{}",
            output.status, stdout, stderr
        );
    }

    let lib_dir = install_prefix.join("lib");
    let lib_file = lib_dir.join("libzigmdx.a");
    if !lib_file.exists() {
        panic!(
            "zig-mdx static library not found after build: {}",
            lib_file.display()
        );
    }

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-lib=static=zigmdx");
}

fn main() {
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rerun-if-env-changed=TARGET");

    build_and_link_zigmdx();

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "ios" {
        return;
    }

    // OpenSSL (via libsqlite3-sys sqlcipher) references ___chkstk_darwin on iOS.
    // Apple's clang runtime provides it in libclang_rt.ios*.a, but Rust's link
    // line doesn't always pull that archive in automatically for cdylib/staticlib.
    let target = env::var("TARGET").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let is_sim = target.contains("ios-sim") || target.starts_with("x86_64-apple-ios");
    let rt = if is_sim {
        "clang_rt.iossim"
    } else {
        "clang_rt.ios"
    };
    let archive = if is_sim {
        "libclang_rt.iossim.a"
    } else {
        "libclang_rt.ios.a"
    };

    // Prefer DEVELOPER_DIR if the caller set it (our justfile does). Fall back
    // to xcode-select, and finally to xcrun.
    let developer_dir = env::var("DEVELOPER_DIR")
        .ok()
        .or_else(|| run_capture_in_path("xcode-select", &["-p"]));

    // Best-effort: use clang -print-resource-dir to locate the clang runtime.
    let resource_dir = developer_dir.as_deref().and_then(|dev| {
        let clang = Path::new(dev).join("Toolchains/XcodeDefault.xctoolchain/usr/bin/clang");
        if clang.exists() {
            run_capture(&clang, &["-print-resource-dir"])
        } else {
            None
        }
    });

    let darwin_dir = resource_dir
        .map(|s| PathBuf::from(s).join("lib/darwin"))
        .or_else(|| {
            // Fallback: derive from DEVELOPER_DIR using the common toolchain layout.
            let dev = developer_dir.as_deref()?;
            let clang_root =
                Path::new(dev).join("Toolchains/XcodeDefault.xctoolchain/usr/lib/clang");
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&clang_root)
                .ok()?
                .filter_map(|e| {
                    let p = e.ok()?.path();
                    if p.is_dir() {
                        Some(p)
                    } else {
                        None
                    }
                })
                .collect();
            entries.sort();
            let latest = entries.pop()?;
            Some(latest.join("lib/darwin"))
        })
        .or_else(|| {
            // Last resort: ask xcrun for clang and use -print-resource-dir.
            let clang = run_capture_in_path("xcrun", &["--find", "clang"])?;
            let clang = PathBuf::from(clang);
            run_capture(&clang, &["-print-resource-dir"])
                .map(|s| PathBuf::from(s).join("lib/darwin"))
        });

    let Some(darwin_dir) = darwin_dir else {
        println!(
            "cargo:warning=ios link fix: could not locate clang runtime dir; not linking {archive}"
        );
        return;
    };

    let src = darwin_dir.join(archive);
    if !src.exists() {
        println!(
            "cargo:warning=ios link fix: missing {archive} under {}; not linking it",
            darwin_dir.display()
        );
        return;
    }

    // Rust doesn't accept universal ("fat") static libraries as native link inputs.
    // The clang runtimes in Xcode are universal, so we have to `lipo -thin` them to
    // the current arch into OUT_DIR, then link that thin archive.
    let lipo = run_capture_in_path("xcrun", &["--find", "lipo"])
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("lipo"));

    let arch = match target_arch.as_str() {
        "aarch64" => "arm64",
        "x86_64" => "x86_64",
        "i386" => "i386",
        other => {
            println!(
                "cargo:warning=ios link fix: unknown target arch {other}; not linking {archive}"
            );
            return;
        }
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap_or_else(|_| ".".to_string()));
    let thin = out_dir.join("libpika_clang_rt_fix.a");
    let status = Command::new(&lipo)
        .args(["-thin", arch])
        .arg(&src)
        .args(["-output"])
        .arg(&thin)
        .status();
    match status {
        Ok(s) if s.success() => {}
        _ => {
            println!(
                "cargo:warning=ios link fix: failed to lipo -thin {arch} {}; not linking clang rt",
                src.display()
            );
            return;
        }
    }

    println!("cargo:rustc-link-search=native={}", out_dir.display());
    // Use a generic name; the thin archive is per-target thanks to OUT_DIR.
    println!("cargo:rustc-link-lib=static=pika_clang_rt_fix");
    // Keep the original target-specific info in the logs for debugging.
    println!("cargo:warning=ios link fix: linked {rt} ({archive}) via thin archive for {arch}");
}
