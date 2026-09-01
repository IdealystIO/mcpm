//! Console API host. Serves this crate's `#[server]` functions at
//! `/_srv/*` on 127.0.0.1:3210 (override with HOST and PORT).
//!
//! ```
//! cargo run -p api --bin mcpm-web --features server
//! ```
//!
//! # Two postures, and why you cannot pick the dangerous one by accident
//!
//! - **Loopback, open** (the default): no key, permissive CORS, writes
//!   recorded as `console`. This is `idealyst dev` on your laptop, where
//!   the only thing that can reach the port is you.
//! - **Reachable, gated**: every `/_srv/*` call must carry
//!   `Authorization: Bearer <token>` for a live key
//!   (`mcpm-mcp --issue-key --agent console --role console`), and CORS
//!   narrows to `MCPM_ALLOWED_ORIGIN`.
//!
//! The second posture is not opt-in. Setting `HOST` to anything but a
//! loopback address turns it on, because a process listening on the
//! network with no key is the failure this file exists to prevent —
//! and it was previously reachable by adding one environment variable.
//! `MCPM_REQUIRE_AUTH=1` also turns it on, for gating a loopback host.
//!
//! In a container, `-p` publishing only reaches a process listening on
//! the container's external interface, so `HOST=0.0.0.0` is required
//! there — and now brings the gate with it.

use std::sync::Arc;

use mcpm_core::{from_bearer, Store};
use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string());
    let store = Store::connect(&database_url)
        .await
        .expect("connect to the mcpm database (is the devcontainer db up on 55432?)");
    store
        .ensure_project(
            &std::env::var("MCPM_PROJECT_NAME").unwrap_or_else(|_| "control-center".into()),
            "",
            "Managed by mcpm — MCP-first project management for agent crews.",
        )
        .await
        .expect("ensure project row");

    let require_auth = api::auth_required();
    if require_auth {
        let live = store.live_key_count().await.expect("count live API keys");
        if live == 0 {
            eprintln!(
                "mcpm-web: refusing to start — this host requires a key and none exist, so \
                 it would answer nobody.\nmcpm-web: issue one first:\n  \
                 mcpm-mcp --issue-key --agent console --role console"
            );
            std::process::exit(1);
        }
    }

    server::install_state(store.clone());
    // The single interception seam the server-fn primitive offers. One
    // hook, installed once — see the crate's `DispatchHook` docs for why
    // it is a slot and not a list.
    server::install_dispatch_hook(ConsoleGate {
        store: Arc::new(store),
        require_auth,
    });
    // FORCE-LINK: the bin must reference something from the `api` crate
    // or the linker dead-strips its `inventory::submit!` route statics
    // and every /_srv/<fn> 404s with no build error (see the framework's
    // server-fn demo). This touch is that reference.
    let _ = api::Snapshot::default();

    let app: axum::Router = server::router().layer(cors(require_auth));

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3210);
    let host: std::net::IpAddr = match std::env::var("HOST") {
        Ok(h) => h
            .parse()
            .unwrap_or_else(|_| panic!("HOST must be an IP address to bind (e.g. 0.0.0.0), got {h:?}")),
        Err(_) => std::net::Ipv4Addr::LOCALHOST.into(),
    };
    let addr: std::net::SocketAddr = (host, port).into();
    println!("mcpm-web: API at http://{addr}/_srv/<fn> (db: {database_url})");
    println!(
        "mcpm-web: {}",
        if require_auth {
            "authenticated — every call needs Authorization: Bearer <key>"
        } else {
            "open on loopback — no key required"
        }
    );
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}

/// CORS, matched to the posture.
///
/// A permissive policy is fine for a port only you can reach and wrong
/// for one the network can: with the gate on, the browser origin must
/// be named. `MCPM_ALLOWED_ORIGIN` unset then means "no browser
/// origin", which blocks the console rather than quietly re-opening it
/// — a loud misconfiguration beats a silent hole.
fn cors(require_auth: bool) -> CorsLayer {
    if !require_auth {
        return CorsLayer::permissive();
    }
    let Ok(origin) = std::env::var("MCPM_ALLOWED_ORIGIN") else {
        eprintln!(
            "mcpm-web: MCPM_ALLOWED_ORIGIN is unset, so no browser origin is allowed. \
             Set it to the console's URL (e.g. https://console.internal) to let the \
             console reach this host."
        );
        return CorsLayer::new();
    };
    let parsed: Vec<axum::http::HeaderValue> = origin
        .split(',')
        .filter_map(|o| o.trim().parse().ok())
        .collect();
    if parsed.is_empty() {
        eprintln!("mcpm-web: MCPM_ALLOWED_ORIGIN={origin:?} parsed to no usable origin");
    }
    // The ORIGIN allowlist is the control here; the request-header list
    // is not. An allowlist of headers protects nothing — a header is not
    // a capability, and the credential check is what decides the request
    // — while an incomplete one silently breaks the console: the server
    // SDK sends its own `x-srv-schema` alongside `authorization`, and a
    // preflight that omits it fails in the browser BEFORE any 401 is
    // seen, so the console reports a network error and never learns it
    // needs a key. Enumerating transport headers is a maintenance trap
    // with no security value, so it does not get enumerated.
    CorsLayer::new()
        .allow_origin(parsed)
        .allow_methods(tower_http::cors::Any)
        .allow_headers(tower_http::cors::Any)
}

/// Resolves every `/_srv/*` call to a [`api::Caller`], rejecting the
/// request when the posture demands a key and none verifies.
///
/// It always inserts a caller, in both postures, so a handler's
/// `Extension<Caller>` resolves without asking whether auth was
/// configured — and the write it records is attributed to a name either
/// way.
///
/// It deliberately does NOT implement `on_open`: the WebSocket
/// handshake a browser sends carries no header we control, so the event
/// subscription authenticates on its own argument instead (see
/// `api::watch_events`). Gating it here would only reject every socket.
struct ConsoleGate {
    store: Arc<Store>,
    require_auth: bool,
}

impl server::DispatchHook for ConsoleGate {
    fn around<'a>(
        &'a self,
        ctx: &'a mut server::Context,
        next: server::Next,
    ) -> server::HookFuture<'a> {
        Box::pin(async move {
            let presented = ctx
                .headers()
                .get(axum::http::header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(from_bearer)
                .map(str::to_string);

            let caller = match presented {
                Some(token) => match self.store.verify_key(&token).await {
                    Ok(identity) => api::Caller {
                        name: identity.agent_name,
                        role: Some(identity.role),
                    },
                    Err(_) => return Err(unauthorized()),
                },
                // No credential. Fine on an open loopback host; the
                // whole point of the gate otherwise.
                None if !self.require_auth => api::Caller::local(),
                None => return Err(unauthorized()),
            };

            ctx.insert(caller);
            next.run(ctx).await
        })
    }
}

/// 401 with the challenge header, so a client learns what to send.
fn unauthorized() -> server::TransportError {
    server::append_response_header("www-authenticate", "Bearer realm=\"mcpm\"");
    server::TransportError::Server {
        status: 401,
        message: "This request carries no valid API key. Send Authorization: Bearer <token> \
                  for a key issued by this deployment (mcpm-mcp --issue-key)."
            .into(),
    }
}
