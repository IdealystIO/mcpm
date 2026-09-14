//! The two HTTP routes attachment bytes travel on: a multipart upload
//! and a download. Server half only — the console reaches them by the
//! paths [`crate::upload_path`] and [`crate::download_path`] spell.
//!
//! Why not `#[server]` fns: those carry a JSON array of arguments and
//! reply with JSON, and a file is neither. So these are plain axum
//! routes merged into the same host, and the one thing they must not
//! do is authenticate differently from `/_srv/*`. They go through the
//! same [`Caller::resolve`] the dispatch gate does, and the worker-key
//! refusal is the same [`require_planner`] the plan edits use.

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::post;
use axum::{Json, Router};
use mcpm_core::{from_bearer, ErrorCode, InlineFile, McpmError, Store, MAX_ATTACHMENT_BYTES};
use serde::Deserialize;

use crate::{require_planner, Caller, WriteResult};

#[derive(Clone)]
struct FilesState {
    store: Store,
    require_auth: bool,
}

/// The routes, to be merged into the host's router under the same CORS
/// layer as everything else.
pub fn router(store: Store, require_auth: bool) -> Router {
    Router::new()
        // One pattern, two verbs: POST takes a SUBJECT id (a feature,
        // a want, a module) and creates; GET takes an ATTACHMENT id
        // and serves.
        .route("/_files/:id", post(upload).get(download))
        // A comment with its files, in one request — see `comment`.
        .route("/_comments/:subject", post(comment))
        // The store's own cap, plus room for the multipart framing and
        // a description. The store still says no to a file over its
        // limit; this only stops a client streaming a gigabyte at a
        // route that would refuse it anyway.
        .layer(DefaultBodyLimit::max(MAX_ATTACHMENT_BYTES + 256 * 1024))
        .with_state(FilesState { store, require_auth })
}

/// `POST /_files/{subject_id}` — multipart with a `file` part and an
/// optional `description` part.
async fn upload(
    State(state): State<FilesState>,
    Path(subject_id): Path<String>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Response {
    let caller = match resolve(&state, &headers, None).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    if let Err(err) = require_planner(&caller) {
        return refusal(StatusCode::FORBIDDEN, err.to_string());
    }

    let mut description = String::new();
    let mut file: Option<(String, String, Bytes)> = None;
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return refusal(StatusCode::BAD_REQUEST, format!("bad multipart body: {e}")),
        };
        match field.name().unwrap_or("") {
            "description" => match field.text().await {
                Ok(t) => description = t,
                Err(e) => return refusal(StatusCode::BAD_REQUEST, format!("bad description part: {e}")),
            },
            "file" => {
                let name = field.file_name().unwrap_or("").to_string();
                let content_type = field.content_type().unwrap_or("").to_string();
                match field.bytes().await {
                    Ok(b) => file = Some((name, content_type, b)),
                    Err(e) => return refusal(StatusCode::BAD_REQUEST, format!("could not read the file part: {e}")),
                }
            }
            // Anything else is ignored rather than refused: a future
            // console may send more, and an old host should not break.
            _ => {}
        }
    }
    let Some((name, content_type, bytes)) = file else {
        return refusal(StatusCode::BAD_REQUEST, "The upload needs a `file` part.".to_string());
    };

    match state
        .store
        .attach_file(caller.name.as_str(), &subject_id, &name, &description, &content_type, bytes.to_vec())
        .await
    {
        Ok(att) => (
            StatusCode::OK,
            Json(WriteResult { message: format!("Attached '{}'.", att.name), id: att.id }),
        )
            .into_response(),
        Err(err) => store_refusal(err),
    }
}

