//! Agent health: whether the machine behind an agent is actually there.
//!
//! The ledger knows what an agent holds and when it last spoke; it
//! cannot tell a box that is thinking from one that has been reclaimed.
//! So an agent may register a URL — its dev server, typically — and the
//! MCP server probes it on a timer. This module is the policy half:
//! which URLs the server will agree to probe, how a response becomes a
//! [`HealthState`], and the probe loop itself. Where the verdict is
//! written, and the event a change of state raises, are the store's.
//!
//! **The allowlist is the whole safety argument.** A URL an agent hands
//! us is a URL the server will fetch from inside the deployment's
//! network, and a server that fetches whatever it is told is a proxy
//! into that network for anyone holding a worker key — instance
//! metadata, the database's port, another service's admin page. So
//! health checks are OFF until the operator names the host suffixes
//! they cover (`MCPM_HEALTH_HOSTS`), a registration outside them is
//! refused, redirects are never followed (a redirect is a URL nobody
//! checked), and no response body is ever read or stored. The status
//! line is the only thing that comes back.

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::error::{ErrorCode, McpmError};
use crate::store::Store;

/// The environment variable that turns health checks on: a
/// comma-separated list of host suffixes the server may probe, e.g.
/// `.dev.crewforgeos.com`. A bare hostname (no leading dot) matches
/// itself exactly and nothing under it.
pub const HEALTH_HOSTS_ENV: &str = "MCPM_HEALTH_HOSTS";
/// Optional: seconds between sweeps. Default [`DEFAULT_INTERVAL_SECS`].
pub const HEALTH_INTERVAL_ENV: &str = "MCPM_HEALTH_INTERVAL_SECS";

pub const DEFAULT_INTERVAL_SECS: u64 = 30;
/// How long one probe waits before it is `unreachable`. An ALB answers
/// a registered host in milliseconds whether or not the box behind it
/// is up; only a black-holed route takes this long.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(8);
/// How many probes run at once in a sweep.
const SWEEP_CONCURRENCY: usize = 8;

/// What the last probe concluded. The four arms are the four different
/// things a manager does next, which is why there are four and not two.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthState {
    /// Something at that name answered. 2xx, 3xx, or a 4xx other than
    /// 404 — a server that refuses us is still a server.
    Up,
    /// The front door answered 5xx: the name is registered and routed,
    /// and nothing behind it is serving. A dev server mid-rebuild, a
    /// stopped instance whose load-balancer rule survives.
    Down,
    /// 404: nothing is registered at that name any more. On a shared
    /// load balancer or proxy that is the "no such host" answer, and it
    /// is what a removed or scaled-to-zero box looks like.
    Gone,
    /// No HTTP answer at all — DNS, connect, TLS, or the timeout. This
    /// is the network between the server and the URL, not the box, and
    /// it is kept apart from `down` so a broken tunnel does not read as
    /// a fleet outage.
    Unreachable,
}

impl HealthState {
    pub fn as_str(self) -> &'static str {
        match self {
            HealthState::Up => "up",
            HealthState::Down => "down",
            HealthState::Gone => "gone",
            HealthState::Unreachable => "unreachable",
        }
    }

    pub fn parse(s: &str) -> Option<HealthState> {
        match s {
            "up" => Some(HealthState::Up),
            "down" => Some(HealthState::Down),
            "gone" => Some(HealthState::Gone),
            "unreachable" => Some(HealthState::Unreachable),
            _ => None,
        }
    }

    /// The one rule that turns a status line into a state. Every probe
    /// goes through here, so the roster, the event feed and a test all
    /// agree on what a 503 means.
    pub fn classify(status: u16) -> HealthState {
        match status {
            404 => HealthState::Gone,
            500..=599 => HealthState::Down,
            _ => HealthState::Up,
        }
    }
}

impl std::fmt::Display for HealthState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One probe's outcome: the state, and the one line that says why
/// (`HTTP 503`, `timed out after 8s`, `connect: connection refused`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthVerdict {
    pub state: HealthState,
    pub detail: String,
}

