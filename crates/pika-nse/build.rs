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

fn main() {
    println!("cargo:rerun-if-env-changed=DEVELOPER_DIR");
    println!("cargo:rerun-if-env-changed=TARGET");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "ios" {
        return;
    }

    let target = env::var("TARGET").unwrap_or_default();
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    let is_sim = target.contains("ios-sim");
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

    let developer_dir = env::var("DEVELOPER_DIR")
        .ok()
        .or_else(|| run_capture_in_path("xcode-select", &["-p"]));

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
    let thin = out_dir.join("libpika_nse_clang_rt_fix.a");
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
    println!("cargo:rustc-link-lib=static=pika_nse_clang_rt_fix");
    println!("cargo:warning=ios link fix: linked {rt} ({archive}) via thin archive for {arch}");
}
