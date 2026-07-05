use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use sea_orm::*;
use sema_pkg::entity::{dependency, package, package_version};
use tower::ServiceExt;

mod common;
use common::*;

// ── Auth Tests ──

#[tokio::test]
async fn test_register_and_login() {
    let (app, _dir) = test_app().await;

    // Register
    let res = post_json(
        app.clone(),
        "/api/v1/auth/register",
        serde_json::json!({"username": "alice", "email": "alice@example.com", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    assert!(res.headers().get("set-cookie").is_some());

    let body = body_json(res).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["username"], "alice");

    // Login
    let res = post_json(
        app.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"username": "alice", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);
}

#[tokio::test]
async fn test_register_normalizes_case() {
    let (app, _dir) = test_app().await;

    let res = post_json(
        app.clone(),
        "/api/v1/auth/register",
        serde_json::json!({"username": "Alice", "email": "Alice@Example.COM", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = body_json(res).await;
    assert_eq!(body["username"], "alice");

    // Login with different case should work
    let res = post_json(
        app.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"username": "ALICE", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_register_duplicate_username() {
    let (app, _dir) = test_app().await;

    let res = post_json(
        app.clone(),
        "/api/v1/auth/register",
        serde_json::json!({"username": "bob", "email": "bob@example.com", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Same username
    let res = post_json(
        app.clone(),
        "/api/v1/auth/register",
        serde_json::json!({"username": "bob", "email": "bob2@example.com", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
    let body = body_json(res).await;
    // Should NOT leak whether username or email was the conflict
    assert_eq!(body["error"], "Registration failed");
}

#[tokio::test]
async fn test_login_wrong_password() {
    let (app, _dir) = test_app().await;

    register_user(app.clone(), "carol", "carol@example.com").await;

    let res = post_json(
        app.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"username": "carol", "password": "wrongpassword"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_login_nonexistent_user() {
    let (app, _dir) = test_app().await;

    let res = post_json(
        app.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"username": "nobody", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_register_validation() {
    let (app, _dir) = test_app().await;

    // Username too short
    let res = post_json(
        app.clone(),
        "/api/v1/auth/register",
        serde_json::json!({"username": "a", "email": "a@b.com", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Password too short
    let res = post_json(
        app.clone(),
        "/api/v1/auth/register",
        serde_json::json!({"username": "validuser", "email": "v@b.com", "password": "short"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Invalid email
    let res = post_json(
        app.clone(),
        "/api/v1/auth/register",
        serde_json::json!({"username": "validuser", "email": "notanemail", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── Token Tests ──

#[tokio::test]
async fn test_create_and_list_tokens() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "tokenuser", "token@example.com").await;

    // Create token
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/tokens",
        serde_json::json!({"name": "ci-token"}),
        &session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = body_json(res).await;
    assert!(body["token"].as_str().unwrap().starts_with("sema_pat_"));
    assert_eq!(body["name"], "ci-token");

    // List tokens
    let res = get_with_session(app.clone(), "/api/v1/tokens", &session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["tokens"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn test_revoke_token() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "revokeuser", "revoke@example.com").await;

    let res = post_json_with_session(
        app.clone(),
        "/api/v1/tokens",
        serde_json::json!({"name": "temp-token"}),
        &session,
    )
    .await;
    let body = body_json(res).await;
    let token_id = body["id"].as_i64().unwrap();

    // Revoke
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri(format!("/api/v1/tokens/{token_id}"))
                .header("cookie", format!("session={session}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // List should be empty
    let res = get_with_session(app.clone(), "/api/v1/tokens", &session).await;
    let body = body_json(res).await;
    assert_eq!(body["tokens"].as_array().unwrap().len(), 0);
}

// ── Package Tests ──

#[tokio::test]
async fn test_publish_and_get_package() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "publisher", "pub@example.com").await;
    let token = create_api_token(app.clone(), &session, "pub-token").await;

    // Publish
    let res = publish_package(app.clone(), &token, "my-pkg", "1.0.0", b"fake tarball data").await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["package"], "my-pkg");
    assert_eq!(body["version"], "1.0.0");

    // Get package
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/my-pkg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["package"]["name"], "my-pkg");
    assert_eq!(body["versions"].as_array().unwrap().len(), 1);
    assert_eq!(body["owners"][0], "publisher");
}

#[tokio::test]
async fn test_publish_duplicate_version() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "dup-pub", "dup@example.com").await;
    let token = create_api_token(app.clone(), &session, "dup-token").await;

    publish_package(app.clone(), &token, "dup-pkg", "1.0.0", b"data").await;

    let res = publish_package(app.clone(), &token, "dup-pkg", "1.0.0", b"data2").await;
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn test_publish_invalid_semver() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "semver-pub", "sv@example.com").await;
    let token = create_api_token(app.clone(), &session, "sv-token").await;

    let res = publish_package(app.clone(), &token, "sv-pkg", "not-a-version", b"data").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_publish_rejects_non_gzip_tarball() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "gz-pub", "gz@example.com").await;
    let token = create_api_token(app.clone(), &session, "gz-token").await;

    let res = publish_package_raw(app.clone(), &token, "gz-pkg", "1.0.0", b"not gzip data").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert!(body["error"].as_str().unwrap().contains("gzip"));
}

#[tokio::test]
async fn test_publish_rejects_invalid_metadata_json() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "meta-pub", "meta@example.com").await;
    let token = create_api_token(app.clone(), &session, "meta-token").await;

    let res = publish_package_full(
        app.clone(),
        &token,
        "meta-pkg",
        "1.0.0",
        &gzip(b"data"),
        "{not valid json",
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_publish_persists_dependencies() {
    let (app, state, _dir) = test_app_with_state().await;
    let session = register_user(app.clone(), "dep-pub", "dep@example.com").await;
    let token = create_api_token(app.clone(), &session, "dep-token").await;

    let meta = serde_json::json!({
        "description": "has deps",
        "dependencies": [
            {"name": "http-kit", "version_req": "^1.2"},
            {"name": "json-kit", "version_req": ">=0.3, <0.5"},
        ]
    });
    let res = publish_package_full(
        app.clone(),
        &token,
        "dep-pkg",
        "1.0.0",
        &gzip(b"data"),
        &meta.to_string(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);

    let deps = dependency::Entity::find().all(&state.db).await.unwrap();
    let mut names: Vec<&str> = deps.iter().map(|d| d.dependency_name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["http-kit", "json-kit"]);
}

#[tokio::test]
async fn test_publish_invalid_version_req_is_atomic() {
    let (app, state, _dir) = test_app_with_state().await;
    let session = register_user(app.clone(), "atom-pub", "atom@example.com").await;
    let token = create_api_token(app.clone(), &session, "atom-token").await;

    // One valid dep followed by an invalid version_req: the publish must be
    // rejected with no partial state (no package, version, or dependency rows).
    let meta = serde_json::json!({
        "dependencies": [
            {"name": "ok-dep", "version_req": "^1.0"},
            {"name": "bad-dep", "version_req": "not-a-req"},
        ]
    });
    let res = publish_package_full(
        app.clone(),
        &token,
        "atom-pkg",
        "1.0.0",
        &gzip(b"data"),
        &meta.to_string(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let pkg_count = package::Entity::find()
        .filter(package::Column::Name.eq("atom-pkg"))
        .count(&state.db)
        .await
        .unwrap();
    assert_eq!(pkg_count, 0);
    assert_eq!(
        package_version::Entity::find()
            .count(&state.db)
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        dependency::Entity::find().count(&state.db).await.unwrap(),
        0
    );

    // A corrected retry of the same name/version succeeds cleanly.
    let meta = serde_json::json!({
        "dependencies": [{"name": "ok-dep", "version_req": "^1.0"}]
    });
    let res = publish_package_full(
        app.clone(),
        &token,
        "atom-pkg",
        "1.0.0",
        &gzip(b"data"),
        &meta.to_string(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_publish_rejects_too_many_dependencies() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "cap-pub", "cap@example.com").await;
    let token = create_api_token(app.clone(), &session, "cap-token").await;

    // Test config caps at 64 dependencies
    let deps: Vec<serde_json::Value> = (0..65)
        .map(|i| serde_json::json!({"name": format!("dep-{i}"), "version_req": "^1.0"}))
        .collect();
    let meta = serde_json::json!({"dependencies": deps});
    let res = publish_package_full(
        app.clone(),
        &token,
        "cap-pkg",
        "1.0.0",
        &gzip(b"data"),
        &meta.to_string(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("Too many dependencies"));
}

#[tokio::test]
async fn test_publish_tarball_over_axum_default_body_limit() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "big-pub", "big@example.com").await;
    let token = create_api_token(app.clone(), &session, "big-token").await;

    // 3 MB payload: over axum's 2 MB default extractor cap, under the
    // configured 10 MB max_tarball_bytes — must succeed thanks to the
    // DefaultBodyLimit override on the publish route.
    let data = vec![0x5a_u8; 3 * 1024 * 1024];
    let res = publish_package(app.clone(), &token, "big-pkg", "1.0.0", &data).await;
    assert_eq!(res.status(), StatusCode::CREATED);
}

#[tokio::test]
async fn test_publish_not_owner() {
    let (app, _dir) = test_app().await;
    let session1 = register_user(app.clone(), "owner1", "o1@example.com").await;
    let token1 = create_api_token(app.clone(), &session1, "t1").await;

    let session2 = register_user(app.clone(), "intruder", "o2@example.com").await;
    let token2 = create_api_token(app.clone(), &session2, "t2").await;

    // Owner1 publishes
    publish_package(app.clone(), &token1, "owned-pkg", "1.0.0", b"data").await;

    // Intruder tries to publish a new version
    let res = publish_package(app.clone(), &token2, "owned-pkg", "2.0.0", b"data").await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_download_package() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "dluser", "dl@example.com").await;
    let token = create_api_token(app.clone(), &session, "dl-token").await;

    let data = b"hello tarball content";
    publish_package(app.clone(), &token, "dl-pkg", "1.0.0", data).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/dl-pkg/1.0.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("content-type").unwrap(),
        "application/gzip"
    );
    let body = res.into_body().collect().await.unwrap().to_bytes();
    // publish_package gzip-wraps its payload, so the stored blob is gzip(data)
    assert_eq!(&body[..], &gzip(data)[..]);
}

#[tokio::test]
async fn test_yank_package() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "yankuser", "yank@example.com").await;
    let token = create_api_token(app.clone(), &session, "yank-token").await;

    publish_package(app.clone(), &token, "yank-pkg", "1.0.0", b"data").await;

    // Yank
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/packages/yank-pkg/1.0.0/yank")
                .header("authorization", format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Download should fail for yanked
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/yank-pkg/1.0.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ── Search Tests ──

#[tokio::test]
async fn test_search_api() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "searchuser", "search@example.com").await;
    let token = create_api_token(app.clone(), &session, "s-token").await;

    publish_package(app.clone(), &token, "http-client", "0.1.0", b"data").await;
    publish_package(app.clone(), &token, "json-parser", "0.2.0", b"data").await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?q=http")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["total"], 1);
    assert_eq!(body["packages"][0]["name"], "http-client");
}

// ── Ownership Tests ──

#[tokio::test]
async fn test_add_and_remove_owner() {
    let (app, _dir) = test_app().await;
    let session1 = register_user(app.clone(), "origowner", "oo@example.com").await;
    let token1 = create_api_token(app.clone(), &session1, "oo-token").await;
    register_user(app.clone(), "newowner", "no@example.com").await;

    publish_package(app.clone(), &token1, "shared-pkg", "1.0.0", b"data").await;

    // Add owner
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/packages/shared-pkg/owners")
                .header("authorization", format!("Bearer {token1}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({"username": "newowner"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // List owners
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/shared-pkg/owners")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = body_json(res).await;
    assert_eq!(body["owners"].as_array().unwrap().len(), 2);

    // Remove new owner
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/packages/shared-pkg/owners")
                .header("authorization", format!("Bearer {token1}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({"username": "newowner"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Cannot remove last owner
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/packages/shared-pkg/owners")
                .header("authorization", format!("Bearer {token1}"))
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({"username": "origowner"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

// ── Web Page Tests ──

#[tokio::test]
async fn test_web_index() {
    let (app, _dir) = test_app().await;

    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res).await;
    assert!(html.contains("Sema Packages"));
    assert!(html.contains("0 packages published"));
}

#[tokio::test]
async fn test_web_login_page() {
    let (app, _dir) = test_app().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/login")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res).await;
    assert!(html.contains("Sign In"));
}

#[tokio::test]
async fn test_web_account_redirects_when_logged_out() {
    let (app, _dir) = test_app().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/account")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Should redirect to /login
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(res.headers().get("location").unwrap(), "/login");
}

#[tokio::test]
async fn test_web_search_page() {
    let (app, _dir) = test_app().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/search?q=test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res).await;
    assert!(html.contains("Results for"));
    assert!(html.contains("test"));
}

#[tokio::test]
async fn test_web_package_not_found() {
    let (app, _dir) = test_app().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/packages/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_healthz() {
    let (app, _dir) = test_app().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri("/healthz")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

// ── Auth Helpers (unit-level) ──

#[test]
fn test_password_hash_and_verify() {
    let hash = sema_pkg::auth::hash_password("mypassword");
    assert!(sema_pkg::auth::verify_password("mypassword", &hash));
    assert!(!sema_pkg::auth::verify_password("wrongpassword", &hash));
}

#[test]
fn test_token_generation() {
    let token = sema_pkg::auth::generate_token();
    assert!(token.starts_with("sema_pat_"));
    assert!(token.len() > 20);
}

#[test]
fn test_username_validation() {
    assert!(sema_pkg::auth::validate_username("alice").is_ok());
    assert!(sema_pkg::auth::validate_username("alice-bob").is_ok());
    assert!(sema_pkg::auth::validate_username("a").is_err()); // too short
    assert!(sema_pkg::auth::validate_username("-alice").is_err()); // starts with hyphen
    assert!(sema_pkg::auth::validate_username("alice-").is_err()); // ends with hyphen
    assert!(sema_pkg::auth::validate_username("alice bob").is_err()); // space
}

#[test]
fn test_email_validation() {
    assert!(sema_pkg::auth::validate_email("a@b.com").is_ok());
    assert!(sema_pkg::auth::validate_email("notanemail").is_err());
    assert!(sema_pkg::auth::validate_email("ab").is_err());
}

#[test]
fn test_password_validation() {
    assert!(sema_pkg::auth::validate_password("longpassword").is_ok());
    assert!(sema_pkg::auth::validate_password("short").is_err());
}

// ── Meta Registry (GitHub redirect) Tests ──

#[tokio::test]
async fn test_download_github_package_redirects() {
    let (app, state, _dir) = test_app_with_state().await;

    // Insert a GitHub-linked package with tarball_url directly in the DB
    let pkg = package::ActiveModel {
        name: Set("gh-pkg".into()),
        description: Set("A GitHub package".into()),
        source: Set("github".into()),
        github_repo: Set(Some("testowner/testrepo".into())),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .unwrap();

    package_version::ActiveModel {
        package_id: Set(pkg.id),
        version: Set("1.0.0".into()),
        checksum_sha256: Set(String::new()),
        blob_key: Set(String::new()),
        size_bytes: Set(0),
        tarball_url: Set(Some(
            "https://api.github.com/repos/testowner/testrepo/tarball/v1.0.0".into(),
        )),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .unwrap();

    // Download should return a 302 redirect
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/gh-pkg/1.0.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::TEMPORARY_REDIRECT);
    assert_eq!(
        res.headers().get("location").unwrap(),
        "https://api.github.com/repos/testowner/testrepo/tarball/v1.0.0"
    );
}

#[tokio::test]
async fn test_get_package_includes_tarball_url() {
    let (app, state, _dir) = test_app_with_state().await;

    let pkg = package::ActiveModel {
        name: Set("gh-pkg2".into()),
        description: Set("Another GH package".into()),
        source: Set("github".into()),
        github_repo: Set(Some("owner/repo".into())),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .unwrap();

    package_version::ActiveModel {
        package_id: Set(pkg.id),
        version: Set("2.0.0".into()),
        checksum_sha256: Set(String::new()),
        blob_key: Set(String::new()),
        size_bytes: Set(0),
        tarball_url: Set(Some(
            "https://api.github.com/repos/owner/repo/tarball/v2.0.0".into(),
        )),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .unwrap();

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/gh-pkg2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(
        body["versions"][0]["tarball_url"],
        "https://api.github.com/repos/owner/repo/tarball/v2.0.0"
    );
}

#[test]
fn test_blob_path() {
    let path = sema_pkg::blob::blob_path("/data/blobs", "abcdef.tar.gz");
    assert_eq!(path.to_str().unwrap(), "/data/blobs/ab/abcdef.tar.gz");
}

// ── Security Tests ──

#[tokio::test]
async fn test_security_non_admin_cannot_access_any_admin_endpoint() {
    let (app, _state, _dir) = test_app_with_state().await;
    let session = register_user(app.clone(), "user1", "user1@example.com").await;

    // GET endpoints
    for uri in [
        "/api/v1/admin/stats",
        "/api/v1/admin/users",
        "/api/v1/admin/users/1",
        "/api/v1/admin/packages",
        "/api/v1/admin/packages/x",
        "/api/v1/admin/audit",
        "/api/v1/admin/reports",
    ] {
        let res = get_with_session(app.clone(), uri, &session).await;
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "GET {uri} should be 403 for non-admin"
        );
    }

    // POST endpoints with empty body
    for uri in [
        "/api/v1/admin/users/1/ban",
        "/api/v1/admin/users/1/unban",
        "/api/v1/admin/users/1/revoke-tokens",
        "/api/v1/admin/packages/x/yank-all",
        "/api/v1/admin/reports/1/action",
        "/api/v1/admin/reports/1/dismiss",
    ] {
        let res = post_empty_with_session(app.clone(), uri, &session).await;
        assert_eq!(
            res.status(),
            StatusCode::FORBIDDEN,
            "POST {uri} should be 403 for non-admin"
        );
    }

    // PUT with JSON body
    let res = put_json_with_session(
        app.clone(),
        "/api/v1/admin/users/1/role",
        serde_json::json!({"is_admin": true}),
        &session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "PUT /api/v1/admin/users/1/role should be 403"
    );

    // DELETE endpoint
    let res = delete_with_session(app.clone(), "/api/v1/admin/packages/x", &session).await;
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "DELETE /api/v1/admin/packages/x should be 403"
    );

    // POST with JSON body
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/admin/packages/x/transfer",
        serde_json::json!({"to_username": "x"}),
        &session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::FORBIDDEN,
        "POST /api/v1/admin/packages/x/transfer should be 403"
    );
}

#[tokio::test]
async fn test_security_unauthenticated_cannot_access_admin_endpoints() {
    let (app, _dir) = test_app().await;

    // GET endpoints
    for uri in [
        "/api/v1/admin/stats",
        "/api/v1/admin/users",
        "/api/v1/admin/users/1",
        "/api/v1/admin/packages",
        "/api/v1/admin/packages/x",
        "/api/v1/admin/audit",
        "/api/v1/admin/reports",
    ] {
        let res = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "GET {uri} should be 401 unauthenticated"
        );
    }

    // POST endpoints
    for uri in [
        "/api/v1/admin/users/1/ban",
        "/api/v1/admin/users/1/unban",
        "/api/v1/admin/users/1/revoke-tokens",
        "/api/v1/admin/packages/x/yank-all",
        "/api/v1/admin/reports/1/action",
        "/api/v1/admin/reports/1/dismiss",
    ] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::UNAUTHORIZED,
            "POST {uri} should be 401 unauthenticated"
        );
    }

    // PUT with JSON body
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri("/api/v1/admin/users/1/role")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({"is_admin": true})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "PUT /api/v1/admin/users/1/role should be 401"
    );

    // DELETE endpoint
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("DELETE")
                .uri("/api/v1/admin/packages/x")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "DELETE /api/v1/admin/packages/x should be 401"
    );

    // POST with JSON body
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/admin/packages/x/transfer")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_string(&serde_json::json!({"to_username": "x"})).unwrap(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "POST /api/v1/admin/packages/x/transfer should be 401"
    );
}

#[tokio::test]
async fn test_security_banned_user_cannot_access_endpoints() {
    let (app, state, _dir) = test_app_with_state().await;

    // Register admin and victim
    let admin_session = register_admin(app.clone(), &state, "admin1", "admin1@example.com").await;
    let victim_session = register_user(app.clone(), "victim", "victim@example.com").await;

    // Create an API token for victim before ban
    let victim_token = create_api_token(app.clone(), &victim_session, "my-token").await;

    // Get victim's user ID and ban them
    let victim_id = get_user_id(&state, "victim").await;
    let res = post_empty_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/ban"),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Banned user's session should be rejected
    let res = get_with_session(app.clone(), "/api/v1/tokens", &victim_session).await;
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "banned user session: GET /api/v1/tokens should be 401"
    );

    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "some-pkg",
            "report_type": "malware",
            "reason": "suspicious"
        }),
        &victim_session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "banned user session: POST /api/v1/reports should be 401"
    );

    // Banned user's API token should be rejected for publish
    let res = publish_package(app.clone(), &victim_token, "x", "1.0.0", b"fake-tarball").await;
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "banned user token: PUT publish should be 401"
    );
}

#[test]
fn test_validate_package_name_blocks_xss_payloads() {
    use sema_pkg::api::packages::validate_package_name;
    // Legitimate names pass
    for ok in ["x", "my-pkg", "lps-foo-bar", "http_client", "a.b.c", "Pkg9"] {
        assert!(validate_package_name(ok).is_ok(), "{ok} should be valid");
    }
    // XSS breakout / path-traversal / structural payloads are rejected
    for bad in [
        "",
        "');alert(1);('",
        "a'b",
        "a b",
        "a<script>",
        "a\"b",
        "-lead",
        "trail-",
        "..",
        "a..b",
        ".hidden",
        "a/b",
    ] {
        assert!(
            validate_package_name(bad).is_err(),
            "{bad:?} should be rejected"
        );
    }
}

#[tokio::test]
async fn test_publish_rejects_xss_package_name() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "xssuser", "xss@example.com").await;
    let token = create_api_token(app.clone(), &session, "xss-token").await;

    // Percent-encoded "');alert(1);('" as a single path segment
    let evil = "%27%29%3Balert%281%29%3B%28%27";
    let res = publish_package_full(app.clone(), &token, evil, "1.0.0", &gzip(b"data"), "{}").await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn test_publish_rejects_non_http_repository_url() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "repouser", "repo@example.com").await;
    let token = create_api_token(app.clone(), &session, "repo-token").await;

    let meta = serde_json::json!({"repository_url": "javascript:alert(document.cookie)"});
    let res = publish_package_full(
        app.clone(),
        &token,
        "repo-pkg",
        "1.0.0",
        &gzip(b"data"),
        &meta.to_string(),
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert!(body["error"].as_str().unwrap().contains("repository_url"));
}

#[test]
fn test_check_production_secrets_rejects_default_key_with_github() {
    use sema_pkg::config::{Config, DEFAULT_OAUTH_TOKEN_KEY};
    let base = || Config {
        host: "0.0.0.0".into(),
        port: 3000,
        database_url: "sqlite::memory:".into(),
        blob_dir: "data/blobs".into(),
        base_url: "https://example.com".into(),
        github_client_id: None,
        github_client_secret: None,
        oauth_token_key: DEFAULT_OAUTH_TOKEN_KEY.into(),
        max_tarball_bytes: 1024,
        max_dependencies: 64,
    };

    // Default key + no GitHub OAuth: allowed (the key is never used)
    assert!(base().check_production_secrets().is_ok());

    // Default key + GitHub OAuth enabled: refuse to boot
    let mut c = base();
    c.github_client_id = Some("id".into());
    c.github_client_secret = Some("secret".into());
    assert!(c.check_production_secrets().is_err());

    // GitHub OAuth enabled with a real key: allowed
    c.oauth_token_key = "a-unique-production-key-32-bytes!".into();
    assert!(c.check_production_secrets().is_ok());
}

#[tokio::test]
async fn test_logout_invalidates_session_server_side() {
    let (app, _dir) = test_app().await;
    let session = register_user(app.clone(), "logoutuser", "logout@example.com").await;

    // Session works before logout
    let res = get_with_session(app.clone(), "/api/v1/tokens", &session).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Log out
    let res = post_empty_with_session(app.clone(), "/api/v1/auth/logout", &session).await;
    assert_eq!(res.status(), StatusCode::OK);

    // The same session cookie must no longer authenticate — a captured cookie
    // cannot be replayed after logout.
    let res = get_with_session(app.clone(), "/api/v1/tokens", &session).await;
    assert_eq!(
        res.status(),
        StatusCode::UNAUTHORIZED,
        "session must be invalid server-side after logout"
    );
}

#[tokio::test]
async fn test_login_session_cookie_flags() {
    let (app, _dir) = test_app().await;
    register_user(app.clone(), "cookieuser", "cookie@example.com").await;

    let res = post_json(
        app.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"username": "cookieuser", "password": "password123"}),
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let cookie = res.headers().get("set-cookie").unwrap().to_str().unwrap();
    assert!(
        cookie.contains("HttpOnly"),
        "session cookie must be HttpOnly"
    );
    assert!(
        cookie.contains("SameSite=Lax"),
        "session cookie must be SameSite=Lax (CSRF defense)"
    );
    // test_app uses an http:// base_url, so Secure must be absent here; it is
    // covered for https by the cookie_secure unit test below.
    assert!(
        !cookie.contains("Secure"),
        "Secure must be omitted for http base_url"
    );
}

#[test]
fn test_sanitize_return_to_blocks_open_redirect() {
    use sema_pkg::auth::sanitize_return_to;
    // Same-site paths pass through
    assert_eq!(sanitize_return_to("/account"), "/account");
    assert_eq!(sanitize_return_to("/packages/foo"), "/packages/foo");
    // External and protocol-relative targets fall back to /account
    assert_eq!(sanitize_return_to("https://evil.com"), "/account");
    assert_eq!(sanitize_return_to("//evil.com"), "/account");
    assert_eq!(sanitize_return_to("/\\evil.com"), "/account");
    assert_eq!(sanitize_return_to("http://evil.com/x"), "/account");
    assert_eq!(sanitize_return_to("javascript:alert(1)"), "/account");
    assert_eq!(sanitize_return_to("/a\\b"), "/account");
    assert_eq!(sanitize_return_to("evil.com"), "/account");
}

#[test]
fn test_cookie_secure_follows_base_url_scheme() {
    use sema_pkg::auth::{cookie_secure, session_cookie};
    assert!(cookie_secure("https://pkg.sema-lang.com"));
    assert!(!cookie_secure("http://localhost:3000"));
    assert!(session_cookie("abc", true).contains("; Secure"));
    assert!(!session_cookie("abc", false).contains("Secure"));
    // Invariants that must hold regardless of Secure
    let c = session_cookie("abc", true);
    assert!(c.contains("HttpOnly") && c.contains("SameSite=Lax"));
}

#[tokio::test]
async fn test_webhook_rejects_empty_secret() {
    let (app, state, _dir) = test_app_with_state().await;

    // A GitHub-linked package whose webhook_secret is empty (misconfigured).
    package::ActiveModel {
        name: Set("nosecret-pkg".into()),
        source: Set("github".into()),
        github_repo: Set(Some("owner/nosecret".into())),
        webhook_secret: Set(Some(String::new())),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .unwrap();

    // A push for a valid tag with any signature must be refused — an empty
    // secret means HMAC(empty_key, body) is attacker-computable.
    let payload = serde_json::json!({
        "ref": "refs/tags/v1.0.0",
        "repository": {"full_name": "owner/nosecret"}
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/github")
                .header("x-github-event", "push")
                .header("x-hub-signature-256", "sha256=deadbeef")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert!(body["error"].as_str().unwrap().contains("secret"));
}

#[tokio::test]
async fn test_webhook_rejects_bad_signature() {
    let (app, state, _dir) = test_app_with_state().await;

    package::ActiveModel {
        name: Set("secret-pkg".into()),
        source: Set("github".into()),
        github_repo: Set(Some("owner/secret".into())),
        webhook_secret: Set(Some("s3cr3t-webhook-key".into())),
        ..Default::default()
    }
    .insert(&state.db)
    .await
    .unwrap();

    let payload = serde_json::json!({
        "ref": "refs/tags/v1.0.0",
        "repository": {"full_name": "owner/secret"}
    });
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/webhooks/github")
                .header("x-github-event", "push")
                .header(
                    "x-hub-signature-256",
                    "sha256=0000000000000000000000000000000000000000000000000000000000000000",
                )
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&payload).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    let body = body_json(res).await;
    assert_eq!(body["error"], "Invalid signature");
}

#[tokio::test]
async fn test_admin_cannot_ban_self() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "admin1", "admin1@example.com").await;
    let admin_id = get_user_id(&state, "admin1").await;

    let res = post_empty_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{admin_id}/ban"),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(body["error"], "Cannot ban yourself");
}

#[tokio::test]
async fn test_admin_cannot_change_own_role() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "admin1", "admin1@example.com").await;
    let admin_id = get_user_id(&state, "admin1").await;

    let res = put_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{admin_id}/role"),
        serde_json::json!({"is_admin": false}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    let body = body_json(res).await;
    assert_eq!(body["error"], "Cannot change your own admin role");
}

#[tokio::test]
async fn test_submit_report_validates_input() {
    let (app, _state, _dir) = test_app_with_state().await;
    let session = register_user(app.clone(), "reporter", "reporter@example.com").await;

    // Invalid target_type
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "invalid",
            "target_name": "some-pkg",
            "report_type": "malware",
            "reason": "suspicious"
        }),
        &session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "invalid target_type should be 400"
    );

    // Invalid report_type
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "some-pkg",
            "report_type": "unknown",
            "reason": "suspicious"
        }),
        &session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "invalid report_type should be 400"
    );

    // Empty reason
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "some-pkg",
            "report_type": "malware",
            "reason": ""
        }),
        &session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "empty reason should be 400"
    );

    // Empty target_name
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "",
            "report_type": "malware",
            "reason": "suspicious"
        }),
        &session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "empty target_name should be 400"
    );

    // Very long reason (3000 chars)
    let long_reason = "a".repeat(3000);
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "some-pkg",
            "report_type": "malware",
            "reason": long_reason
        }),
        &session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::BAD_REQUEST,
        "very long reason should be 400"
    );
}

