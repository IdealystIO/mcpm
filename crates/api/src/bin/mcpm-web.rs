//! Console API host. Serves this crate's `#[server]` functions at
//! `/_srv/*` on port 3210 (override with PORT), CORS-open so the
//! `idealyst dev` page on another port can call it.
//!
//! ```
//! cargo run -p api --bin mcpm-web --features server
//! ```

use tower_http::cors::CorsLayer;

#[tokio::main]
async fn main() {
    let database_url = std::env::var("DATABASE_URL")
        .unwrap_or_else(|_| "postgres://app:app@localhost:55432/app".to_string());
    let store = mcpm_core::Store::connect(&database_url)
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

    server::install_state(store);
    // FORCE-LINK: the bin must reference something from the `api` crate
    // or the linker dead-strips its `inventory::submit!` route statics
    // and every /_srv/<fn> 404s with no build error (see the framework's
    // server-fn demo). This touch is that reference.
    let _ = api::Snapshot::default();

    let app: axum::Router = server::router().layer(CorsLayer::permissive());

    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(3210);
    let addr: std::net::SocketAddr = ([127, 0, 0, 1], port).into();
    println!("mcpm-web: API at http://{addr}/_srv/<fn> (db: {database_url})");
    let listener = tokio::net::TcpListener::bind(addr).await.expect("bind");
    axum::serve(listener, app).await.expect("serve");
}
