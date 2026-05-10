use super::*;
use crate::state::AppState;
use axum::{body::Body, http, Router, routing::{post, get, delete}};
use bollard::Docker;
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use std::str::FromStr;
use tower::ServiceExt;

async fn make_test_pool() -> SqlitePool {
    let pool = SqliteConnectOptions::from_str("sqlite::memory:")
        .unwrap()
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(pool).await.unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    pool
}

#[tokio::test]
async fn test_create_project_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let project_name = format!("test_{}", &suffix[..8]);
    let expected_network = format!("husker_{}", project_name);

    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool: pool.clone(), docker: docker.clone() };

    let app = Router::new()
        .route("/api/projects", post(create_project))
        .with_state(state);

    let body = serde_json::json!({ "name": project_name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(project.name, project_name);
    assert_eq!(project.network_name, expected_network);

    let row = sqlx::query!("SELECT name FROM projects WHERE id = ?", project.id)
        .fetch_one(&pool).await.unwrap();
    assert_eq!(row.name, project_name);

    let networks = docker.list_networks(None).await.unwrap();
    assert!(networks.iter().any(|n| n.name.as_deref() == Some(&expected_network)));

    docker.remove_network(&expected_network).await.ok();
}

#[tokio::test]
async fn test_create_project_empty_name() {
    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool: pool.clone(), docker: docker.clone() };

    let app = Router::new()
        .route("/api/projects", post(create_project))
        .with_state(state);

    let body = serde_json::json!({ "name": "" }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn test_create_project_duplicate_name() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let project_name = format!("test_{}", &suffix[..8]);
    let expected_network = format!("husker_{}", project_name);

    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool: pool.clone(), docker: docker.clone() };

    let app = Router::new()
        .route("/api/projects", post(create_project))
        .with_state(state);

    let body = serde_json::json!({ "name": project_name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201);

    let body = serde_json::json!({ "name": project_name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);

    docker.remove_network(&expected_network).await.ok();
}

#[tokio::test]
async fn test_list_projects() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let name1 = format!("test_a_{}", &suffix[..8]);
    let name2 = format!("test_b_{}", &suffix[..8]);

    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool: pool.clone(), docker: docker.clone() };

    let app = Router::new()
        .route("/api/projects", post(create_project).get(list_projects))
        .with_state(state);

    let body = serde_json::json!({ "name": name1 }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(request).await.unwrap();

    let body = serde_json::json!({ "name": name2 }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    app.clone().oneshot(request).await.unwrap();

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let projects: Vec<Project> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].name, name1);
    assert_eq!(projects[1].name, name2);

    docker.remove_network(&format!("husker_{}", name1)).await.ok();
    docker.remove_network(&format!("husker_{}", name2)).await.ok();
}

#[tokio::test]
async fn test_get_project_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let name = format!("test_a_{}", &suffix[..8]);

    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool: pool.clone(), docker: docker.clone() };

    let app = Router::new()
        .route("/api/projects", post(create_project))
        .route("/api/projects/{id}", get(get_project))
        .with_state(state);

    let body = serde_json::json!({ "name": name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();
    let project_id = project.id;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}", project_id))
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();

    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(project.id, project_id);
    assert_eq!(project.name, name);
    docker.remove_network(&format!("husker_{}", name)).await.ok();
}

#[tokio::test]
async fn test_get_project_not_found() {
    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool, docker };

    let app = Router::new()
        .route("/api/projects/{id}", get(get_project))
        .with_state(state);

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects/99999")
        .body(Body::empty())
        .unwrap();
    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_delete_project_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let name = format!("test_a_{}", &suffix[..8]);
    let pool = make_test_pool().await;

    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool: pool.clone(), docker: docker.clone() };

    let app = Router::new()
        .route("/api/projects", post(create_project))
        .route("/api/projects/{id}", delete(delete_project))
        .with_state(state);

    let body = serde_json::json!({ "name": name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();
    let project_id = project.id;
    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}", project_id))
        .body(Body::empty())
        .unwrap();

    let response = app.oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    let row = sqlx::query!("SELECT id FROM projects WHERE id = ?", project_id)
        .fetch_optional(&pool).await.unwrap();
    assert!(row.is_none());

    let networks = docker.list_networks(None).await.unwrap();
    assert!(!networks.iter().any(|n| n.name.as_deref() == Some(&format!("husker_{}", name))));
}

#[tokio::test]
async fn test_delete_project_not_found() {
    let pool = make_test_pool().await;
    let docker = Docker::connect_with_local_defaults().unwrap();
    let state = AppState { pool: pool.clone(), docker: docker.clone() };
    let app = Router::new()
        .route("/api/projects/{id}", delete(delete_project))
        .with_state(state);

    let request = http::Request::builder()
        .method("DELETE")
        .uri("/api/projects/99999")
        .body(Body::empty())
        .unwrap();

    let response = app.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}