#[tokio::test]
async fn test_submit_report_unauthenticated() {
    let (app, _dir) = test_app().await;

    let res = post_json(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "some-pkg",
            "report_type": "malware",
            "reason": "suspicious"
        }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_web_admin_page_requires_admin() {
    let (app, _state, _dir) = test_app_with_state().await;
    let session = register_user(app.clone(), "user1", "user1@example.com").await;

    let res = get_with_session(app.clone(), "/admin", &session).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_web_admin_page_redirects_unauthenticated() {
    let (app, _dir) = test_app().await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/admin")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    // Should redirect to /login
    assert!(
        res.status() == StatusCode::SEE_OTHER
            || res.status() == StatusCode::FOUND
            || res.status() == StatusCode::TEMPORARY_REDIRECT,
        "GET /admin unauthenticated should redirect, got {}",
        res.status()
    );
    let location = res
        .headers()
        .get("location")
        .expect("should have location header");
    assert!(
        location.to_str().unwrap().contains("/login"),
        "redirect should point to /login"
    );
}

#[tokio::test]
async fn test_web_admin_page_loads_for_admin() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "admin1", "admin1@example.com").await;

    let res = get_with_session(app.clone(), "/admin", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let html = body_string(res).await;
    assert!(html.contains("Admin"), "admin page should contain 'Admin'");
}

// ── Admin Happy-Path Tests ──

#[tokio::test]
async fn test_admin_stats() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "stats-admin", "stats-admin@test.com").await;
    let user_session = register_user(app.clone(), "stats-user", "stats-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "stats-token").await;
    publish_package(app.clone(), &token, "stats-pkg", "1.0.0", b"data").await;

    let res = get_with_session(app.clone(), "/api/v1/admin/stats", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["total_users"], 2);
    assert_eq!(body["total_packages"], 1);
    assert_eq!(body["banned_users"], 0);
    assert_eq!(body["open_reports"], 0);
}

