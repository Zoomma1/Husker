use std::str::FromStr;
use axum::{http, Router};
use axum::routing::{get, post, delete};
use bollard::Docker;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use tower::ServiceExt;
use crate::routes::apps::{create_app, App};
use crate::state::AppState;
use super::*;

async fn seed_project_and_app(pool: &SqlitePool, suffix: &str) -> (i64, i64) {
    let created_at = chrono::Utc::now().to_rfc3339();
    let project_name = format!("project_{}", &suffix[..8]);
    let network_name = format!("husker_project_{}", &suffix[..8]);
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        project_name,
        network_name,
        created_at
    ).fetch_one(pool).await.unwrap();
    let app_name = format!("app_{}", &suffix[..8]);
    let git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let app = sqlx::query!(
        "INSERT INTO apps (project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status)
         VALUES (?, ?, ?, 'main', 'Dockerfile', NULL, NULL, ?, 0, NULL, 'pending') RETURNING id",
        project.id,
        app_name,
        git_url,
        created_at
    ).fetch_one(pool).await.unwrap();
    (project.id, app.id)
}

async fn make_test_pool() -> SqlitePool {
    let pool = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(pool).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn create_env_var_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = crate::routes::env_vars::tests::make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps", post(create_app))
        .route("/api/projects/{pid}/apps/{aid}/env", post(create_env))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let _app: App = serde_json::from_slice(&bytes).unwrap();
    let app_id = _app.id;

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project.id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_var: EnvVar = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_var.key, "DATABASE_URL");
    assert_eq!(env_var.value, expected_database_url);
}

#[tokio::test]
async fn create_env_var_empty_key_returns_422() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = crate::routes::env_vars::tests::make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps", post(create_app))
        .route("/api/projects/{pid}/apps/{aid}/env", post(create_env))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let _app: App = serde_json::from_slice(&bytes).unwrap();
    let app_id = _app.id;

    let body = serde_json::json!({ "key": "", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project.id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn create_env_var_invalid_key_returns_422() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = crate::routes::env_vars::tests::make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps", post(create_app))
        .route("/api/projects/{pid}/apps/{aid}/env", post(create_env))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let _app: App = serde_json::from_slice(&bytes).unwrap();
    let app_id = _app.id;

    let body = serde_json::json!({ "key": "DB-url", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project.id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn create_env_var_duplicate_key_returns_409() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = crate::routes::env_vars::tests::make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps", post(create_app))
        .route("/api/projects/{pid}/apps/{aid}/env", post(create_env))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let _app: App = serde_json::from_slice(&bytes).unwrap();
    let app_id = _app.id;

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project.id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let _response = app.clone().oneshot(request).await.unwrap();

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project.id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 409);
}

#[tokio::test]
async fn create_env_var_app_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = crate::routes::env_vars::tests::make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env", post(create_env))
        .with_state(state.clone());

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project.id, "99999999"))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn create_env_var_project_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let expected_database_url = format!("postgres://user:password@localhost/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = crate::routes::env_vars::tests::make_test_pool().await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env", post(create_env))
        .with_state(state.clone());

    let body = serde_json::json!({ "key": "DATABASE_URL", "value": expected_database_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", "9999999", "1"))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

// ─────────────── list_env ───────────────

#[tokio::test]
async fn list_env_happy_path_empty() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env", get(list_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_vars: Vec<EnvVar> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_vars.len(), 0);
}

#[tokio::test]
async fn list_env_happy_path_with_vars() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;

    for (k, v) in [("DATABASE_URL", "postgres://x"), ("PORT", "8080"), ("LOG_LEVEL", "info")] {
        sqlx::query!(
            "INSERT INTO env_vars (app_id, key, value) VALUES (?, ?, ?)",
            app_id, k, v
        ).execute(&pool).await.unwrap();
    }

    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env", get(list_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
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
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;
    let other_suffix = uuid::Uuid::new_v4().to_string();
    let (_other_project_id, other_app_id) = seed_project_and_app(&pool, &other_suffix).await;

    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'MINE', 'a')",
        app_id
    ).execute(&pool).await.unwrap();
    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'OTHER', 'b')",
        other_app_id
    ).execute(&pool).await.unwrap();

    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env", get(list_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_vars: Vec<EnvVar> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_vars.len(), 1);
    assert_eq!(env_vars[0].key, "MINE");
}

