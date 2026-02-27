use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;

#[derive(Debug)]
pub struct CapturedRequest {
    pub method: String,
    pub path: String,
    pub headers: HashMap<String, String>,
    pub body: String,
}

/// Spawn a one-shot HTTP mock server that accepts a single request, captures it,
/// and responds with the given status line and body. Returns the base URL and a
/// receiver that yields the captured request.
pub fn spawn_one_shot_server(
    status_line: &str,
    response_body: &str,
) -> (String, mpsc::Receiver<CapturedRequest>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
    let addr = listener.local_addr().expect("read mock server addr");
    let (tx, rx) = mpsc::channel();
    let status_line = status_line.to_string();
    let response_body = response_body.to_string();

    thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept mock request");
        let req = read_http_request(&mut stream);
        tx.send(req).expect("send captured request");

        let response = format!(
            "HTTP/1.1 {status_line}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        stream
            .write_all(response.as_bytes())
            .expect("write mock response");
    });

    (format!("http://{addr}"), rx)
}

fn read_http_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut buf = Vec::new();
    let mut header_end = None;
    let mut content_length = 0usize;

    loop {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).expect("read request bytes");
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if header_end.is_none() {
            header_end = buf
                .windows(4)
                .position(|window| window == b"\r\n\r\n")
                .map(|idx| idx + 4);
            if let Some(end) = header_end {
                let headers = String::from_utf8_lossy(&buf[..end]);
                for line in headers.lines() {
                    if let Some((key, value)) = line.split_once(':') {
                        if key.eq_ignore_ascii_case("content-length") {
                            content_length = value.trim().parse::<usize>().unwrap_or(0);
                        }
                    }
                }
            }
        }
        if let Some(end) = header_end {
            if buf.len() >= end + content_length {
                break;
            }
        }
    }

    let end = header_end.expect("request headers must be present");
    let headers_raw = String::from_utf8_lossy(&buf[..end]);
    let mut lines = headers_raw.lines();
    let request_line = lines.next().expect("request line");
    let mut parts = request_line.split_whitespace();
    let method = parts.next().expect("method").to_string();
    let path = parts.next().expect("path").to_string();
    let mut headers = HashMap::new();
    for line in lines {
        if line.trim().is_empty() {
            break;
        }
        if let Some((key, value)) = line.split_once(':') {
            headers.insert(key.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let body = String::from_utf8(buf[end..end + content_length].to_vec()).expect("utf8 body");

    CapturedRequest {
        method,
        path,
        headers,
        body,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpStream;

    #[test]
    fn mock_server_captures_get() {
        let (url, rx) = spawn_one_shot_server("200 OK", r#"{"ok":true}"#);
        let addr = url.trim_start_matches("http://");
        let mut stream = TcpStream::connect(addr).unwrap();
        stream
            .write_all(b"GET /test-path HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .unwrap();
        let mut resp = String::new();
        stream.read_to_string(&mut resp).unwrap();
        assert!(resp.contains("200 OK"));
        let req = rx.recv().unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/test-path");
    }
}