#[tokio::test]
async fn test_admin_list_users() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "lu-admin", "lu-admin@test.com").await;
    register_user(app.clone(), "lu-alice", "lu-alice@test.com").await;
    register_user(app.clone(), "lu-bob", "lu-bob@test.com").await;

    let res = get_with_session(app.clone(), "/api/v1/admin/users", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 3);
    for user in users {
        assert!(user["username"].as_str().is_some());
        assert!(user["email"].as_str().is_some());
        assert!(user.get("is_admin").is_some());
        assert!(user.get("package_count").is_some());
        assert!(user.get("banned").is_some());
        assert!(user["created_at"].as_str().is_some());
    }
}

#[tokio::test]
async fn test_admin_list_users_search() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "lus-admin", "lus-admin@test.com").await;
    register_user(app.clone(), "lus-alice", "lus-alice@test.com").await;
    register_user(app.clone(), "lus-bob", "lus-bob@test.com").await;

    let res = get_with_session(app.clone(), "/api/v1/admin/users?q=alice", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["username"], "lus-alice");
}

#[tokio::test]
async fn test_admin_get_user() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "gu-admin", "gu-admin@test.com").await;
    let alice_session = register_user(app.clone(), "gu-alice", "gu-alice@test.com").await;
    let alice_token = create_api_token(app.clone(), &alice_session, "gu-token").await;
    publish_package(app.clone(), &alice_token, "gu-pkg", "1.0.0", b"data").await;

    let alice_id = get_user_id(&state, "gu-alice").await;
    let res = get_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{alice_id}"),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["user"]["username"], "gu-alice");
    assert_eq!(body["user"]["email"], "gu-alice@test.com");
    assert!(body["packages"]
        .as_array()
        .unwrap()
        .contains(&serde_json::json!("gu-pkg")));
    assert_eq!(body["active_token_count"], 1);
}

