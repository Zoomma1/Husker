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
async fn create_env_var_empty_key_returns_400() {
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
    assert_eq!(response.status(), 400);
}

#[tokio::test]
async fn create_env_var_invalid_key_returns_400() {
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
    assert_eq!(response.status(), 400);
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

    let response = app.clone().oneshot(request).await.unwrap();

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
