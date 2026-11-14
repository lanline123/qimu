//! qimu-server — the LAN host service (spec §4). POC stage: health check
//! plus the thinnest end of the update-check chain (spec Phase-0 item 3:
//! read manifest -> query host -> answer what changed). Storage is one
//! portable directory: SQLite metadata + content-hash file store; those
//! land with the storage milestone.

use axum::{routing::{get, post}, Json, Router};
use serde_json::{json, Value};

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok", "service": "qimu-server", "version": env!("CARGO_PKG_VERSION") }))
}

/// Stub of the update check: the client (or the PowerPoint COM bridge)
/// posts the embedded manifest of an opened deck; the server answers
/// which binding instances have updates on their tracked branch.
/// POC returns an empty update list with the request echoed for tracing.
async fn check_updates(Json(manifest): Json<Value>) -> Json<Value> {
    Json(json!({
        "deck_id": manifest.get("deck_id").cloned().unwrap_or(Value::Null),
        "updates": [],
        "note": "poc stub — diff engine lands with the storage milestone",
    }))
}

#[tokio::main]
async fn main() {
    let app = Router::new()
        .route("/health", get(health))
        .route("/v1/check-updates", post(check_updates));
    let addr = "127.0.0.1:8730";
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    println!("qimu-server listening on http://{addr}");
    axum::serve(listener, app).await.expect("serve");
}