#[tokio::test]
async fn list_env_app_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, _app_id) = seed_project_and_app(&pool, &suffix).await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env", get(list_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/9999999/env", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn list_env_project_not_found_returns_404() {
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env", get(list_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects/9999999/apps/1/env")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

// ─────────────── get_env ───────────────

#[tokio::test]
async fn get_env_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;
    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'DATABASE_URL', 'postgres://x')",
        app_id
    ).execute(&pool).await.unwrap();

    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", get(get_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env/DATABASE_URL", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let env_var: EnvVar = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(env_var.key, "DATABASE_URL");
    assert_eq!(env_var.value, "postgres://x");
    assert_eq!(env_var.app_id, app_id);
}

#[tokio::test]
async fn get_env_key_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;

    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", get(get_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env/UNKNOWN_KEY", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn get_env_scoped_to_app() {
    // faux-vert : la même key existe pour une autre app, ne doit PAS être retournée
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;
    let other_suffix = uuid::Uuid::new_v4().to_string();
    let (_op, other_app_id) = seed_project_and_app(&pool, &other_suffix).await;

    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'SHARED_KEY', 'value_other')",
        other_app_id
    ).execute(&pool).await.unwrap();

    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", get(get_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}/env/SHARED_KEY", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn get_env_app_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, _app_id) = seed_project_and_app(&pool, &suffix).await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", get(get_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/9999999/env/FOO", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn get_env_project_not_found_returns_404() {
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", get(get_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects/9999999/apps/1/env/FOO")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

// ─────────────── delete_env ───────────────

#[tokio::test]
async fn delete_env_happy_path_returns_204() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;
    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'DATABASE_URL', 'postgres://x')",
        app_id
    ).execute(&pool).await.unwrap();

    let state = AppState { pool: pool.clone(), docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", delete(delete_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}/env/DATABASE_URL", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    // confirme suppression effective
    let remaining = sqlx::query!(
        "SELECT id FROM env_vars WHERE app_id = ? AND key = 'DATABASE_URL'",
        app_id
    ).fetch_optional(&pool).await.unwrap();
    assert!(remaining.is_none());
}

#[tokio::test]
async fn delete_env_idempotent_unknown_key_returns_204() {
    // décision verrouillée refine 2026-05-29 : DELETE inexistant = 204 (idempotent)
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;

    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", delete(delete_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}/env/UNKNOWN_KEY", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);
}

#[tokio::test]
async fn delete_env_scoped_to_app() {
    // faux-vert : delete sur app A avec key existant sur app B ne supprime PAS l'autre
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, app_id) = seed_project_and_app(&pool, &suffix).await;
    let other_suffix = uuid::Uuid::new_v4().to_string();
    let (_op, other_app_id) = seed_project_and_app(&pool, &other_suffix).await;

    sqlx::query!(
        "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'SHARED', 'a')",
        other_app_id
    ).execute(&pool).await.unwrap();

    let state = AppState { pool: pool.clone(), docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", delete(delete_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}/env/SHARED", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    let still_there = sqlx::query!(
        "SELECT id FROM env_vars WHERE app_id = ? AND key = 'SHARED'",
        other_app_id
    ).fetch_optional(&pool).await.unwrap();
    assert!(still_there.is_some(), "env var de l'autre app doit rester");
}

#[tokio::test]
async fn delete_env_app_not_found_returns_404() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let (project_id, _app_id) = seed_project_and_app(&pool, &suffix).await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", delete(delete_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/9999999/env/FOO", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn delete_env_project_not_found_returns_404() {
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{pid}/apps/{aid}/env/{key}", delete(delete_env))
        .with_state(state);

    let request = http::Request::builder()
        .method("DELETE")
        .uri("/api/projects/9999999/apps/1/env/FOO")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}
