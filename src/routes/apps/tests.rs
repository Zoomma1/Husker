use super::*;
use crate::routes::test_routes_helpers::TestApp;
use axum::http;
use tower::ServiceExt;

#[tokio::test]
async fn test_create_app_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), 201);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
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

    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let body = serde_json::json!({ "name": "", "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), 422);
}

#[tokio::test]
async fn test_create_app_project_not_found() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);

    let ctx = TestApp::new().await;

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", 9999999))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_list_apps_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let app_name_2 = format!("app_test_{}_2", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);
    let expected_git_url_2 = format!("https://github.com/user/{}_2", &suffix[..8]);

    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    ctx.router.clone().oneshot(request).await.unwrap();

    let body = serde_json::json!({ "name": app_name_2, "git_url": expected_git_url_2 }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    ctx.router.clone().oneshot(request).await.unwrap();

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let apps: Vec<App> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(apps.len(), 2);
    assert_eq!(apps[0].name, app_name);
    assert_eq!(apps[0].git_url, expected_git_url);
    assert_eq!(apps[1].name, app_name_2);
    assert_eq!(apps[1].git_url, expected_git_url_2);
}

#[tokio::test]
async fn test_list_apps_project_not_found() {
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps", 9999999))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_list_apps_empty_project() {
    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps", project_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let apps: Vec<App> = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(apps.len(), 0);
}

#[tokio::test]
async fn test_get_app_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let app_id = serde_json::from_slice::<App>(&bytes).unwrap().id;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}", project_id, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 200);

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let app: App = serde_json::from_slice(&bytes).unwrap();

    assert_eq!(app.name, app_name);
    assert_eq!(app.git_url, expected_git_url);
    assert_eq!(app.git_branch, "main");
    assert_eq!(app.dockerfile_path, "Dockerfile");
    assert_eq!(app.status, "pending");
}

#[tokio::test]
async fn test_get_app_not_found() {
    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}", project_id, 999999))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_get_app_project_not_found() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();

    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let app_id = serde_json::from_slice::<App>(&bytes).unwrap().id;

    let request = http::Request::builder()
        .method("GET")
        .uri(format!("/api/projects/{}/apps/{}", 99999, app_id))
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_delete_app_happy_path() {
    let suffix = uuid::Uuid::new_v4().to_string();
    let app_name = format!("app_test_{}", &suffix[..8]);
    let expected_git_url = format!("https://github.com/user/{}", &suffix[..8]);

    let ctx = TestApp::new().await;
    let project_id = ctx.with_project().await;

    let body = serde_json::json!({ "name": app_name, "git_url": expected_git_url }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps", project_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let app_id = serde_json::from_slice::<App>(&bytes).unwrap().id;

    // Route flat HUSKER-17. App jamais déployée -> pas de container : `destroy_app` fait un
    // remove_container qui 404 (idempotent) puis supprime la ligne DB. 204 attendu.
    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/apps/{}", app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    let row = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status FROM apps WHERE id = ?",
        app_id
    ).fetch_optional(&ctx.pool).await.unwrap();
    assert!(row.is_none());
}

