use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use walkdir::WalkDir;

use crate::cli::{human_log, json_print, CliError, JsonOk};
use crate::config::{load_rmp_toml, RmpAndroid, RmpIos, RmpToml};
use crate::util::{discover_xcode_dev_dir, run_capture, which};

pub fn bindings(
    root: &Path,
    json: bool,
    verbose: bool,
    args: crate::cli::BindingsArgs,
) -> Result<(), CliError> {
    let cfg = load_rmp_toml(root)?;
    let core_pkg = cfg.core.crate_.clone();
    let core_lib = core_pkg.replace('-', "_");

    let ios = cfg.ios.clone();
    let android = cfg.android.clone();

    match args.target {
        crate::cli::BindingsTarget::Swift => {
            let ios = ios.ok_or_else(|| CliError::user("rmp.toml missing [ios] section"))?;
            if args.clean {
                clean_ios(root, verbose)?;
            }
            if args.check {
                check_ios_sources(root, &cfg, verbose)?;
            } else {
                generate_ios_sources(root, &cfg, verbose)?;
            }
            build_ios_xcframework(root, &ios, &core_lib, &core_pkg, verbose)?;
        }
        crate::cli::BindingsTarget::Kotlin => {
            let android =
                android.ok_or_else(|| CliError::user("rmp.toml missing [android] section"))?;
            if args.clean {
                clean_android(root, verbose)?;
            }
            if args.check {
                check_android_sources(root, &cfg, verbose)?;
            } else {
                generate_android_sources(root, &cfg, verbose)?;
            }
            build_android_so(root, &android, &core_pkg, verbose)?;
        }
        crate::cli::BindingsTarget::All => {
            let ios = ios.ok_or_else(|| CliError::user("rmp.toml missing [ios] section"))?;
            let android =
                android.ok_or_else(|| CliError::user("rmp.toml missing [android] section"))?;
            if args.clean {
                clean_ios(root, verbose)?;
                clean_android(root, verbose)?;
            }
            if args.check {
                check_ios_sources(root, &cfg, verbose)?;
                check_android_sources(root, &cfg, verbose)?;
            } else {
                generate_ios_sources(root, &cfg, verbose)?;
                generate_android_sources(root, &cfg, verbose)?;
            }
            build_ios_xcframework(root, &ios, &core_lib, &core_pkg, verbose)?;
            build_android_so(root, &android, &core_pkg, verbose)?;
        }
    }

    if json {
        json_print(&JsonOk {
            ok: true,
            data: serde_json::json!({}),
        });
    }

    Ok(())
}

fn host_cdylib_path(root: &Path, core_lib: &str) -> Result<PathBuf, CliError> {
    let target = root.join("target/release");
    let candidates = [
        target.join(format!("lib{core_lib}.dylib")),
        target.join(format!("lib{core_lib}.so")),
        target.join(format!("{core_lib}.dll")),
    ];
    for p in candidates {
        if p.is_file() {
            return Ok(p);
        }
    }
    Err(CliError::operational(
        "missing built host cdylib (expected target/release/libpika_core.*)",
    ))
}

fn cargo_build_host(root: &Path, core_pkg: &str, verbose: bool) -> Result<(), CliError> {
    if which("cargo").is_none() {
        return Err(CliError::operational("missing `cargo` on PATH"));
    }
    human_log(
        verbose,
        format!("cargo build -p {core_pkg} --release (host)"),
    );
    let status = Command::new("cargo")
        .current_dir(root)
        .arg("build")
        .arg("-p")
        .arg(core_pkg)
        .arg("--release")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::operational(format!("failed to run cargo: {e}")))?;
    if !status.success() {
        return Err(CliError::operational("cargo build failed"));
    }
    Ok(())
}