#[tokio::test]
async fn test_admin_get_user_not_found() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "gunf-admin", "gunf-admin@test.com").await;

    let res = get_with_session(app.clone(), "/api/v1/admin/users/99999", &admin_session).await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_admin_ban_user() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "ban-admin", "ban-admin@test.com").await;
    let victim_session = register_user(app.clone(), "ban-victim", "ban-victim@test.com").await;
    let victim_token = create_api_token(app.clone(), &victim_session, "ban-token").await;

    let victim_id = get_user_id(&state, "ban-victim").await;

    // Ban the user
    let res = post_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/ban"),
        serde_json::json!({"reason": "spam"}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);

    // Victim's session should be invalidated (401)
    let res = get_with_session(app.clone(), "/api/v1/tokens", &victim_session).await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    // Victim's token should be revoked (401 on publish)
    let res = publish_package(app.clone(), &victim_token, "ban-pkg", "1.0.0", b"data").await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_admin_unban_user() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "unban-admin", "unban-admin@test.com").await;
    register_user(app.clone(), "unban-victim", "unban-victim@test.com").await;

    let victim_id = get_user_id(&state, "unban-victim").await;

    // Ban
    post_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/ban"),
        serde_json::json!({"reason": "test"}),
        &admin_session,
    )
    .await;

    // Unban
    let res = post_empty_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/unban"),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);

    // Victim can log in again and access authenticated endpoints
    let login_res = post_json(
        app.clone(),
        "/api/v1/auth/login",
        serde_json::json!({"username": "unban-victim", "password": "password123"}),
    )
    .await;
    assert_eq!(login_res.status(), StatusCode::OK);
    let new_session = login_res
        .headers()
        .get("set-cookie")
        .unwrap()
        .to_str()
        .unwrap()
        .split(';')
        .next()
        .unwrap()
        .strip_prefix("session=")
        .unwrap()
        .to_string();

    let res = get_with_session(app.clone(), "/api/v1/tokens", &new_session).await;
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_admin_revoke_tokens() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "rt-admin", "rt-admin@test.com").await;
    let victim_session = register_user(app.clone(), "rt-victim", "rt-victim@test.com").await;
    let token1 = create_api_token(app.clone(), &victim_session, "rt-tok1").await;
    let token2 = create_api_token(app.clone(), &victim_session, "rt-tok2").await;

    let victim_id = get_user_id(&state, "rt-victim").await;

    let res = post_empty_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/revoke-tokens"),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["revoked"], 2);

    // Both tokens should be unusable
    let res = publish_package(app.clone(), &token1, "rt-pkg1", "1.0.0", b"data").await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let res = publish_package(app.clone(), &token2, "rt-pkg2", "1.0.0", b"data").await;
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_admin_set_role() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "sr-admin", "sr-admin@test.com").await;
    let user_session = register_user(app.clone(), "sr-user1", "sr-user1@test.com").await;

    let user_id = get_user_id(&state, "sr-user1").await;

    // Promote to admin
    let res = put_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{user_id}/role"),
        serde_json::json!({"is_admin": true}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // User1 can now access admin stats
    let res = get_with_session(app.clone(), "/api/v1/admin/stats", &user_session).await;
    assert_eq!(res.status(), StatusCode::OK);

    // Demote back to regular user
    let res = put_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{user_id}/role"),
        serde_json::json!({"is_admin": false}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // User1 should get 403 on admin endpoints
    let res = get_with_session(app.clone(), "/api/v1/admin/stats", &user_session).await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_admin_list_packages() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "lp-admin", "lp-admin@test.com").await;
    let user_session = register_user(app.clone(), "lp-user", "lp-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "lp-token").await;
    publish_package(app.clone(), &token, "lp-alpha", "1.0.0", b"data").await;
    publish_package(app.clone(), &token, "lp-beta", "0.1.0", b"data").await;

    let res = get_with_session(app.clone(), "/api/v1/admin/packages", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let packages = body["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 2);
    for pkg in packages {
        assert!(pkg["name"].as_str().is_some());
        assert!(pkg["description"].as_str().is_some());
        assert!(pkg.get("latest_version").is_some());
        assert!(pkg.get("version_count").is_some());
        assert!(pkg["created_at"].as_str().is_some());
    }
}

#[tokio::test]
async fn test_admin_list_packages_search() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "lps-admin", "lps-admin@test.com").await;
    let user_session = register_user(app.clone(), "lps-user", "lps-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "lps-token").await;
    publish_package(app.clone(), &token, "lps-foo-bar", "1.0.0", b"data").await;
    publish_package(app.clone(), &token, "lps-baz-qux", "1.0.0", b"data").await;

    let res = get_with_session(app.clone(), "/api/v1/admin/packages?q=foo", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let packages = body["packages"].as_array().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0]["name"], "lps-foo-bar");
}

#[tokio::test]
async fn test_admin_yank_all() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "ya-admin", "ya-admin@test.com").await;
    let user_session = register_user(app.clone(), "ya-user", "ya-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "ya-token").await;
    publish_package(app.clone(), &token, "ya-pkg", "1.0.0", b"data1").await;
    publish_package(app.clone(), &token, "ya-pkg", "2.0.0", b"data2").await;
    publish_package(app.clone(), &token, "ya-pkg", "3.0.0", b"data3").await;

    let res = post_empty_with_session(
        app.clone(),
        "/api/v1/admin/packages/ya-pkg/yank-all",
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);
    assert_eq!(body["yanked"], 3);

    // Downloading any version should return 404
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/ya-pkg/1.0.0/download")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_admin_remove_package() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "rp-admin", "rp-admin@test.com").await;
    let user_session = register_user(app.clone(), "rp-user", "rp-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "rp-token").await;
    publish_package(app.clone(), &token, "rp-pkg", "1.0.0", b"data").await;

    let res =
        delete_with_session(app.clone(), "/api/v1/admin/packages/rp-pkg", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);

    // Package should no longer exist
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/rp-pkg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_admin_transfer_ownership() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "to-admin", "to-admin@test.com").await;
    let owner_session = register_user(app.clone(), "to-owner1", "to-owner1@test.com").await;
    let owner_token = create_api_token(app.clone(), &owner_session, "to-tok1").await;
    let new_owner_session = register_user(app.clone(), "to-newowner", "to-newowner@test.com").await;
    let new_owner_token = create_api_token(app.clone(), &new_owner_session, "to-tok2").await;

    publish_package(app.clone(), &owner_token, "to-pkg", "1.0.0", b"data").await;

    // Transfer ownership
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/admin/packages/to-pkg/transfer",
        serde_json::json!({"to_username": "to-newowner"}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["ok"], true);

    // New owner can publish a new version
    let res = publish_package(app.clone(), &new_owner_token, "to-pkg", "2.0.0", b"data2").await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Original owner cannot publish
    let res = publish_package(app.clone(), &owner_token, "to-pkg", "3.0.0", b"data3").await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_admin_audit_log() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "al-admin", "al-admin@test.com").await;
    let victim_session = register_user(app.clone(), "al-victim", "al-victim@test.com").await;
    let token = create_api_token(app.clone(), &victim_session, "al-token").await;
    publish_package(app.clone(), &token, "al-pkg", "1.0.0", b"data").await;

    let victim_id = get_user_id(&state, "al-victim").await;

    // Ban the user (generates audit entry)
    post_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/ban"),
        serde_json::json!({"reason": "audit test"}),
        &admin_session,
    )
    .await;

    // Check audit log
    let res = get_with_session(app.clone(), "/api/v1/admin/audit", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let entries = body["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    let actions: Vec<&str> = entries
        .iter()
        .map(|e| e["action"].as_str().unwrap())
        .collect();
    assert!(actions.contains(&"ban_user"));
}

#[tokio::test]
async fn test_admin_audit_filter() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "af-admin", "af-admin@test.com").await;
    register_user(app.clone(), "af-victim", "af-victim@test.com").await;

    let victim_id = get_user_id(&state, "af-victim").await;

    // Ban (generates ban_user audit entry)
    post_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/ban"),
        serde_json::json!({"reason": "filter test"}),
        &admin_session,
    )
    .await;

    // Unban (generates unban_user audit entry)
    post_empty_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/unban"),
        &admin_session,
    )
    .await;

    // Filter by ban_user action
    let res = get_with_session(
        app.clone(),
        "/api/v1/admin/audit?action=ban_user",
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let entries = body["entries"].as_array().unwrap();
    assert!(!entries.is_empty());
    for entry in entries {
        assert_eq!(entry["action"], "ban_user");
    }
}