#[tokio::test]
async fn test_delete_app_not_found() {
    // Route flat : id inconnu -> 404 avant tout appel Docker (NotFound au load DB).
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("DELETE")
        .uri("/api/apps/9999999")
        .header("Content-Type", "application/json")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_deploy_app_not_found() {
    // Mapping HTTP de la route flat : app inconnue -> 404 (deploy() -> NotFound -> into_response).
    // Pas d'I/O git/docker (l'orchestrateur retourne avant).
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("POST")
        .uri("/api/apps/9999999/deploy")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_stop_app_not_found() {
    // App inconnue -> 404 avant tout appel Docker (lifecycle retourne NotFound au load DB).
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("POST")
        .uri("/api/apps/9999999/stop")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_restart_app_not_found() {
    // Idem stop : app inconnue -> 404 avant tout appel Docker.
    let ctx = TestApp::new().await;

    let request = http::Request::builder()
        .method("POST")
        .uri("/api/apps/9999999/restart")
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 404);
}

#[tokio::test]
async fn test_delete_app_cleans_env_vars() {
    // destroy_app supprime les env vars de l'app (pas d'ON DELETE CASCADE, PRAGMA foreign_keys
    // OFF -> sans ce cleanup elles deviendraient orphelines).
    let ctx = TestApp::new().await;
    let (project_id, app_id) = ctx.with_app().await;

    // Seed une env var via l'API (handler, aucun appel Docker).
    let body = serde_json::json!({ "key": "TOKEN", "value": "secret" }).to_string();
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/api/projects/{}/apps/{}/env", project_id, app_id))
        .header("Content-Type", "application/json")
        .body(axum::body::Body::from(body))
        .unwrap();
    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 201, "env var créée");

    // Destroy l'app (jamais déployée -> remove_container 404-fast).
    let request = http::Request::builder()
        .method("DELETE")
        .uri(format!("/api/apps/{}", app_id))
        .body(axum::body::Body::empty())
        .unwrap();
    let response = ctx.router.clone().oneshot(request).await.unwrap();
    assert_eq!(response.status(), 204);

    let rows = sqlx::query!("SELECT key, value FROM env_vars WHERE app_id = ?", app_id)
        .fetch_all(&ctx.pool)
        .await
        .unwrap();
    assert!(rows.is_empty(), "les env vars sont supprimées avec l'app");
}

/// Seed direct SQL d'un déploiement (pas de write path HUSKER-21 dans ce module). Retourne
/// son id.
async fn seed_deployment(
    pool: &sqlx::SqlitePool,
    app_id: i64,
    status: &str,
    git_sha: Option<&str>,
    started_at: &str,
    finished_at: Option<&str>,
) -> i64 {
    let row = sqlx::query!(
        "INSERT INTO deployments (app_id, git_sha, status, started_at, finished_at, log_path)
         VALUES (?, ?, ?, ?, ?, NULL) RETURNING id",
        app_id,
        git_sha,
        status,
        started_at,
        finished_at
    )
    .fetch_one(pool)
    .await
    .unwrap();
    row.id
}

async fn seed_signal(pool: &sqlx::SqlitePool, deployment_id: i64, kind: &str, message: &str) {
    let created_at = chrono::Utc::now().to_rfc3339();
    sqlx::query!(
        "INSERT INTO deployment_signals (deployment_id, kind, message, created_at) VALUES (?, ?, ?, ?)",
        deployment_id,
        kind,
        message,
        created_at
    )
    .execute(pool)
    .await
    .unwrap();
}

/// Boilerplate partagé des tests `list_deployments` : requête, statut, bytes bruts. Ne
/// désérialise pas en `DeploymentsPage` — certains tests (404) n'ont pas ce corps.
async fn get_deployments_raw(
    ctx: &TestApp,
    app_id: i64,
    query: &str,
) -> (http::StatusCode, axum::body::Bytes) {
    let uri = if query.is_empty() {
        format!("/api/apps/{}/deployments", app_id)
    } else {
        format!("/api/apps/{}/deployments?{}", app_id, query)
    };
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .body(axum::body::Body::empty())
        .unwrap();

    let response = ctx.router.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, bytes)
}