fn generate_ios_sources(root: &Path, cfg: &RmpToml, verbose: bool) -> Result<(), CliError> {
    let core_pkg = cfg.core.crate_.as_str();
    let core_lib = cfg.core.crate_.replace('-', "_");
    cargo_build_host(root, core_pkg, verbose)?;
    let lib = host_cdylib_path(root, &core_lib)?;
    let out_dir = root.join("ios/Bindings");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CliError::operational(format!("failed to create ios/Bindings: {e}")))?;

    human_log(verbose, "uniffi-bindgen generate (swift)");
    let status = Command::new("cargo")
        .current_dir(root)
        .arg("run")
        .arg("-q")
        .arg("-p")
        .arg("uniffi-bindgen")
        .arg("--")
        .arg("generate")
        .arg("--library")
        .arg(lib)
        .arg("--language")
        .arg("swift")
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--config")
        .arg(root.join("rust/uniffi.toml"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::operational(format!("failed to run uniffi-bindgen: {e}")))?;
    if !status.success() {
        return Err(CliError::operational("uniffi swift generation failed"));
    }
    Ok(())
}

fn generate_android_sources(root: &Path, cfg: &RmpToml, verbose: bool) -> Result<(), CliError> {
    let core_pkg = cfg.core.crate_.as_str();
    let core_lib = cfg.core.crate_.replace('-', "_");
    cargo_build_host(root, core_pkg, verbose)?;
    let lib = host_cdylib_path(root, &core_lib)?;
    let out_dir = root.join("android/app/src/main/java");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CliError::operational(format!("failed to create android java dir: {e}")))?;

    human_log(verbose, "uniffi-bindgen generate (kotlin)");
    let status = Command::new("cargo")
        .current_dir(root)
        .arg("run")
        .arg("-q")
        .arg("-p")
        .arg("uniffi-bindgen")
        .arg("--")
        .arg("generate")
        .arg("--library")
        .arg(lib)
        .arg("--language")
        .arg("kotlin")
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--no-format")
        .arg("--config")
        .arg(root.join("rust/uniffi.toml"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::operational(format!("failed to run uniffi-bindgen: {e}")))?;
    if !status.success() {
        return Err(CliError::operational("uniffi kotlin generation failed"));
    }
    Ok(())
}

fn clean_ios(root: &Path, verbose: bool) -> Result<(), CliError> {
    human_log(verbose, "clean ios bindings + build outputs");
    let bindings = root.join("ios/Bindings");
    for f in [
        "pika_core.swift",
        "pika_coreFFI.h",
        "pika_coreFFI.modulemap",
    ] {
        let p = bindings.join(f);
        let _ = std::fs::remove_file(p);
    }
    let _ = std::fs::remove_dir_all(root.join("ios/.build"));
    let _ = std::fs::remove_dir_all(root.join("ios/Frameworks"));
    Ok(())
}

fn clean_android(root: &Path, verbose: bool) -> Result<(), CliError> {
    human_log(verbose, "clean android bindings + jniLibs");
    let _ =
        std::fs::remove_file(root.join("android/app/src/main/java/com/pika/app/rust/pika_core.kt"));
    let _ = std::fs::remove_dir_all(root.join("android/app/src/main/jniLibs"));
    Ok(())
}

fn check_ios_sources(root: &Path, cfg: &RmpToml, verbose: bool) -> Result<(), CliError> {
    use tempfile::TempDir;

    let core_pkg = cfg.core.crate_.as_str();
    let core_lib = cfg.core.crate_.replace('-', "_");
    cargo_build_host(root, core_pkg, verbose)?;
    let lib = host_cdylib_path(root, &core_lib)?;
    let tmp = TempDir::new().map_err(|e| CliError::operational(format!("tempdir: {e}")))?;
    let out_dir = tmp.path().join("Bindings");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CliError::operational(format!("failed to create temp out dir: {e}")))?;

    let status = Command::new("cargo")
        .current_dir(root)
        .arg("run")
        .arg("-q")
        .arg("-p")
        .arg("uniffi-bindgen")
        .arg("--")
        .arg("generate")
        .arg("--library")
        .arg(lib)
        .arg("--language")
        .arg("swift")
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--config")
        .arg(root.join("rust/uniffi.toml"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::operational(format!("failed to run uniffi-bindgen: {e}")))?;
    if !status.success() {
        return Err(CliError::operational("uniffi swift generation failed"));
    }

    let want = out_dir;
    let have = root.join("ios/Bindings");
    diff_dir(&have, &want, &[".gitkeep"])?;
    Ok(())
}

