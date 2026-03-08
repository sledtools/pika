use std::fs;
use std::path::PathBuf;
use std::process::Command;

fn first_test_nix_store_path() -> PathBuf {
    fs::read_dir("/nix/store")
        .expect("read /nix/store")
        .find_map(|entry| {
            let path = entry.ok()?.path();
            path.exists().then_some(path)
        })
        .expect("find existing /nix/store path for tests")
}

#[test]
fn fulfill_prepared_output_request_helper_replays_requested_mounts() {
    let root = std::env::temp_dir().join(format!(
        "pikaci-fulfill-request-cli-{}",
        uuid::Uuid::new_v4()
    ));
    let request_path = root.join("request.json");
    let realized_path = first_test_nix_store_path();
    let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
    fs::create_dir_all(&root).expect("create root");
    fs::write(
        &request_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "node_id": "prepare-pika-core-linux-rust-workspace-build",
            "installable": "path:/tmp/snapshot#ci.aarch64-linux.workspaceBuild",
            "output_name": "ci.aarch64-linux.workspaceBuild",
            "protocol": "nix_store_path_v1",
            "realized_path": realized_path.display().to_string(),
            "requested_exposures": [
                {
                    "kind": "host_symlink_mount",
                    "path": mount_path.display().to_string(),
                    "access": "read_only"
                }
            ]
        }))
        .expect("encode request"),
    )
    .expect("write request");

    let output = Command::new(env!("CARGO_BIN_EXE_pikaci-fulfill-prepared-output"))
        .arg(&request_path)
        .output()
        .expect("run helper fulfill-prepared-output");

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("decode stdout");
    assert!(stdout.contains(&format!("request={}", request_path.display())));
    assert!(stdout.contains("output=ci.aarch64-linux.workspaceBuild"));
    assert!(stdout.contains("requested_exposures=1"));
    assert_eq!(
        fs::read_link(&mount_path).expect("read symlink"),
        realized_path
    );

    let _ = fs::remove_dir_all(&root);
}