/// Idem `get_deployments_raw`, désérialisé en `DeploymentsPage` — pour les tests 200.
async fn get_deployments(ctx: &TestApp, app_id: i64, query: &str) -> DeploymentsPage {
    let (status, bytes) = get_deployments_raw(ctx, app_id, query).await;
    assert_eq!(status, 200, "attendu 200, body: {:?}", bytes);
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_list_deployments_app_not_found() {
    let ctx = TestApp::new().await;

    let (status, _bytes) = get_deployments_raw(&ctx, 9999999, "").await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn test_list_deployments_empty() {
    let ctx = TestApp::new().await;
    let (_project_id, app_id) = ctx.with_app().await;

    let page = get_deployments(&ctx, app_id, "").await;

    assert!(page.data.is_empty());
    assert_eq!(page.total, 0);
    assert_eq!(page.limit, 20);
    assert_eq!(page.offset, 0);
}

#[tokio::test]
async fn test_list_deployments_ordered_most_recent_first() {
    let ctx = TestApp::new().await;
    let (_project_id, app_id) = ctx.with_app().await;

    let older = seed_deployment(
        &ctx.pool,
        app_id,
        "success",
        Some("sha_old"),
        "2026-09-20T10:00:00Z",
        Some("2026-09-20T10:05:00Z"),
    )
    .await;
    let newer = seed_deployment(
        &ctx.pool,
        app_id,
        "failed",
        Some("sha_new"),
        "2026-09-21T10:00:00Z",
        Some("2026-09-21T10:05:00Z"),
    )
    .await;

    let page = get_deployments(&ctx, app_id, "").await;

    assert_eq!(page.total, 2);
    assert_eq!(page.data.len(), 2);
    assert_eq!(page.data[0].id, newer);
    assert_eq!(page.data[0].status, "failed");
    assert_eq!(page.data[1].id, older);
    assert_eq!(page.data[1].status, "success");
}

#[tokio::test]
async fn test_list_deployments_tie_break_on_same_started_at() {
    let ctx = TestApp::new().await;
    let (_project_id, app_id) = ctx.with_app().await;

    let first = seed_deployment(
        &ctx.pool,
        app_id,
        "success",
        Some("sha_a"),
        "2026-09-21T10:00:00Z",
        Some("2026-09-21T10:05:00Z"),
    )
    .await;
    let second = seed_deployment(
        &ctx.pool,
        app_id,
        "success",
        Some("sha_b"),
        "2026-09-21T10:00:00Z",
        Some("2026-09-21T10:05:00Z"),
    )
    .await;

    let page = get_deployments(&ctx, app_id, "").await;

    assert_eq!(page.data[0].id, second.max(first));
    assert_eq!(page.data[1].id, second.min(first));
}

#[tokio::test]
async fn test_list_deployments_offset_beyond_total() {
    let ctx = TestApp::new().await;
    let (_project_id, app_id) = ctx.with_app().await;

    seed_deployment(
        &ctx.pool,
        app_id,
        "success",
        Some("sha"),
        "2026-09-21T10:00:00Z",
        Some("2026-09-21T10:05:00Z"),
    )
    .await;

    let page = get_deployments(&ctx, app_id, "offset=50").await;

    assert!(page.data.is_empty());
    assert_eq!(page.total, 1);
    assert_eq!(page.offset, 50);
}

#[tokio::test]
async fn test_list_deployments_with_and_without_signals() {
    let ctx = TestApp::new().await;
    let (_project_id, app_id) = ctx.with_app().await;

    let with_signal = seed_deployment(
        &ctx.pool,
        app_id,
        "success",
        Some("sha_with_signal"),
        "2026-09-21T10:00:00Z",
        Some("2026-09-21T10:05:00Z"),
    )
    .await;
    seed_signal(&ctx.pool, with_signal, "policy", "base image non pinné").await;

    let without_signal = seed_deployment(
        &ctx.pool,
        app_id,
        "success",
        Some("sha_no_signal"),
        "2026-09-20T10:00:00Z",
        Some("2026-09-20T10:05:00Z"),
    )
    .await;

    let page = get_deployments(&ctx, app_id, "").await;

    let entry_with_signal = page.data.iter().find(|d| d.id == with_signal).unwrap();
    assert_eq!(entry_with_signal.signals.len(), 1);
    assert_eq!(entry_with_signal.signals[0].kind, "policy");
    assert_eq!(entry_with_signal.signals[0].message, "base image non pinné");

    let entry_without_signal = page.data.iter().find(|d| d.id == without_signal).unwrap();
    assert!(entry_without_signal.signals.is_empty());
}

#[tokio::test]
async fn test_list_deployments_malformed_query_param_returns_json_error() {
    // Cohérence de l'enveloppe d'erreur : ValidatedQuery mappe le rejet vers AppError
    // au lieu du texte brut renvoyé par défaut par axum::extract::Query.
    let ctx = TestApp::new().await;
    let (_project_id, app_id) = ctx.with_app().await;

    let (status, bytes) = get_deployments_raw(&ctx, app_id, "limit=abc").await;

    assert_eq!(status, 400);
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(body.get("error").is_some(), "body: {:?}", body);
}

#[test]
fn is_safe_path_segment_rejects_traversal() {
    // Garde anti-échappement du data root dans destroy_app (#1 code-review HUSKER-17).
    assert!(is_safe_path_segment("web"));
    assert!(is_safe_path_segment("my-app_2"));
    assert!(!is_safe_path_segment(""));
    assert!(!is_safe_path_segment("."));
    assert!(!is_safe_path_segment(".."));
    assert!(!is_safe_path_segment("../evil"));
    assert!(!is_safe_path_segment("a/b"));
    assert!(!is_safe_path_segment("a\\b"));
    assert!(!is_safe_path_segment("/etc/passwd"));
}
