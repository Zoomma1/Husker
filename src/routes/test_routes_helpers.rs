//! Helpers partagés pour les tests d'intégration des routes.
//!
//! `TestApp` centralise le setup répété dans `routes/*/tests.rs` :
//! pool SQLite in-memory + migrations, client Docker, et le router de
//! production réel (`crate::app`). Les helpers de seed (`with_project`,
//! `with_app`) insèrent directement en base — pas via les handlers — pour
//! poser une précondition sans effet de bord Docker (réseau).

use std::str::FromStr;
use std::sync::LazyLock;

use axum::Router;
use bollard::Docker;
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::state::AppState;

/// Sérialise les tests qui mutent les variables d'env de roots (`HUSKER_SOURCES_ROOT` /
/// `HUSKER_DATA_ROOT`), globales au process — sinon races en `cargo test -- --ignored`
/// (multi-thread). Partagé entre les E2E deploy/lifecycle (`deploy.rs`) et la pipeline
/// E2E (`pipeline_e2e.rs`). Mutex tokio : le guard peut être tenu à travers un await.
pub(crate) static ENV_ROOTS_LOCK: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

pub(crate) struct TestApp {
    pub router: Router,
    pub pool: SqlitePool,
    pub docker: Docker,
}

impl TestApp {
    /// Pool in-memory migré + Docker + router de production (`crate::app`).
    /// Aucune ressource seedée.
    pub async fn new() -> Self {
        let options = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(options).await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();

        let docker = Docker::connect_with_local_defaults().unwrap();

        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };
        let router = crate::app(state);

        Self { router, pool, docker }
    }

    /// Insère un project (nom unique). Retourne son id.
    pub async fn with_project(&self) -> i64 {
        let suffix = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        let project_name = format!("project_{}", &suffix[..8]);
        let network_name = format!("husker_project_{}", &suffix[..8]);
        let project = sqlx::query!(
            "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?) RETURNING id",
            project_name,
            network_name,
            created_at
        )
        .fetch_one(&self.pool)
        .await
        .unwrap();
        project.id
    }

    /// Insère un project + une app (defaults : branch `main`, `Dockerfile`,
    /// status `pending`). Retourne `(project_id, app_id)`.
    pub async fn with_app(&self) -> (i64, i64) {
        let project_id = self.with_project().await;
        let suffix = uuid::Uuid::new_v4().to_string();
        let created_at = chrono::Utc::now().to_rfc3339();
        let app_name = format!("app_{}", &suffix[..8]);
        let git_url = format!("https://github.com/user/{}", &suffix[..8]);
        let app = sqlx::query!(
            "INSERT INTO apps (project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status)
             VALUES (?, ?, ?, 'main', 'Dockerfile', NULL, NULL, ?, 0, NULL, 'pending') RETURNING id",
            project_id,
            app_name,
            git_url,
            created_at
        )
        .fetch_one(&self.pool)
        .await
        .unwrap();
        (project_id, app.id)
    }
}