fn check_android_sources(root: &Path, cfg: &RmpToml, verbose: bool) -> Result<(), CliError> {
    use tempfile::TempDir;

    let core_pkg = cfg.core.crate_.as_str();
    let core_lib = cfg.core.crate_.replace('-', "_");
    cargo_build_host(root, core_pkg, verbose)?;
    let lib = host_cdylib_path(root, &core_lib)?;
    let tmp = TempDir::new().map_err(|e| CliError::operational(format!("tempdir: {e}")))?;
    let out_dir = tmp.path().join("java");
    std::fs::create_dir_all(&out_dir)
        .map_err(|e| CliError::operational(format!("failed to create temp out dir: {e}")))?;

    let status = Command::new("cargo")
        .current_dir(root)
        .arg("run")
        .arg("-q")
        .arg("-p")
        .arg("uniffi-bindgen")
        .arg("--")
        .arg("generate")
        .arg("--library")
        .arg(lib)
        .arg("--language")
        .arg("kotlin")
        .arg("--out-dir")
        .arg(&out_dir)
        .arg("--no-format")
        .arg("--config")
        .arg(root.join("rust/uniffi.toml"))
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::operational(format!("failed to run uniffi-bindgen: {e}")))?;
    if !status.success() {
        return Err(CliError::operational("uniffi kotlin generation failed"));
    }

    let want = out_dir.join("com/pika/app/rust");
    let have = root.join("android/app/src/main/java/com/pika/app/rust");
    diff_dir(&have, &want, &[])?;
    Ok(())
}

fn diff_dir(have: &Path, want: &Path, ignore: &[&str]) -> Result<(), CliError> {
    let mut mismatches: Vec<String> = vec![];

    let mut want_files: Vec<PathBuf> = vec![];
    for ent in WalkDir::new(want).into_iter().flatten() {
        if !ent.file_type().is_file() {
            continue;
        }
        let rel = ent.path().strip_prefix(want).unwrap().to_path_buf();
        if ignore.iter().any(|i| Path::new(i) == rel) {
            continue;
        }
        want_files.push(rel);
    }

    let mut have_files: Vec<PathBuf> = vec![];
    for ent in WalkDir::new(have).into_iter().flatten() {
        if !ent.file_type().is_file() {
            continue;
        }
        let rel = ent.path().strip_prefix(have).unwrap().to_path_buf();
        if ignore.iter().any(|i| Path::new(i) == rel) {
            continue;
        }
        have_files.push(rel);
    }

    want_files.sort();
    have_files.sort();

    for rel in want_files.iter() {
        let w = std::fs::read(want.join(rel)).unwrap_or_default();
        let h = std::fs::read(have.join(rel)).unwrap_or_default();
        if w != h {
            mismatches.push(rel.to_string_lossy().to_string());
        }
    }
    for rel in have_files.iter() {
        if !want_files.contains(rel) {
            mismatches.push(format!("extra: {}", rel.to_string_lossy()));
        }
    }

    if !mismatches.is_empty() {
        return Err(
            CliError::operational("bindings --check failed (generated outputs differ)")
                .with_detail("files", serde_json::json!(mismatches)),
        );
    }
    Ok(())
}

