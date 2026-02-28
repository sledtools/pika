#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pika_core::{AppReconciler, AppUpdate};

pub fn wait_until(what: &str, timeout: Duration, f: impl FnMut() -> bool) {
    wait_until_with_poll(what, timeout, Duration::from_millis(100), f);
}

pub fn wait_until_with_poll(
    what: &str,
    timeout: Duration,
    poll: Duration,
    mut f: impl FnMut() -> bool,
) {
    let start = Instant::now();
    while start.elapsed() < timeout {
        if f() {
            return;
        }
        std::thread::sleep(poll);
    }
    panic!("{what}: condition not met within {timeout:?}");
}

pub fn write_config(data_dir: &str, relay_url: &str) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": [relay_url],
        "key_package_relay_urls": [relay_url],
        "call_moq_url": "ws://moq.local/anon",
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

pub fn write_config_with_moq(
    data_dir: &str,
    relay_url: &str,
    kp_relay_url: Option<&str>,
    moq_url: &str,
) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let mut v = serde_json::json!({
        "disable_network": false,
        "relay_urls": [relay_url],
        "call_moq_url": moq_url,
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    if let Some(kp) = kp_relay_url {
        v.as_object_mut().unwrap().insert(
            "key_package_relay_urls".to_string(),
            serde_json::json!([kp]),
        );
    }
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

pub fn write_config_multi(data_dir: &str, relays: &[String], kp_relays: &[String], moq_url: &str) {
    let path = std::path::Path::new(data_dir).join("pika_config.json");
    let v = serde_json::json!({
        "disable_network": false,
        "relay_urls": relays,
        "key_package_relay_urls": kp_relays,
        "call_moq_url": moq_url,
        "call_broadcast_prefix": "pika/calls",
        "call_audio_backend": "synthetic",
    });
    std::fs::write(path, serde_json::to_vec(&v).unwrap()).unwrap();
}

#[derive(Clone)]
pub struct Collector(pub Arc<Mutex<Vec<AppUpdate>>>);

impl Collector {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(Vec::new())))
    }

    #[allow(dead_code)]
    pub fn last_toast(&self) -> Option<String> {
        self.0.lock().unwrap().iter().rev().find_map(|u| match u {
            AppUpdate::FullState(s) => s.toast.clone(),
            _ => None,
        })
    }
}

impl AppReconciler for Collector {
    fn reconcile(&self, update: AppUpdate) {
        self.0.lock().unwrap().push(update);
    }
}
