async fn webhook_handler(
    State(state): State<Arc<AppState>>,
    headers: axum::http::HeaderMap,
    body: Bytes,
) -> impl IntoResponse {
    let secret = match &state.webhook_secret {
        Some(s) => s,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({"error": "webhook not configured"})),
            )
                .into_response();
        }
    };

    let signature = match headers
        .get("x-pika-signature-256")
        .and_then(|v| v.to_str().ok())
    {
        Some(s) => s.to_string(),
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing signature"})),
            )
                .into_response();
        }
    };

    if !verify_signature(secret, &body, &signature) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": "invalid signature"})),
        )
            .into_response();
    }

    let (update_count, branch_ref_update_count) = summarize_webhook_ref_updates(&body);
    if branch_ref_update_count > 0 {
        state.forge_runtime.request_mirror_from_webhook();
    } else {
        state.forge_runtime.wake_webhook();
    }
    eprintln!(
        "webhook: received {} ref updates ({} branch ref updates)",
        update_count, branch_ref_update_count
    );

    Json(serde_json::json!({"status": "ok"})).into_response()
}

fn summarize_webhook_ref_updates(payload: &[u8]) -> (usize, usize) {
    let mut update_count = 0usize;
    let mut branch_ref_update_count = 0usize;
    for line in String::from_utf8_lossy(payload).lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        update_count += 1;
        if line
            .split_whitespace()
            .nth(2)
            .is_some_and(|ref_name| ref_name.starts_with("refs/heads/"))
        {
            branch_ref_update_count += 1;
        }
    }
    (update_count, branch_ref_update_count)
}

fn verify_signature(secret: &str, payload: &[u8], signature_header: &str) -> bool {
    let hex_sig = match signature_header.strip_prefix("sha256=") {
        Some(h) => h,
        None => return false,
    };
    let sig_bytes = match hex::decode(hex_sig) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let mut mac =
        Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key size");
    mac.update(payload);
    mac.verify_slice(&sig_bytes).is_ok()
}

// --- Inbox handlers ---

