use axum::body::Body;
use axum::http::{Request, Response, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use sea_orm::prelude::Expr;
use sea_orm::*;
use sema_pkg::entity::user;
use serde_json::Value;
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

use sema_pkg::{build_router, AppState};

pub async fn test_app() -> (Router, TempDir) {
    let (app, _state, dir) = test_app_with_state().await;
    (app, dir)
}

pub async fn test_app_with_state() -> (Router, Arc<AppState>, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let blob_dir = dir.path().join("blobs");
    std::fs::create_dir_all(&blob_dir).unwrap();

    // Use DATABASE_URL from env if set (for multi-driver Docker testing),
    // otherwise default to a temp SQLite database
    let db_url = std::env::var("DATABASE_URL").unwrap_or_else(|_| {
        let db_path = dir.path().join("test.db");
        format!("sqlite://{}?mode=rwc", db_path.display())
    });
    let db = sema_pkg::db::connect(&db_url).await;

    let config = sema_pkg::config::Config {
        host: "127.0.0.1".into(),
        port: 0,
        database_url: db_url,
        blob_dir: blob_dir.to_str().unwrap().into(),
        base_url: "http://localhost:3000".into(),
        github_client_id: None,
        github_client_secret: None,
        oauth_token_key: "test-key-32-bytes-long-for-aes!!".into(),
        max_tarball_bytes: 10 * 1024 * 1024,
        max_dependencies: 64,
    };

    let state = Arc::new(AppState { db, config });
    let app = build_router(state.clone());
    (app, state, dir)
}

pub async fn body_json(res: Response<Body>) -> Value {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}

pub async fn body_string(res: Response<Body>) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

pub async fn post_json(app: Router, uri: &str, body: Value) -> Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

pub async fn post_json_with_session(
    app: Router,
    uri: &str,
    body: Value,
    session: &str,
) -> Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .header("cookie", format!("session={session}"))
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

pub async fn get_with_session(app: Router, uri: &str, session: &str) -> Response<Body> {
    app.oneshot(
        Request::builder()
            .uri(uri)
            .header("cookie", format!("session={session}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Register a user and return the session ID
pub async fn register_user(app: Router, username: &str, email: &str) -> String {
    let res = post_json(
        app,
        "/api/v1/auth/register",
        serde_json::json!({
            "username": username,
            "email": email,
            "password": "password123"
        }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    extract_session(res)
}

/// Extract session ID from Set-Cookie header
fn extract_session(res: Response<Body>) -> String {
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap();
    cookie
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("session=")
        .unwrap()
        .to_string()
}

/// Create an API token and return the raw token string
pub async fn create_api_token(app: Router, session: &str, name: &str) -> String {
    let res = post_json_with_session(
        app,
        "/api/v1/tokens",
        serde_json::json!({"name": name}),
        session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = body_json(res).await;
    body["token"].as_str().unwrap().to_string()
}

/// Wrap `data` in a real (decompressible) gzip stream using stored deflate
/// blocks — valid fixtures for the registry's gzip check without pulling a
/// compression dependency into the tests.
pub fn gzip(data: &[u8]) -> Vec<u8> {
    // Header: magic, CM=8 (deflate), FLG=0, MTIME=0, XFL=0, OS=255 (unknown)
    let mut out = vec![0x1f, 0x8b, 0x08, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xff];

    // Deflate stored blocks (max 0xFFFF bytes each); an empty input still
    // needs one final empty block.
    let mut chunks = data.chunks(0xFFFF).peekable();
    loop {
        let chunk: &[u8] = chunks.next().unwrap_or(&[]);
        let is_last = chunks.peek().is_none();
        out.push(if is_last { 0x01 } else { 0x00 }); // BFINAL + BTYPE=00 (stored)
        let len = chunk.len() as u16;
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(chunk);
        if is_last {
            break;
        }
    }

    out.extend_from_slice(&crc32(data).to_le_bytes());
    out.extend_from_slice(&(data.len() as u32).to_le_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// Publish a package via multipart. `data` is gzip-wrapped before upload
/// (the registry rejects non-gzip tarballs); use [`publish_package_raw`]
/// to send bytes verbatim.
pub async fn publish_package(
    app: Router,
    token: &str,
    name: &str,
    version: &str,
    data: &[u8],
) -> Response<Body> {
    publish_package_raw(app, token, name, version, &gzip(data)).await
}

/// Publish a package sending `raw_tarball` bytes exactly as given.
pub async fn publish_package_raw(
    app: Router,
    token: &str,
    name: &str,
    version: &str,
    raw_tarball: &[u8],
) -> Response<Body> {
    let meta = serde_json::json!({"description": format!("A test package: {name}")});
    publish_package_full(
        app,
        token,
        name,
        version,
        raw_tarball,
        &serde_json::to_string(&meta).unwrap(),
    )
    .await
}

/// Publish with full control over the raw tarball bytes and metadata JSON.
pub async fn publish_package_full(
    app: Router,
    token: &str,
    name: &str,
    version: &str,
    raw_tarball: &[u8],
    metadata_json: &str,
) -> Response<Body> {
    let boundary = "----testboundary";
    let mut body_bytes = Vec::new();

    // Metadata field
    body_bytes.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body_bytes.extend_from_slice(b"Content-Disposition: form-data; name=\"metadata\"\r\n\r\n");
    body_bytes.extend_from_slice(metadata_json.as_bytes());
    body_bytes.extend_from_slice(b"\r\n");

    // Tarball field
    body_bytes.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
    body_bytes.extend_from_slice(
        b"Content-Disposition: form-data; name=\"tarball\"; filename=\"pkg.tar.gz\"\r\n",
    );
    body_bytes.extend_from_slice(b"Content-Type: application/gzip\r\n\r\n");
    body_bytes.extend_from_slice(raw_tarball);
    body_bytes.extend_from_slice(b"\r\n");
    body_bytes.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    app.oneshot(
        Request::builder()
            .method("PUT")
            .uri(format!("/api/v1/packages/{name}/{version}"))
            .header("authorization", format!("Bearer {token}"))
            .header(
                "content-type",
                format!("multipart/form-data; boundary={boundary}"),
            )
            .body(Body::from(body_bytes))
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Promote a user to admin via direct DB access.
pub async fn make_admin(state: &Arc<AppState>, username: &str) {
    user::Entity::update_many()
        .col_expr(user::Column::IsAdmin, Expr::value(1i32))
        .filter(user::Column::Username.eq(username))
        .exec(&state.db)
        .await
        .unwrap();
}

/// Register a user and make them admin. Returns session string.
pub async fn register_admin(
    app: Router,
    state: &Arc<AppState>,
    username: &str,
    email: &str,
) -> String {
    let session = register_user(app, username, email).await;
    make_admin(state, username).await;
    session
}

/// Send a POST with empty body and session cookie.
pub async fn post_empty_with_session(app: Router, uri: &str, session: &str) -> Response<Body> {
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri(uri)
            .header("cookie", format!("session={session}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Send a PUT with JSON body and session cookie.
pub async fn put_json_with_session(
    app: Router,
    uri: &str,
    body: Value,
    session: &str,
) -> Response<Body> {
    app.oneshot(
        Request::builder()
            .method("PUT")
            .uri(uri)
            .header("content-type", "application/json")
            .header("cookie", format!("session={session}"))
            .body(Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Send a DELETE with session cookie.
pub async fn delete_with_session(app: Router, uri: &str, session: &str) -> Response<Body> {
    app.oneshot(
        Request::builder()
            .method("DELETE")
            .uri(uri)
            .header("cookie", format!("session={session}"))
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap()
}

/// Get a user's ID from the database.
pub async fn get_user_id(state: &Arc<AppState>, username: &str) -> i64 {
    user::Entity::find()
        .filter(user::Column::Username.eq(username))
        .one(&state.db)
        .await
        .unwrap()
        .unwrap()
        .id
}