#[tokio::test]
async fn test_admin_reports_lifecycle() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "rl-admin", "rl-admin@test.com").await;
    let user_session = register_user(app.clone(), "rl-reporter", "rl-reporter@test.com").await;

    // User submits a report
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "suspicious-pkg",
            "report_type": "spam",
            "reason": "This package is spam"
        }),
        &user_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Admin lists reports and sees it
    let res = get_with_session(app.clone(), "/api/v1/admin/reports", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let reports = body["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 1);
    let report_id = reports[0]["id"].as_i64().unwrap();
    assert_eq!(reports[0]["status"], "open");

    // Admin dismisses the report
    let res = post_empty_with_session(
        app.clone(),
        &format!("/api/v1/admin/reports/{report_id}/dismiss"),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Open reports list should be empty now
    let res = get_with_session(app.clone(), "/api/v1/admin/reports", &admin_session).await;
    let body = body_json(res).await;
    assert_eq!(body["reports"].as_array().unwrap().len(), 0);

    // Submit another report
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "user",
            "target_name": "bad-actor",
            "report_type": "abuse",
            "reason": "Abusive behavior"
        }),
        &user_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Admin actions the second report
    let res = get_with_session(app.clone(), "/api/v1/admin/reports", &admin_session).await;
    let body = body_json(res).await;
    let reports = body["reports"].as_array().unwrap();
    assert_eq!(reports.len(), 1);
    let report_id2 = reports[0]["id"].as_i64().unwrap();

    let res = post_empty_with_session(
        app.clone(),
        &format!("/api/v1/admin/reports/{report_id2}/action"),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // No more open reports
    let res = get_with_session(app.clone(), "/api/v1/admin/reports", &admin_session).await;
    let body = body_json(res).await;
    assert_eq!(body["reports"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn test_admin_list_users_filter_banned() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "fb-admin", "fb-admin@test.com").await;
    register_user(app.clone(), "fb-good", "fb-good@test.com").await;
    register_user(app.clone(), "fb-bad", "fb-bad@test.com").await;

    let bad_id = get_user_id(&state, "fb-bad").await;

    // Ban one user
    post_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{bad_id}/ban"),
        serde_json::json!({"reason": "test"}),
        &admin_session,
    )
    .await;

    // Filter banned
    let res = get_with_session(
        app.clone(),
        "/api/v1/admin/users?status=banned",
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 1);
    assert_eq!(users[0]["username"], "fb-bad");

    // Filter active -- should exclude banned user
    let res = get_with_session(
        app.clone(),
        "/api/v1/admin/users?status=active",
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let users = body["users"].as_array().unwrap();
    assert_eq!(users.len(), 2);
    let usernames: Vec<&str> = users
        .iter()
        .map(|u| u["username"].as_str().unwrap())
        .collect();
    assert!(!usernames.contains(&"fb-bad"));
}

// ── Coverage Gap Tests ──

#[tokio::test]
async fn test_download_tracking() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session = register_admin(app.clone(), &state, "dt-admin", "dt-admin@test.com").await;
    let user_session = register_user(app.clone(), "dt-user", "dt-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "dt-token").await;

    publish_package(app.clone(), &token, "dt-pkg", "1.0.0", b"dt-data").await;

    // Download twice
    for _ in 0..2 {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/packages/dt-pkg/1.0.0/download")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    // Check package endpoint for total_downloads
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/dt-pkg")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert!(
        body["total_downloads"].as_i64().unwrap_or(0) >= 2,
        "expected total_downloads >= 2, got {:?}",
        body["total_downloads"]
    );

    // Check admin stats for total_downloads
    let res = get_with_session(app.clone(), "/api/v1/admin/stats", &admin_session).await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert!(
        body["total_downloads"].as_i64().unwrap_or(0) >= 2,
        "expected admin stats total_downloads >= 2, got {:?}",
        body["total_downloads"]
    );
}

#[tokio::test]
async fn test_admin_get_package_detail() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "dpd-admin", "dpd-admin@test.com").await;
    let user_session = register_user(app.clone(), "dpd-user1", "dpd-user1@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "dpd-token").await;

    // Publish two versions
    let res = publish_package(app.clone(), &token, "detail-pkg", "1.0.0", b"data1").await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let res = publish_package(app.clone(), &token, "detail-pkg", "2.0.0", b"data2").await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Submit a report against it
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "detail-pkg",
            "report_type": "malware",
            "reason": "Looks suspicious"
        }),
        &user_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Admin gets package detail
    let res = get_with_session(
        app.clone(),
        "/api/v1/admin/packages/detail-pkg",
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;

    assert_eq!(body["package"]["name"], "detail-pkg");
    assert!(
        body["versions"].as_array().unwrap().len() >= 2,
        "expected at least 2 versions"
    );
    assert!(
        !body["owners"].as_array().unwrap().is_empty(),
        "expected at least one owner"
    );
    assert!(
        body["open_reports"].as_i64().unwrap_or(0) >= 1,
        "expected open_reports >= 1, got {:?}",
        body["open_reports"]
    );
    // total_downloads should be present (may be 0)
    assert!(
        body.get("total_downloads").is_some(),
        "expected total_downloads field"
    );
}