/// What this server is willing to probe, read once from the
/// environment. `None` from [`HealthPolicy::from_env`] means health
/// checks are off: nothing is probed and no URL can be registered.
#[derive(Debug, Clone)]
pub struct HealthPolicy {
    /// Lower-cased. A suffix starting with `.` matches any host under
    /// it; one without matches that host exactly.
    suffixes: Vec<String>,
    pub interval: Duration,
}

impl HealthPolicy {
    /// Build from an explicit list of suffixes (tests, embedding).
    pub fn new(suffixes: impl IntoIterator<Item = impl AsRef<str>>) -> Option<HealthPolicy> {
        let suffixes: Vec<String> = suffixes
            .into_iter()
            .map(|s| s.as_ref().trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty() && s != ".")
            .collect();
        if suffixes.is_empty() {
            return None;
        }
        Some(HealthPolicy { suffixes, interval: Duration::from_secs(DEFAULT_INTERVAL_SECS) })
    }

    /// Read `MCPM_HEALTH_HOSTS` (and the optional interval). Unset or
    /// empty means off, which is the default posture: a server probes
    /// nothing it was not told to.
    pub fn from_env() -> Result<Option<HealthPolicy>, McpmError> {
        let hosts = std::env::var(HEALTH_HOSTS_ENV).unwrap_or_default();
        let Some(mut policy) = HealthPolicy::new(hosts.split(',')) else {
            return Ok(None);
        };
        if let Ok(raw) = std::env::var(HEALTH_INTERVAL_ENV) {
            let secs: u64 = raw.trim().parse().map_err(|_| {
                McpmError::new(
                    ErrorCode::Internal,
                    format!("{HEALTH_INTERVAL_ENV} must be a whole number of seconds, got {raw:?}."),
                    serde_json::Value::Null,
                    "Fix the environment and restart.",
                )
            })?;
            policy.interval = Duration::from_secs(secs.max(5));
        }
        Ok(Some(policy))
    }

    /// One line for a startup banner.
    pub fn describe(&self) -> String {
        format!(
            "probing hosts under {} every {}s",
            self.suffixes.join(", "),
            self.interval.as_secs()
        )
    }

    /// Whether `host` (already lower-cased) is one this policy covers.
    fn covers(&self, host: &str) -> bool {
        self.suffixes.iter().any(|s| {
            if let Some(bare) = s.strip_prefix('.') {
                host == bare || host.ends_with(s.as_str())
            } else {
                host == s
            }
        })
    }

    /// Validate a URL an agent wants probed and return it normalised,
    /// or say exactly why it is refused. This is the gate the safety
    /// argument above rests on, so every registration path calls it —
    /// `issue_worker_key`, `get_context`, and anything added later.
    pub fn accept(&self, raw: &str) -> Result<String, McpmError> {
        let raw = raw.trim();
        let refuse = |why: String| {
            McpmError::new(
                ErrorCode::PlanInvalid,
                format!("health_url refused: {why}"),
                serde_json::json!({ "health_url": raw, "allowed": self.suffixes }),
                "Register the URL of the box's own dev server — the one the console would open \
                 to look at it — under a host this server is configured to probe. Nothing was \
                 changed.",
            )
        };
        let url = url::Url::parse(raw).map_err(|e| refuse(format!("not a URL ({e})")))?;
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(refuse(format!("scheme must be http or https, not {other}"))),
        }
        if !url.username().is_empty() || url.password().is_some() {
            return Err(refuse("a URL with credentials in it is never probed".into()));
        }
        let host = match url.host() {
            Some(url::Host::Domain(d)) => d.to_ascii_lowercase(),
            Some(_) => return Err(refuse("the host must be a name, not an address".into())),
            None => return Err(refuse("the URL names no host".into())),
        };
        if !self.covers(&host) {
            return Err(refuse(format!(
                "{host} is not under a host this server probes ({})",
                self.suffixes.join(", ")
            )));
        }
        Ok(url.to_string())
    }
}

