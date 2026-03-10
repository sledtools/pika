use std::fs;
use std::os::unix::fs::PermissionsExt;
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
fn launch_fulfill_prepared_output_launcher_invokes_helper_from_request_file() {
    let root = std::env::temp_dir().join(format!(
        "pikaci-launch-fulfill-request-cli-{}",
        uuid::Uuid::new_v4()
    ));
    let launch_request_path = root.join("launch-request.json");
    let helper_request_path = root.join("helper-request.json");
    let helper_result_path = root.join("helper-result.json");
    let helper_path = root.join("pikaci-fulfill-prepared-output");
    let realized_path = first_test_nix_store_path();
    let mount_path = root.join("jobs/job-1/staged-linux-rust/workspace-build");
    fs::create_dir_all(&root).expect("create root");

    fs::write(
        &helper_request_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "node_id": "prepare-pika-core-linux-rust-workspace-build",
            "installable": "path:/tmp/snapshot#ci.x86_64-linux.workspaceBuild",
            "output_name": "ci.x86_64-linux.workspaceBuild",
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
        .expect("encode helper request"),
    )
    .expect("write helper request");

    fs::write(
        &helper_path,
        format!(
            "#!/bin/sh\nexec \"{}\" \"$@\"\n",
            env!("CARGO_BIN_EXE_pikaci-fulfill-prepared-output")
        ),
    )
    .expect("write helper shim");
    let mut permissions = fs::metadata(&helper_path)
        .expect("helper metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&helper_path, permissions).expect("set helper executable");

    fs::write(
        &launch_request_path,
        serde_json::to_vec_pretty(&serde_json::json!({
            "schema_version": 1,
            "helper_program": helper_path.display().to_string(),
            "helper_request_path": helper_request_path.display().to_string(),
            "helper_result_path": helper_result_path.display().to_string(),
            "node_id": "prepare-pika-core-linux-rust-workspace-build",
            "output_name": "ci.x86_64-linux.workspaceBuild"
        }))
        .expect("encode launch request"),
    )
    .expect("write launch request");

    let output = Command::new(env!("CARGO_BIN_EXE_pikaci-launch-fulfill-prepared-output"))
        .arg(&launch_request_path)
        .output()
        .expect("run launcher");

    assert!(
        output.status.success(),
        "stdout={}\nstderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let helper_result_body =
        fs::read_to_string(&helper_result_path).expect("read helper result file");
    let helper_result_json: serde_json::Value =
        serde_json::from_str(&helper_result_body).expect("decode helper result file");
    assert_eq!(helper_result_json["status"], "succeeded");
    assert_eq!(
        helper_result_json["request_path"],
        helper_request_path.display().to_string()
    );
    assert_eq!(
        fs::read_link(&mount_path).expect("read symlink"),
        realized_path
    );

    let _ = fs::remove_dir_all(&root);
}