/// `POST /_comments/{subject_id}` — a note, a question or an answer
/// with its files, as one multipart body, so the console makes one
/// request and the store one transaction: `kind`, `body`,
/// `assigned_to` (question), `answers` (answer: the question id), and
/// any number of `file` parts.
///
/// Any caller may comment — a worker key included, because commenting
/// is not planning — and a person (no key, or a console key) does so
/// as a human actor, which is what lets them answer any question.
async fn comment(
    State(state): State<FilesState>,
    Path(subject_id): Path<String>,
    headers: HeaderMap,
    mut form: Multipart,
) -> Response {
    let caller = match resolve(&state, &headers, None).await {
        Ok(c) => c,
        Err(r) => return r,
    };
    let mut kind = "note".to_string();
    let mut body = String::new();
    let mut assigned_to = String::new();
    let mut answers = String::new();
    let mut files: Vec<InlineFile> = Vec::new();
    loop {
        let field = match form.next_field().await {
            Ok(Some(f)) => f,
            Ok(None) => break,
            Err(e) => return refusal(StatusCode::BAD_REQUEST, format!("bad multipart body: {e}")),
        };
        let name = field.name().unwrap_or("").to_string();
        match name.as_str() {
            "file" => {
                let file_name = field.file_name().unwrap_or("").to_string();
                let content_type = field.content_type().unwrap_or("").to_string();
                match field.bytes().await {
                    Ok(b) => files.push(InlineFile {
                        name: file_name,
                        description: String::new(),
                        content_type,
                        bytes: b.to_vec(),
                    }),
                    Err(e) => return refusal(StatusCode::BAD_REQUEST, format!("could not read a file part: {e}")),
                }
            }
            "kind" | "body" | "assigned_to" | "answers" => {
                let text = match field.text().await {
                    Ok(t) => t,
                    Err(e) => return refusal(StatusCode::BAD_REQUEST, format!("bad {name} part: {e}")),
                };
                match name.as_str() {
                    "kind" => kind = text,
                    "body" => body = text,
                    "assigned_to" => assigned_to = text,
                    _ => answers = text,
                }
            }
            _ => {}
        }
    }
    let actor = caller.actor();
    let posted = match kind.as_str() {
        "note" => state.store.add_comment(&actor, &subject_id, &body, files).await,
        "question" => {
            state
                .store
                .ask_question(
                    &actor,
                    &subject_id,
                    &body,
                    Some(assigned_to.as_str()).filter(|a| !a.trim().is_empty()),
                    files,
                )
                .await
        }
        "answer" => state.store.answer_question(&actor, &answers, &body, files).await,
        other => {
            return refusal(
                StatusCode::BAD_REQUEST,
                format!("`kind` must be note, question or answer; got {other:?}."),
            )
        }
    };
    match posted {
        Ok(c) => (
            StatusCode::OK,
            Json(WriteResult {
                message: match c.kind {
                    mcpm_core::CommentKind::Note => "Comment posted.".to_string(),
                    mcpm_core::CommentKind::Question => format!(
                        "Question posted \u{2014} owed by {}.",
                        c.assigned_to.as_deref().unwrap_or("anyone")
                    ),
                    mcpm_core::CommentKind::Answer => "Answer posted.".to_string(),
                },
                id: c.id,
            }),
        )
            .into_response(),
        Err(err) => store_refusal(err),
    }
}

#[derive(Deserialize)]
struct DownloadQuery {
    /// The console's key, hex-encoded — a navigation carries no header.
    key: Option<String>,
}

/// `GET /_files/{attachment_id}` — 302 to a presigned link when the
/// object store mints one, else the bytes themselves.
async fn download(
    State(state): State<FilesState>,
    Path(attachment_id): Path<String>,
    Query(q): Query<DownloadQuery>,
    headers: HeaderMap,
) -> Response {
    let from_query = q.key.as_deref().and_then(crate::unhex);
    if let Err(r) = resolve(&state, &headers, from_query.as_deref()).await {
        return r;
    }
    match state.store.attachment_link(&attachment_id).await {
        Ok(Some(url)) => Redirect::temporary(&url).into_response(),
        Ok(None) => match state.store.attachment_bytes(&attachment_id).await {
            Ok((view, bytes)) => {
                let disposition = format!(
                    "attachment; filename=\"{}\"",
                    view.name.replace(['"', '\\'], "_")
                );
                (
                    StatusCode::OK,
                    [
                        (header::CONTENT_TYPE, view.content_type),
                        (header::CONTENT_DISPOSITION, disposition),
                        (header::CACHE_CONTROL, "private, no-store".to_string()),
                    ],
                    bytes,
                )
                    .into_response()
            }
            Err(err) => store_refusal(err),
        },
        Err(err) => store_refusal(err),
    }
}

/// Who is calling, under the host's posture. A header first; the query
/// key only where the caller passed one, which is only the download.
async fn resolve(
    state: &FilesState,
    headers: &HeaderMap,
    query_key: Option<&str>,
) -> Result<Caller, Response> {
    let from_header = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(from_bearer);
    let presented = from_header.or(query_key);
    Caller::resolve(&state.store, presented, state.require_auth)
        .await
        .map_err(|()| {
            (
                StatusCode::UNAUTHORIZED,
                [(header::WWW_AUTHENTICATE, "Bearer realm=\"mcpm\"")],
                "This request carries no valid API key.",
            )
                .into_response()
        })
}

/// A store refusal, in the store's words, with a status the console
/// can tell apart from a transport failure.
fn store_refusal(err: McpmError) -> Response {
    let status = match err.code {
        ErrorCode::NotFound => StatusCode::NOT_FOUND,
        ErrorCode::Forbidden | ErrorCode::Unauthorized => StatusCode::FORBIDDEN,
        ErrorCode::PendingResolution | ErrorCode::PlanConflict => StatusCode::CONFLICT,
        ErrorCode::Internal => StatusCode::BAD_GATEWAY,
        _ => StatusCode::UNPROCESSABLE_ENTITY,
    };
    refusal(status, err.message)
}

fn refusal(status: StatusCode, message: String) -> Response {
    (status, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], message).into_response()
}