/// The client every probe shares. Redirects are refused rather than
/// followed — a 3xx is already an answer, and the place it points to
/// is a URL nobody validated.
pub fn probe_client() -> reqwest::Client {
    // Not a scene element: the linter's `builder::` heuristic matches
    // reqwest's client builder by name alone.
    // idealyst-lint-disable-next-line prefer-ui-macro
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(PROBE_TIMEOUT)
        .user_agent(concat!("mcpm-health/", env!("CARGO_PKG_VERSION")))
        .build()
        .expect("reqwest client with static configuration")
}

/// Probe one URL. Reads the status line and nothing else: the body is
/// dropped unread, so what the box serves never enters the database.
pub async fn probe(client: &reqwest::Client, url: &str) -> HealthVerdict {
    match client.get(url).send().await {
        Ok(response) => {
            let status = response.status().as_u16();
            drop(response);
            HealthVerdict { state: HealthState::classify(status), detail: format!("HTTP {status}") }
        }
        Err(err) => HealthVerdict { state: HealthState::Unreachable, detail: describe_error(&err) },
    }
}

/// The short, credential-free reason a probe got no answer. reqwest's
/// `Display` nests the URL and the full source chain; the roster wants
/// one phrase.
fn describe_error(err: &reqwest::Error) -> String {
    if err.is_timeout() {
        return format!("timed out after {}s", PROBE_TIMEOUT.as_secs());
    }
    // The innermost cause is the useful one ("connection refused",
    // "failed to lookup address information").
    let mut cause: &dyn std::error::Error = err;
    while let Some(next) = cause.source() {
        cause = next;
    }
    let text = cause.to_string();
    let text = text.split(':').next_back().unwrap_or(&text).trim();
    let kind = if err.is_connect() { "connect" } else if err.is_request() { "request" } else { "error" };
    if text.is_empty() {
        kind.to_string()
    } else {
        format!("{kind}: {}", truncate(text, 80))
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let cut: String = s.chars().take(max).collect();
        format!("{cut}\u{2026}")
    }
}

/// One pass over every agent with a URL registered. Public so a test
/// or an operator can run a sweep by hand; the timer just calls it.
pub async fn sweep(store: &Store, client: &reqwest::Client) -> Result<usize, McpmError> {
    let targets = store.health_targets().await?;
    let count = targets.len();
    use futures_util::StreamExt as _;
    let mut stream = futures_util::stream::iter(targets.into_iter().map(|t| async move {
        let verdict = probe(client, &t.url).await;
        (t.name, verdict)
    }))
    .buffer_unordered(SWEEP_CONCURRENCY);
    while let Some((name, verdict)) = stream.next().await {
        if let Err(err) = store.record_health(&name, &verdict).await {
            eprintln!("mcpm health: could not record {name}: {err}");
        }
    }
    Ok(count)
}

