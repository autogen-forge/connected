//! A small web server for trying the pages in an ordinary browser, behind the `dev-server`
//! feature. It serves `ui/` from disk, so a change shows after a reload, and it answers the
//! same commands the window does, so the pages run against the real app.
//!
//! It listens on the loopback address only, and is never part of a release build.

use std::path::{Path, PathBuf};

use tiny_http::{Header, Method, Request, Response, Server};

use crate::commands::dispatch;
use crate::state::AppState;

pub fn serve(state: AppState, port: u16) {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("ui");
    let server = match Server::http(("127.0.0.1", port)) {
        Ok(server) => server,
        Err(err) => crate::fail(&format!("cannot listen on port {port}: {err}")),
    };
    println!("pages from {} at http://127.0.0.1:{port}/", ui.display());

    for mut request in server.incoming_requests() {
        let path = request.url().split('?').next().unwrap_or("/").to_owned();
        let response = match (request.method(), path.as_str()) {
            (Method::Post, "/call") => call(&state, &mut request),
            (Method::Get, _) => file(&ui, &path),
            _ => text(405, "text/plain", "method not allowed"),
        };
        let _ = request.respond(response);
    }
    state.shutdown();
}

type Reply = Response<std::io::Cursor<Vec<u8>>>;

fn call(state: &AppState, request: &mut Request) -> Reply {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json(400, serde_json::json!({ "error": "unreadable request" }));
    }
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&body) else {
        return json(400, serde_json::json!({ "error": "the body is not JSON" }));
    };
    let command = parsed.get("cmd").and_then(|cmd| cmd.as_str()).unwrap_or_default();
    let args = parsed.get("args").cloned().unwrap_or(serde_json::Value::Null);

    match dispatch(state, command, args) {
        Ok(value) => json(200, value),
        Err(error) => json(400, serde_json::json!({ "error": error })),
    }
}

fn file(ui: &Path, url_path: &str) -> Reply {
    let relative = url_path.trim_start_matches('/');
    let relative = if relative.is_empty() { "index.html" } else { relative };
    if relative.split('/').any(|part| part == ".." || part.is_empty()) {
        return text(404, "text/plain", "not found");
    }

    let full: PathBuf = ui.join(relative);
    match std::fs::read(&full) {
        Ok(bytes) => {
            let kind = match full.extension().and_then(|ext| ext.to_str()) {
                Some("html") => "text/html; charset=utf-8",
                Some("css") => "text/css; charset=utf-8",
                Some("js" | "mjs") => "text/javascript; charset=utf-8",
                Some("png") => "image/png",
                Some("svg") => "image/svg+xml",
                _ => "application/octet-stream",
            };
            reply(200, kind, bytes)
        }
        Err(_) => text(404, "text/plain", "not found"),
    }
}

fn json(status: u16, value: serde_json::Value) -> Reply {
    reply(status, "application/json", value.to_string().into_bytes())
}

fn text(status: u16, kind: &str, body: &str) -> Reply {
    reply(status, kind, body.as_bytes().to_vec())
}

fn reply(status: u16, kind: &str, body: Vec<u8>) -> Reply {
    let header = |name: &str, value: &str| {
        Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("a valid header")
    };
    Response::from_data(body)
        .with_status_code(status)
        .with_header(header("Content-Type", kind))
        .with_header(header("Cache-Control", "no-store"))
}
