use axum::http;
use tower::ServiceExt;
use crate::routes::test_routes_helpers::TestApp;
use super::*;

#[tokio::test]
async fn create_env_var_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_var: EnvVar = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_var.key, "DATABASE_URL");
    assert_eq!(env_var.value, expected_database_url);
}

#[tokio::test]
async fn create_env_var_empty_key_returns_422() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    let body = serde_json::json!({ "key": "", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn create_env_var_invalid_key_returns_422() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    let body = serde_json::json!({ "key": "DB-url", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn create_env_var_duplicate_key_returns_409() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let _response = ctx.router.clone().oneshot(request).await.unwrap();

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 409);
}

#[tokio::test]
async fn create_env_var_app_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, "99999999"))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn create_env_var_project_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);

    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", "9999999", "1"))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

// ─────────────── list_env ───────────────

#[tokio::test]
async fn list_env_happy_path_empty() {
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_vars: Vec<EnvVar> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_vars.len(), 0);
}

#[tokio::test]
async fn list_env_happy_path_with_vars() {
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    for (k, v) in [("DATABASE_URL", "postgres://x"), ("PORT", "8080"), ("LOG_LEVEL", "info")] {
        sqlx::query!(
            "INSERT INTO env_vars (app_id, key, value) VALUES (?, ?, ?)",
            app_id, k, v
        ).execute(&ctx.pool).await.unwrap();
    }

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_vars: Vec<EnvVar> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_vars.len(), 3);
    let keys: Vec<&str> = env_vars.iter().map(|e| e.key.as_str()).collect();
    assert!(keys.contains(&"DATABASE_URL"));
    assert!(keys.contains(&"PORT"));
    assert!(keys.contains(&"LOG_LEVEL"));
}

#[tokio::test]
async fn list_env_scoped_to_app() {
    // faux-vert protection : env vars d'une autre app ne fuitent pas
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;
    let (_other_project_id, other_app_id) = ctx.with_app().await;

    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'MINE', 'a')",
        app_id
    ).execute(&ctx.pool).await.unwrap();
    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'OTHER', 'b')",
        other_app_id
    ).execute(&ctx.pool).await.unwrap();

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_vars: Vec<EnvVar> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_vars.len(), 1);
    assert_eq!(env_vars[0].key, "MINE");
}

#[tokio::test]
async fn list_env_app_not_found_returns_404() {
    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/9999999/env", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn list_env_project_not_found_returns_404() {
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects/9999999/apps/1/env")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

// ─────────────── get_env ───────────────

#[tokio::test]
async fn get_env_happy_path() {
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;
    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'DATABASE_URL', 'postgres://x')",
        app_id
    ).execute(&ctx.pool).await.unwrap();

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env/DATABASE_URL", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_var: EnvVar = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_var.key, "DATABASE_URL");
    assert_eq!(env_var.value, "postgres://x");
    assert_eq!(env_var.app_id, app_id);
}

#[tokio::test]
async fn get_env_key_not_found_returns_404() {
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env/UNKNOWN_KEY", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn get_env_scoped_to_app() {
    // faux-vert : la même key existe pour une autre app, ne doit PAS être retournée
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;
    let (_op, other_app_id) = ctx.with_app().await;

    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'SHARED_KEY', 'value_other')",
        other_app_id
    ).execute(&ctx.pool).await.unwrap();

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env/SHARED_KEY", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn get_env_app_not_found_returns_404() {
    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/9999999/env/FOO", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn get_env_project_not_found_returns_404() {
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects/9999999/apps/1/env/FOO")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

// ─────────────── delete_env ───────────────

#[tokio::test]
async fn delete_env_happy_path_returns_204() {
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;
    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'DATABASE_URL', 'postgres://x')",
        app_id
    ).execute(&ctx.pool).await.unwrap();

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}/env/DATABASE_URL", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    // confirme suppression effective
    let remaining = sqlx::query!(
        "SELECT id FROM env_vars WHERE app_id = ? AND key = 'DATABASE_URL'",
        app_id
    ).fetch_optional(&ctx.pool).await.unwrap();
    assert!(remaining.is_none());
}

#[tokio::test]
async fn delete_env_idempotent_unknown_key_returns_204() {
    // décision verrouillée refine 2026-05-29 : DELETE inexistant = 204 (idempotent)
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}/env/UNKNOWN_KEY", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);
}

#[tokio::test]
async fn delete_env_scoped_to_app() {
    // faux-vert : delete sur app A avec key existant sur app B ne supprime PAS l'autre
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;
    let (_op, other_app_id) = ctx.with_app().await;

    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'SHARED', 'a')",
        other_app_id
    ).execute(&ctx.pool).await.unwrap();

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}/env/SHARED", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    let still_there = sqlx::query!(
        "SELECT id FROM env_vars WHERE app_id = ? AND key = 'SHARED'",
        other_app_id
    ).fetch_optional(&ctx.pool).await.unwrap();
    assert!(still_there.is_some(), "env var de l'autre app doit rester");
}

#[tokio::test]
async fn delete_env_app_not_found_returns_404() {
    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/9999999/env/FOO", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn delete_env_project_not_found_returns_404() {
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("DELETE")
        .uri("/api/projects/9999999/apps/1/env/FOO")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}