#[tokio::test]
async fn test_admin_transfer_package_not_found() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "tpnf-admin", "tpnf-admin@test.com").await;

    let res = post_json_with_session(
        app.clone(),
        "/api/v1/admin/packages/nonexistent-xyz/transfer",
        serde_json::json!({"to_username": "someone"}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_admin_transfer_target_user_not_found() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "ttunf-admin", "ttunf-admin@test.com").await;
    let user_session = register_user(app.clone(), "ttunf-user", "ttunf-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "ttunf-token").await;

    publish_package(app.clone(), &token, "ttunf-pkg", "1.0.0", b"data").await;

    let res = post_json_with_session(
        app.clone(),
        "/api/v1/admin/packages/ttunf-pkg/transfer",
        serde_json::json!({"to_username": "ghost-user"}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn test_yank_not_owner() {
    let (app, _state, _dir) = test_app_with_state().await;
    let session1 = register_user(app.clone(), "yno-owner", "yno-owner@test.com").await;
    let token1 = create_api_token(app.clone(), &session1, "yno-tok1").await;

    let session2 = register_user(app.clone(), "yno-intruder", "yno-intruder@test.com").await;
    let token2 = create_api_token(app.clone(), &session2, "yno-tok2").await;

    // Owner publishes
    let res = publish_package(app.clone(), &token1, "yno-pkg", "1.0.0", b"data").await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Intruder tries to yank
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/api/v1/packages/yno-pkg/1.0.0/yank")
                .header("authorization", format!("Bearer {token2}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn test_revoke_other_users_token() {
    let (app, _state, _dir) = test_app_with_state().await;
    let session1 = register_user(app.clone(), "rot-user1", "rot-user1@test.com").await;
    let session2 = register_user(app.clone(), "rot-user2", "rot-user2@test.com").await;

    // user1 creates a token
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/tokens",
        serde_json::json!({"name": "rot-tok1"}),
        &session1,
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);
    let body = body_json(res).await;
    let token_id = body["id"].as_i64().unwrap();
    let token_value = body["token"].as_str().unwrap().to_string();

    // user2 tries to delete user1's token
    let res = delete_with_session(
        app.clone(),
        &format!("/api/v1/tokens/{token_id}"),
        &session2,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "user2 should not be able to delete user1's token"
    );

    // user1's token should still work (publish with it)
    let res = publish_package(app.clone(), &token_value, "rot-pkg", "1.0.0", b"data").await;
    assert_eq!(
        res.status(),
        StatusCode::CREATED,
        "user1's token should still work after user2's failed delete attempt"
    );
}

#[tokio::test]
async fn test_get_package_api_not_found() {
    let (app, _dir) = test_app().await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/packages/nonexistent-xyz-123")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    let body = body_json(res).await;
    assert!(
        body.get("error").is_some(),
        "expected error field in 404 response body"
    );
}

#[tokio::test]
async fn test_pagination_integration() {
    let (app, _state, _dir) = test_app_with_state().await;
    let session = register_user(app.clone(), "pag-user", "pag-user@test.com").await;
    let token = create_api_token(app.clone(), &session, "pag-token").await;

    // Publish 5 packages with unique names
    for i in 1..=5 {
        let name = format!("pag-pkg-{i}");
        let res = publish_package(app.clone(), &token, &name, "1.0.0", b"data").await;
        assert_eq!(res.status(), StatusCode::CREATED);
    }

    // Page 1: expect 2 results
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?per_page=2&page=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let page1 = body["packages"].as_array().unwrap();
    assert_eq!(page1.len(), 2, "page 1 should have 2 packages");
    assert!(
        body["total"].as_i64().unwrap_or(0) >= 5,
        "total should be >= 5"
    );

    // Page 2: expect 2 different results
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?per_page=2&page=2")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let page2 = body["packages"].as_array().unwrap();
    assert_eq!(page2.len(), 2, "page 2 should have 2 packages");
    // Ensure page 1 and page 2 have different packages
    let page1_names: Vec<&str> = page1.iter().map(|p| p["name"].as_str().unwrap()).collect();
    let page2_names: Vec<&str> = page2.iter().map(|p| p["name"].as_str().unwrap()).collect();
    for name in &page2_names {
        assert!(
            !page1_names.contains(name),
            "page 2 package {name} should not appear in page 1"
        );
    }

    // Page 3: expect 1 result (the 5th)
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/search?per_page=2&page=3")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let page3 = body["packages"].as_array().unwrap();
    assert_eq!(page3.len(), 1, "page 3 should have 1 package");
}

#[tokio::test]
async fn test_admin_audit_text_search() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "ats-admin", "ats-admin@test.com").await;
    register_user(
        app.clone(),
        "audit-searchable-victim",
        "ats-victim@test.com",
    )
    .await;

    let victim_id = get_user_id(&state, "audit-searchable-victim").await;

    // Ban the user (generates an audit entry referencing the username)
    let res = post_json_with_session(
        app.clone(),
        &format!("/api/v1/admin/users/{victim_id}/ban"),
        serde_json::json!({"reason": "audit search test"}),
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);

    // Search audit log for the victim's name
    let res = get_with_session(
        app.clone(),
        "/api/v1/admin/audit?q=audit-searchable-victim",
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let entries = body["entries"].as_array().unwrap();
    assert!(
        !entries.is_empty(),
        "expected at least 1 audit entry matching 'audit-searchable-victim'"
    );
    // All returned entries should reference the victim name somewhere
    for entry in entries {
        let entry_str = serde_json::to_string(entry).unwrap();
        assert!(
            entry_str.contains("audit-searchable-victim"),
            "audit entry should reference 'audit-searchable-victim': {entry_str}"
        );
    }
}

#[tokio::test]
async fn test_admin_packages_filter_reported() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "pfr-admin", "pfr-admin@test.com").await;
    let user_session = register_user(app.clone(), "pfr-user", "pfr-user@test.com").await;
    let token = create_api_token(app.clone(), &user_session, "pfr-token").await;

    publish_package(app.clone(), &token, "pfr-reported-pkg", "1.0.0", b"data").await;
    publish_package(app.clone(), &token, "pfr-clean-pkg", "1.0.0", b"data").await;

    // Submit a report against the first package
    let res = post_json_with_session(
        app.clone(),
        "/api/v1/reports",
        serde_json::json!({
            "target_type": "package",
            "target_name": "pfr-reported-pkg",
            "report_type": "malware",
            "reason": "Looks malicious"
        }),
        &user_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::CREATED);

    // Admin filters packages by reported
    let res = get_with_session(
        app.clone(),
        "/api/v1/admin/packages?reported=true",
        &admin_session,
    )
    .await;
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let packages = body["packages"].as_array().unwrap();
    let names: Vec<&str> = packages
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        names.contains(&"pfr-reported-pkg"),
        "reported package should appear in filtered results, got: {names:?}"
    );
}

#[tokio::test]
async fn test_admin_report_not_found() {
    let (app, state, _dir) = test_app_with_state().await;
    let admin_session =
        register_admin(app.clone(), &state, "rnf-admin", "rnf-admin@test.com").await;

    // Dismiss nonexistent report
    let res = post_empty_with_session(
        app.clone(),
        "/api/v1/admin/reports/99999/dismiss",
        &admin_session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "dismiss nonexistent report should be 404"
    );

    // Action nonexistent report
    let res = post_empty_with_session(
        app.clone(),
        "/api/v1/admin/reports/99999/action",
        &admin_session,
    )
    .await;
    assert_eq!(
        res.status(),
        StatusCode::NOT_FOUND,
        "action nonexistent report should be 404"
    );
}