fn build_ios_xcframework(
    root: &Path,
    _ios: &RmpIos,
    core_lib: &str,
    core_pkg: &str,
    verbose: bool,
) -> Result<(), CliError> {
    let dev_dir = discover_xcode_dev_dir()?;

    // Build Rust static libs for iOS (device + simulator) using Xcode toolchain.
    build_ios_staticlibs(root, &dev_dir, core_pkg, core_lib, verbose)?;

    // Ensure headers exist from UniFFI swift generation.
    let bindings_dir = root.join("ios/Bindings");
    let hdr = bindings_dir.join("pika_coreFFI.h");
    let mm = bindings_dir.join("pika_coreFFI.modulemap");
    if !hdr.is_file() || !mm.is_file() {
        return Err(CliError::operational(
            "missing ios/Bindings headers; run `rmp bindings swift` first",
        ));
    }

    // Assemble xcframework.
    let build_dir = root.join("ios/.build");
    let headers_dir = build_dir.join("headers");
    let frameworks_dir = root.join("ios/Frameworks");
    let _ = std::fs::remove_dir_all(&build_dir);
    let _ = std::fs::remove_dir_all(&frameworks_dir);
    std::fs::create_dir_all(&headers_dir)
        .map_err(|e| CliError::operational(format!("failed to create ios/.build: {e}")))?;
    std::fs::create_dir_all(&frameworks_dir)
        .map_err(|e| CliError::operational(format!("failed to create ios/Frameworks: {e}")))?;

    std::fs::copy(&hdr, headers_dir.join("pika_coreFFI.h"))
        .map_err(|e| CliError::operational(format!("copy header: {e}")))?;
    std::fs::copy(&mm, headers_dir.join("module.modulemap"))
        .map_err(|e| CliError::operational(format!("copy modulemap: {e}")))?;

    // lipo: combine sim slices
    let sim_a = build_dir.join("libpika_core_sim.a");
    let status = Command::new("/usr/bin/xcrun")
        .env("DEVELOPER_DIR", &dev_dir)
        .arg("lipo")
        .arg("-create")
        .arg(root.join(format!(
            "target/aarch64-apple-ios-sim/release/lib{core_lib}.a"
        )))
        .arg(root.join(format!("target/x86_64-apple-ios/release/lib{core_lib}.a")))
        .arg("-output")
        .arg(&sim_a)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::operational(format!("failed to run lipo: {e}")))?;
    if !status.success() {
        return Err(CliError::operational("lipo failed"));
    }

    let xcf_name = pascal_case(core_lib);
    let out_xcf = frameworks_dir.join(format!("{xcf_name}.xcframework"));
    let status = Command::new("/usr/bin/xcrun")
        .env("DEVELOPER_DIR", &dev_dir)
        .arg("xcodebuild")
        .arg("-create-xcframework")
        .arg("-library")
        .arg(root.join(format!("target/aarch64-apple-ios/release/lib{core_lib}.a")))
        .arg("-headers")
        .arg(&headers_dir)
        .arg("-library")
        .arg(&sim_a)
        .arg("-headers")
        .arg(&headers_dir)
        .arg("-output")
        .arg(&out_xcf)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| {
            CliError::operational(format!("failed to run xcodebuild -create-xcframework: {e}"))
        })?;
    if !status.success() {
        return Err(CliError::operational(
            "xcodebuild -create-xcframework failed",
        ));
    }

    Ok(())
}