/// The probe loop. Runs until the process ends; one per deployment,
/// in the MCP server's HTTP mode, because that process is up whenever
/// a keyed box could be. A sweep that fails to read its targets logs
/// and waits for the next tick rather than exiting: the database
/// hiccup that caused it is the same one that would take the server
/// down anyway if it lasted.
pub async fn run_prober(store: Arc<Store>, policy: Arc<HealthPolicy>) {
    let client = probe_client();
    loop {
        if let Err(err) = sweep(&store, &client).await {
            eprintln!("mcpm health: sweep failed: {err}");
        }
        tokio::time::sleep(policy.interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> HealthPolicy {
        HealthPolicy::new([".dev.crewforgeos.com", "localhost"]).expect("policy")
    }

    #[test]
    fn empty_or_blank_config_is_off() {
        assert!(HealthPolicy::new(Vec::<&str>::new()).is_none());
        assert!(HealthPolicy::new(["", " ", "."]).is_none());
    }

    #[test]
    fn a_host_under_a_dotted_suffix_is_accepted_and_normalised() {
        let ok = policy().accept(" HTTPS://Attendance-Modes.dev.crewforgeos.com ").expect("accept");
        assert_eq!(ok, "https://attendance-modes.dev.crewforgeos.com/");
        // The suffix's own apex counts as under it.
        policy().accept("https://dev.crewforgeos.com/").expect("apex");
    }

    #[test]
    fn a_bare_suffix_matches_only_itself() {
        policy().accept("http://localhost:3500/").expect("exact host");
        let err = policy().accept("http://evil.localhost/").unwrap_err();
        assert!(err.message.contains("not under a host"), "{}", err.message);
    }

    #[test]
    fn everything_the_allowlist_exists_to_stop_is_refused() {
        let p = policy();
        for bad in [
            "https://169.254.169.254/latest/meta-data/",  // an address, not a name
            "http://[::1]/",                               // likewise
            "https://mcpm-mcp.dev.crewforgeos.com.evil.example/", // a lookalike
            "https://dev-crewforgeos.com/",                // not under the suffix
            "ftp://box.dev.crewforgeos.com/",              // scheme
            "https://user:pw@box.dev.crewforgeos.com/",    // credentials
            "box.dev.crewforgeos.com",                     // not a URL
            "",
        ] {
            assert!(p.accept(bad).is_err(), "should refuse {bad:?}");
        }
    }

    #[test]
    fn the_status_line_maps_to_four_states() {
        assert_eq!(HealthState::classify(200), HealthState::Up);
        assert_eq!(HealthState::classify(302), HealthState::Up);
        // A server that refuses us is a server.
        assert_eq!(HealthState::classify(401), HealthState::Up);
        assert_eq!(HealthState::classify(403), HealthState::Up);
        // The load balancer's "no rule for this host".
        assert_eq!(HealthState::classify(404), HealthState::Gone);
        // The front door answered; nothing behind it did.
        assert_eq!(HealthState::classify(502), HealthState::Down);
        assert_eq!(HealthState::classify(503), HealthState::Down);
        assert_eq!(HealthState::classify(504), HealthState::Down);
    }

    /// A listener that answers every connection with one canned status
    /// line and closes. Returns the URL to probe.
    async fn one_shot_server(status: u16) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let body = "this body must never be stored";
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                });
            }
        });
        format!("http://127.0.0.1:{port}/")
    }

    #[tokio::test]
    async fn a_probe_reads_the_status_line_and_nothing_else() {
        let client = probe_client();
        for (status, state) in [
            (200, HealthState::Up),
            (302, HealthState::Up),
            (403, HealthState::Up),
            (404, HealthState::Gone),
            (503, HealthState::Down),
        ] {
            let url = one_shot_server(status).await;
            let v = probe(&client, &url).await;
            assert_eq!(v.state, state, "status {status}");
            assert_eq!(v.detail, format!("HTTP {status}"));
            assert!(!v.detail.contains("body"), "the body never reaches the verdict");
        }
    }

    #[tokio::test]
    async fn nothing_listening_is_unreachable_with_a_short_reason() {
        // Bind and drop: the port was ours a moment ago, so nothing
        // else is on it, and a connect is refused rather than timing out.
        let port = {
            let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            l.local_addr().unwrap().port()
        };
        let v = probe(&probe_client(), &format!("http://127.0.0.1:{port}/")).await;
        assert_eq!(v.state, HealthState::Unreachable);
        assert!(v.detail.starts_with("connect: "), "{}", v.detail);
        assert!(v.detail.len() < 100, "one phrase, not a chain: {}", v.detail);
    }

    #[test]
    fn states_round_trip_through_their_column_text() {
        for s in [HealthState::Up, HealthState::Down, HealthState::Gone, HealthState::Unreachable] {
            assert_eq!(HealthState::parse(s.as_str()), Some(s));
        }
        assert_eq!(HealthState::parse("ok"), None);
    }
}
