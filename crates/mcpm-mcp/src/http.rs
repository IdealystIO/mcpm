//! The HTTP transport: one shared server for a cluster of agents that
//! do not run on this machine.
//!
//! MCP's Streamable HTTP shape, minus the parts a stateless server does
//! not need. `POST /mcp` carries one JSON-RPC message and answers with
//! one reply; `GET /mcp` is refused, because the server never initiates
//! anything toward a client.
//!
//! **There is no session state, on purpose.** The stdio server has to
//! carry a session because `get_context` is where an agent says who it
//! is; here the key already said, and it says it again on every
//! request. So there is nothing to pin a `Mcp-Session-Id` to, nothing to
//! expire, and no way for a reconnect to land on somebody else's
//! identity — which is the failure mode session-keyed identity has.

use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use mcpm_core::{from_bearer, McpmError, Store};
use serde_json::{json, Value};

use crate::rpc::{self, Session};

/// Run the listener until the process is killed.
pub async fn serve(store: Store, bind: &str) -> Result<(), String> {
    let addr: SocketAddr = bind
        .parse()
        .map_err(|_| format!("--bind wants HOST:PORT (e.g. 0.0.0.0:3211), got {bind:?}"))?;

    // A listener nobody holds a key for admits nobody. That is almost
    // certainly a half-finished deployment rather than an intended
    // state, and it fails as a wall every agent bounces off with no
    // clue why — so say it here instead.
    let live = store.live_key_count().await.map_err(|e| e.to_string())?;
    if live == 0 {
        return Err("no live API keys — this listener would refuse every request. \
                    Issue one first: mcpm-mcp --issue-key --agent NAME --role manager"
            .to_string());
    }

    let app = Router::new()
        .route("/mcp", post(handle).get(no_server_stream))
        // A plain liveness probe for whatever supervises the process.
        // Unauthenticated on purpose: it reveals only that the port is
        // answering, which anything that can reach the port already knows.
        .route("/health", get(|| async { "ok" }))
        .with_state(Arc::new(store));

    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("cannot bind {addr}: {e}"))?;
    eprintln!("mcpm-mcp: MCP over HTTP at http://{addr}/mcp ({live} live key(s))");
    if addr.ip().is_loopback() {
        eprintln!("mcpm-mcp: bound to loopback — pass --bind 0.0.0.0:{} to accept remote agents", addr.port());
    }
    axum::serve(listener, app)
        .await
        .map_err(|e| format!("serve failed: {e}"))
}

/// One JSON-RPC message in, one reply out.
async fn handle(
    State(store): State<Arc<Store>>,
    headers: HeaderMap,
    body: String,
) -> axum::response::Response {
    let identity = match authenticate(&store, &headers).await {
        Ok(identity) => identity,
        Err(err) => return unauthorized(err),
    };
    // A console key authenticates the dashboard, which reads the store
    // directly. Refusing it here rather than per-tool means a leaked
    // console key cannot reach the agent surface at all.
    if !identity.role.is_agent() {
        return (
            StatusCode::FORBIDDEN,
            Json(envelope(&McpmError::forbidden(
                "connect to the MCP endpoint",
                identity.role,
            ))),
        )
            .into_response();
    }

    let msg: Value = match serde_json::from_str(&body) {
        Ok(msg) => msg,
        Err(err) => {
            return Json(json!({
                "jsonrpc": "2.0", "id": Value::Null,
                "error": { "code": -32700, "message": format!("parse error: {err}") }
            }))
            .into_response()
        }
    };

    // Fresh per request: the key is the whole identity, so there is
    // nothing to carry between them.
    let mut session = Session::keyed(identity);
    match rpc::handle_message(&store, &mut session, &msg).await {
        Some(reply) => Json(reply).into_response(),
        // A notification. Accepted, nothing to say back.
        None => StatusCode::ACCEPTED.into_response(),
    }
}

/// `GET /mcp` opens the server→client stream in the Streamable HTTP
/// spec. This server never pushes, so it says so rather than holding a
/// socket open forever.
async fn no_server_stream() -> impl IntoResponse {
    (
        StatusCode::METHOD_NOT_ALLOWED,
        "mcpm serves MCP over POST /mcp only; it never initiates messages toward a client.",
    )
}

async fn authenticate(
    store: &Store,
    headers: &HeaderMap,
) -> Result<mcpm_core::KeyIdentity, McpmError> {
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(from_bearer)
        .ok_or_else(McpmError::unauthorized)?;
    store.verify_key(token).await
}

/// 401 with the challenge header, so a client knows what to send rather
/// than guessing. The body is the same error envelope the tools use.
fn unauthorized(err: McpmError) -> axum::response::Response {
    (
        StatusCode::UNAUTHORIZED,
        [(axum::http::header::WWW_AUTHENTICATE, "Bearer realm=\"mcpm\"")],
        Json(envelope(&err)),
    )
        .into_response()
}

fn envelope(err: &McpmError) -> Value {
    serde_json::to_value(err).unwrap_or_else(|_| json!({ "code": "INTERNAL" }))
}
