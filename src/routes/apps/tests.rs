use std::str::FromStr;
use axum::{http, Router};
use axum::routing::{get, post, delete};
use bollard::Docker;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use tower::ServiceExt;
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
async fn test_create_app_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), 201);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let app: App = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(app.name, app_name);
    assert_eq!(app.git_url, expected_git_url);
    assert_eq!(app.git_branch, "main");
    assert_eq!(app.dockerfile_path, "Dockerfile");
    assert_eq!(app.status, "pending");
}

#[tokio::test]
async fn test_create_app_empty_name() {
    let expected_git_url = "https://github.com/user/test";
    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": "", "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn test_create_app_project_not_found() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", 9999999))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_list_apps_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let app_name_2 = format!("app_test_{}_2", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let expected_git_url_2 = format!("https://github.com/user/{}_2", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app).get(list_apps))
        .with_state(state.clone());

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    app.clone().oneshot(request).await.unwrap();

    let body = serde_json::json!({ "name": app_name_2, "git_url": expected_git_url_2 }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project.id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    app.clone().oneshot(request).await.unwrap();

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps", project.id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let apps: Vec<App> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(apps.len(), 2);
    assert_eq!(apps[0].name, app_name);
    assert_eq!(apps[0].git_url, expected_git_url);
    assert_eq!(apps[1].name, app_name_2);
    assert_eq!(apps[1].git_url, expected_git_url_2);
}

#[tokio::test]
async fn test_list_apps_project_not_found() {
    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", get(list_apps))
        .with_state(state.clone());

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps", 9999999))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_list_apps_empty_project() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app).get(list_apps))
        .with_state(state.clone());

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps", project.id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let apps: Vec<App> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(apps.len(), 0);
}

#[tokio::test]
async fn test_get_app_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app))
        .route("/api/projects/{project_id}/apps/{app_id}", get(get_app))
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
    let app_id = serde_json::from_slice::<App>(&bytes).unwrap().id;

    let request =http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}", project.id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let app: App = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(app.name, app_name);
    assert_eq!(app.git_url, expected_git_url);
    assert_eq!(app.git_branch, "main");
    assert_eq!(app.dockerfile_path, "Dockerfile");
    assert_eq!(app.status, "pending");
}

#[tokio::test]
async fn test_get_app_not_found() {
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps/{app_id}", get(get_app))
        .with_state(state.clone());

    let request =http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}", project.id, 999999))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_get_app_project_not_found() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app))
        .route("/api/projects/{project_id}/apps/{app_id}", get(get_app))
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
    let app_id = serde_json::from_slice::<App>(&bytes).unwrap().id;

    let request =http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}", 99999, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_delete_app_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app))
        .route("/api/projects/{project_id}/apps/{app_id}", delete(delete_app))
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
    let app_id = serde_json::from_slice::<App>(&bytes).unwrap().id;

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}", project.id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    let row = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status FROM apps WHERE id = ?",
        app_id
    ).fetch_optional(&state.pool).await.unwrap();
    assert!(row.is_none());
}

#[tokio::test]
async fn test_delete_app_app_not_found() {
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps/{app_id}", delete(delete_app))
        .with_state(state.clone());

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}", project.id, 999999))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_delete_app_project_not_found() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let docker = Docker::connect_with_local_defaults().unwrap();
    let pool = make_test_pool().await;
    let created_at = chrono::Utc::now().to_rfc3339();
    let project = sqlx::query!(
        "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
        "test_project",
        "husker_test_project",
        created_at
    ).fetch_one(&pool).await.unwrap();
    let state = AppState { pool, docker };
    let app = Router::new()
        .route("/api/projects/{project_id}/apps", post(create_app))
        .route("/api/projects/{project_id}/apps/{app_id}", delete(delete_app))
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
    let app_id = serde_json::from_slice::<App>(&bytes).unwrap().id;

    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}/apps/{}", 9999999, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}