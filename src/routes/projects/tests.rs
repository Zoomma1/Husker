use super::*;
use crate::routes::test_routes_helpers::TestApp;
use axum::{body::Body, http};
use tower::ServiceExt;

#[tokio::test]
async fn test_create_project_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let project_name = format!("test_{}", &suffix[..8]);
    let expected_network = format!("husker_{}", project_name);

    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "name": project_name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(project.name, project_name);
    assert_eq!(project.network_name, expected_network);

    let row = sqlx::query!("SELECT name FROM projects WHERE id = ?", project.id)
        .fetch_one(&ctx.pool).await.unwrap();
    assert_eq!(row.name, project_name);

    let networks = ctx.docker.list_networks(None).await.unwrap();
    assert!(networks.iter().any(|n| n.name.as_deref() == Some(&expected_network)));

    ctx.docker.remove_network(&expected_network).await.ok();
}

#[tokio::test]
async fn test_create_project_empty_name() {
    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "name": "" }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn test_create_project_duplicate_name() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let project_name = format!("test_{}", &suffix[..8]);
    let expected_network = format!("husker_{}", project_name);

    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "name": project_name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201);

    let body = serde_json::json!({ "name": project_name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 422);

    ctx.docker.remove_network(&expected_network).await.ok();
}

#[tokio::test]
async fn test_list_projects() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let name1 = format!("test_a_{}", &suffix[..8]);
    let name2 = format!("test_b_{}", &suffix[..8]);

    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "name": name1 }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    ctx.router.clone().oneshot(request).await.unwrap();

    let body = serde_json::json!({ "name": name2 }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    ctx.router.clone().oneshot(request).await.unwrap();

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects")
        .body(Body::empty())
        .unwrap();
    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let projects: Vec<Project> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(projects.len(), 2);
    assert_eq!(projects[0].name, name1);
    assert_eq!(projects[1].name, name2);

    ctx.docker.remove_network(&format!("husker_{}", name1)).await.ok();
    ctx.docker.remove_network(&format!("husker_{}", name2)).await.ok();
}

#[tokio::test]
async fn test_get_project_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let name = format!("test_a_{}", &suffix[..8]);

    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "name": name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();
    let project_id = project.id;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}", project_id))
        .body(Body::empty())
        .unwrap();
    let response = ctx.router.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(project.id, project_id);
    assert_eq!(project.name, name);
    ctx.docker.remove_network(&format!("husker_{}", name)).await.ok();
}

#[tokio::test]
async fn test_get_project_not_found() {
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("GET")
        .uri("/api/projects/99999")
        .body(Body::empty())
        .unwrap();
    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_delete_project_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let name = format!("test_a_{}", &suffix[..8]);

    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "name": name }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri("/api/projects")
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap();
    let response = ctx.router.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let project: Project = serde_json::from_slice(&bytes).unwrap();
    let project_id = project.id;
    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/projects/{}", project_id))
        .body(Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    let row = sqlx::query!("SELECT id FROM projects WHERE id = ?", project_id)
        .fetch_optional(&ctx.pool).await.unwrap();
    assert!(row.is_none());

    let networks = ctx.docker.list_networks(None).await.unwrap();
    assert!(!networks.iter().any(|n| n.name.as_deref() == Some(&format!("husker_{}", name))));
}

#[tokio::test]
async fn test_delete_project_not_found() {
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("DELETE")
        .uri("/api/projects/99999")
        .body(Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}
