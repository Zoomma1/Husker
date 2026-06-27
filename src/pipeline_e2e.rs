//! Test E2E « pipeline complet » d'une app, piloté par l'API HTTP réelle (`crate::app`)
//! contre Docker + git réels : création projet → création app → deploy → stop →
//! suppression du container → nettoyage (delete app + delete project).
//!
//! Complémentaire aux E2E unitaires de `deploy.rs` (qui appellent `deploy`/`stop`/`restart`
//! directement) : ici on traverse le routeur de production de bout en bout.
//!
//! ⚠️ Gap lifecycle exposé par ce test : aucun endpoint ne supprime le container.
//! `delete_app` ne nettoie que la DB (le container reste orphelin) et `delete_project`
//! échouerait sur un network encore peuplé. La suppression du container se fait donc via
//! le client Docker — à transformer en endpoint en M4.
//!
//! Les helpers git/tmp sont volontairement locaux (fichier isolé, hors scope HUSKER-14) ;
//! à factoriser dans un module de test partagé si d'autres E2E HTTP apparaissent.

use axum::body::Body;
use axum::http;
use bollard::query_parameters::RemoveContainerOptionsBuilder;
use git2::{Repository, Signature};
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use tower::ServiceExt;

use crate::routes::test_routes_helpers::{TestApp, ENV_ROOTS_LOCK};

/// Image légère dont le CMD garde le container vivant (pour observer running -> stopped).
const RUNNING_DOCKERFILE: &str = "FROM alpine:3.20\nCMD [\"sleep\", \"3600\"]\n";

struct TmpDir(PathBuf);
impl TmpDir {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("husker-pipeline-e2e-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&p).unwrap();
        TmpDir(p)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TmpDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn set_roots(tmp: &TmpDir) {
    std::env::set_var("HUSKER_SOURCES_ROOT", tmp.path().join("sources"));
    std::env::set_var("HUSKER_DATA_ROOT", tmp.path().join("data"));
}

/// Init un repo git local et y commit un `Dockerfile` ; renvoie (sha, branche par défaut).
fn init_repo_with_dockerfile(dir: &Path, content: &str) -> (String, String) {
    let repo = Repository::init(dir).unwrap();
    fs::write(dir.join("Dockerfile"), content).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("Dockerfile")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = Signature::now("husker-test", "test@husker").unwrap();
    let sha = repo
        .commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
        .unwrap()
        .to_string();
    let branch = repo.head().unwrap().shorthand().unwrap().to_string();
    (sha, branch)
}

/// Envoie une requête sur le routeur de prod et renvoie `(status, corps JSON)`.
/// Corps `None` -> requête vide ; réponse vide -> `Value::Null`.
async fn send(
    ctx: &TestApp,
    method: &str,
    uri: &str,
    body: Option<serde_json::Value>,
) -> (u16, serde_json::Value) {
    let builder = http::Request::builder().method(method).uri(uri);
    let request = match body {
        Some(b) => builder
            .header("Content-Type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    };
    let response = ctx.router.clone().oneshot(request).await.unwrap();
    let status = response.status().as_u16();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let json = if bytes.is_empty() {
        serde_json::Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null)
    };
    (status, json)
}

#[tokio::test]
#[ignore = "Docker+git réels : pipeline complet create->deploy->stop->remove (cargo test -- --ignored)"]
async fn full_lifecycle_create_deploy_stop_remove() {
    let _guard = ENV_ROOTS_LOCK.lock().await;
    let tmp = TmpDir::new();
    set_roots(&tmp);

    // Fixture : repo git local avec un Dockerfile qui garde le container vivant.
    let (_sha, branch) = init_repo_with_dockerfile(&tmp.path().join("repo"), RUNNING_DOCKERFILE);
    let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

    let ctx = TestApp::new().await;
    let docker = ctx.docker.clone();

    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let project_name = format!("pipe{}", &suffix[..12]);
    let app_name = "web";
    let network = format!("husker_{project_name}");
    let container = format!("husker_{project_name}_{app_name}");

    // 1. CRÉATION du projet -> 201 + network Docker créé par le handler.
    let (status, project) = send(&ctx, "POST", "/api/projects", Some(json!({"name": project_name}))).await;
    assert_eq!(status, 201, "création projet");
    let project_id = project["id"].as_i64().expect("project id");
    assert!(
        docker.inspect_network(&network, None).await.is_ok(),
        "le network du projet doit exister après création"
    );

    // 2. CRÉATION de l'app -> 201, status initial `pending`.
    let (status, app) = send(
        &ctx,
        "POST",
        &format!("/api/projects/{project_id}/apps"),
        Some(json!({"name": app_name, "git_url": git_url, "git_branch": branch})),
    )
    .await;
    assert_eq!(status, 201, "création app");
    let app_id = app["id"].as_i64().expect("app id");
    assert_eq!(app["status"], "pending", "app créée en pending");

    // 3. LANCEMENT (deploy git->build->run) -> 200, status `running`, container up.
    let (status, deployed) = send(&ctx, "POST", &format!("/api/apps/{app_id}/deploy"), None).await;
    assert_eq!(status, 200, "deploy");
    assert_eq!(deployed["status"], "running", "status DB -> running");
    let info = docker
        .inspect_container(&container, None)
        .await
        .expect("container créé par le deploy");
    assert_eq!(
        info.state.and_then(|s| s.running),
        Some(true),
        "container running après deploy"
    );

    // 4. STOP -> 200, status `stopped`, container conservé mais arrêté.
    let (status, stopped) = send(&ctx, "POST", &format!("/api/apps/{app_id}/stop"), None).await;
    assert_eq!(status, 200, "stop");
    assert_eq!(stopped["status"], "stopped", "status DB -> stopped");
    let info = docker
        .inspect_container(&container, None)
        .await
        .expect("container conservé après stop");
    assert_eq!(
        info.state.and_then(|s| s.running),
        Some(false),
        "container arrêté (pas supprimé)"
    );

    // 5. SUPPRESSION du container — pas d'endpoint dédié (gap M4) : via le client Docker.
    docker
        .remove_container(
            &container,
            Some(RemoveContainerOptionsBuilder::default().force(true).build()),
        )
        .await
        .expect("suppression du container");
    assert!(
        matches!(
            docker.inspect_container(&container, None).await,
            Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. })
        ),
        "container effectivement supprimé"
    );

    // 6. NETTOYAGE des ressources : delete app (DB) puis delete project (supprime le network).
    let (status, _) = send(&ctx, "DELETE", &format!("/api/projects/{project_id}/apps/{app_id}"), None).await;
    assert_eq!(status, 204, "delete app");
    let (status, _) = send(&ctx, "DELETE", &format!("/api/projects/{project_id}"), None).await;
    assert_eq!(status, 204, "delete project");
    assert!(
        docker.inspect_network(&network, None).await.is_err(),
        "le network doit être supprimé avec le projet"
    );
}
