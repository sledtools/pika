#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::net::UdpSocket;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Debug)]
pub struct LocalMoqRelay {
    pub url: String,
    child: Child,
    stderr_lines: Arc<Mutex<Vec<String>>>,
    stderr_thread: Option<JoinHandle<()>>,
}

impl LocalMoqRelay {
    pub fn spawn() -> Option<Self> {
        let bin = find_in_path("moq-relay")?;

        // Pick a free UDP port (race window is small and acceptable for tests).
        let sock = UdpSocket::bind("127.0.0.1:0").ok()?;
        let port = sock.local_addr().ok()?.port();
        drop(sock);

        let url = format!("https://127.0.0.1:{port}/anon");

        // Use a self-signed cert; clients should disable verify for localhost URLs.
        let mut cmd = Command::new(bin);
        cmd.arg("--server-bind")
            .arg(format!("127.0.0.1:{port}"))
            .arg("--tls-generate")
            .arg("127.0.0.1")
            .arg("--auth-public")
            .arg("anon")
            .arg("--log-level")
            .arg("warn")
            .stdout(Stdio::null())
            .stderr(Stdio::piped());

        let mut child = cmd.spawn().ok()?;
        let stderr = child.stderr.take()?;
        let stderr_lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let lines_for_thread = stderr_lines.clone();
        let stderr_thread = std::thread::spawn(move || {
            let reader = BufReader::new(stderr);
            for line in reader.lines().map_while(Result::ok) {
                lines_for_thread.lock().unwrap().push(line);
            }
        });

        // Give it a moment to bind before the first client connect.
        std::thread::sleep(Duration::from_millis(250));
        if let Ok(Some(status)) = child.try_wait() {
            eprintln!(
                "SKIP: moq-relay exited early ({status}); stderr:\n{}",
                stderr_lines.lock().unwrap().join("\n")
            );
            return None;
        }

        Some(Self {
            url,
            child,
            stderr_lines,
            stderr_thread: Some(stderr_thread),
        })
    }

    pub fn dump_stderr(&self, max_lines: usize) {
        let lines = self.stderr_lines.lock().unwrap();
        let start = lines.len().saturating_sub(max_lines);
        eprintln!(
            "--- moq-relay stderr (last {} lines) ---",
            lines.len() - start
        );
        for l in &lines[start..] {
            eprintln!("{l}");
        }
    }
}

impl Drop for LocalMoqRelay {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(t) = self.stderr_thread.take() {
            let _ = t.join();
        }
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