fn build_ios_staticlibs(
    root: &Path,
    dev_dir: &Path,
    core_pkg: &str,
    _core_lib: &str,
    verbose: bool,
) -> Result<(), CliError> {
    if which("cargo").is_none() {
        return Err(CliError::operational("missing `cargo` on PATH"));
    }

    // Replicate `just ios-rust` env behavior.
    let toolchain_bin = dev_dir.join("Toolchains/XcodeDefault.xctoolchain/usr/bin");
    let cc = toolchain_bin.join("clang");
    let cxx = toolchain_bin.join("clang++");
    let ar = toolchain_bin.join("ar");
    let ranlib = toolchain_bin.join("ranlib");
    let ios_min = "17.0";

    // sdk roots
    let sdk_ios = {
        let mut cmd = Command::new("/usr/bin/xcrun");
        cmd.env("DEVELOPER_DIR", dev_dir)
            .arg("--sdk")
            .arg("iphoneos")
            .arg("--show-sdk-path");
        let out = run_capture(cmd)?;
        if !out.status.success() {
            return Err(CliError::operational(
                "xcrun --sdk iphoneos --show-sdk-path failed",
            ));
        }
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };
    let sdk_sim = {
        let mut cmd = Command::new("/usr/bin/xcrun");
        cmd.env("DEVELOPER_DIR", dev_dir)
            .arg("--sdk")
            .arg("iphonesimulator")
            .arg("--show-sdk-path");
        let out = run_capture(cmd)?;
        if !out.status.success() {
            return Err(CliError::operational(
                "xcrun --sdk iphonesimulator --show-sdk-path failed",
            ));
        }
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    };

    human_log(verbose, "cargo build (iOS staticlib slices)");
    for (target, sdkroot, min_flag) in [
        (
            "aarch64-apple-ios",
            sdk_ios.as_str(),
            "-miphoneos-version-min=",
        ),
        (
            "aarch64-apple-ios-sim",
            sdk_sim.as_str(),
            "-mios-simulator-version-min=",
        ),
        (
            "x86_64-apple-ios",
            sdk_sim.as_str(),
            "-mios-simulator-version-min=",
        ),
    ] {
        let mut cmd = Command::new("cargo");
        cmd.current_dir(root)
            .arg("build")
            .arg("-p")
            .arg(core_pkg)
            .arg("--release")
            .arg("--lib")
            .arg("--target")
            .arg(target);

        // Clean out Nix/toolchain vars that break iOS builds.
        for k in [
            "LIBRARY_PATH",
            "SDKROOT",
            "MACOSX_DEPLOYMENT_TARGET",
            "CC",
            "CXX",
            "AR",
            "RANLIB",
            "LD",
        ] {
            cmd.env_remove(k);
        }

        cmd.env("DEVELOPER_DIR", dev_dir)
            .env("CC", &cc)
            .env("CXX", &cxx)
            .env("AR", &ar)
            .env("RANLIB", &ranlib)
            .env("SDKROOT", sdkroot)
            .env("IPHONEOS_DEPLOYMENT_TARGET", ios_min)
            .env(
                "RUSTFLAGS",
                format!(
                    "-C linker={} -C link-arg={}{}",
                    cc.to_string_lossy(),
                    min_flag,
                    ios_min
                ),
            );

        // Ensure linker is clang for relevant targets.
        match target {
            "aarch64-apple-ios" => {
                cmd.env("CARGO_TARGET_AARCH64_APPLE_IOS_LINKER", &cc);
            }
            "aarch64-apple-ios-sim" => {
                cmd.env("CARGO_TARGET_AARCH64_APPLE_IOS_SIM_LINKER", &cc);
            }
            "x86_64-apple-ios" => {
                cmd.env("CARGO_TARGET_X86_64_APPLE_IOS_LINKER", &cc);
            }
            _ => {}
        }

        let status = cmd
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .map_err(|e| CliError::operational(format!("failed to run cargo for {target}: {e}")))?;
        if !status.success() {
            return Err(CliError::operational(format!(
                "cargo build failed for target {target}"
            )));
        }
    }

    Ok(())
}

fn build_android_so(
    root: &Path,
    android: &RmpAndroid,
    core_pkg: &str,
    verbose: bool,
) -> Result<(), CliError> {
    // Ensure local.properties so Gradle doesn't require Android Studio.
    let sdk = std::env::var("ANDROID_HOME")
        .ok()
        .or_else(|| std::env::var("ANDROID_SDK_ROOT").ok())
        .unwrap_or_default();
    if !sdk.is_empty() {
        let lp = root.join("android/local.properties");
        let _ = std::fs::create_dir_all(root.join("android"));
        let contents = format!("sdk.dir={}\n", sdk);
        let _ = std::fs::write(lp, contents);
    }

    human_log(verbose, "cargo ndk build (android .so)");
    let status = Command::new("cargo")
        .current_dir(root)
        .arg("ndk")
        .arg("-o")
        .arg(root.join("android/app/src/main/jniLibs"))
        .arg("-P")
        .arg("26")
        .arg("-t")
        .arg("arm64-v8a")
        .arg("-t")
        .arg("armeabi-v7a")
        .arg("-t")
        .arg("x86_64")
        .arg("build")
        .arg("-p")
        .arg(core_pkg)
        .arg("--release")
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .map_err(|e| CliError::operational(format!("failed to run cargo-ndk: {e}")))?;
    if !status.success() {
        return Err(CliError::operational("cargo ndk build failed"));
    }

    // Best-effort: ensure package name matches config; Android uses applicationId in Gradle.
    let _ = android.app_id.as_str();

    Ok(())
}

/// Convert a snake_case lib name to PascalCase (e.g., "pika_core" → "PikaCore").
fn pascal_case(s: &str) -> String {
    s.split('_')
        .filter(|seg| !seg.is_empty())
        .map(|seg| {
            let mut c = seg.chars();
            match c.next() {
                Some(first) => {
                    let mut part = first.to_uppercase().to_string();
                    part.extend(c);
                    part
                }
                None => String::new(),
            }
        })
        .collect()
}
