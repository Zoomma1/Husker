use crate::errors::AppError;
use crate::extractors::{non_blank, ValidatedJson, ValidatedQuery};
use crate::routes::projects::Project;
use crate::state::AppState;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use bollard::query_parameters::RemoveContainerOptionsBuilder;
use serde::{Deserialize, Serialize};
use validator::Validate;

#[derive(Deserialize, Validate)]
pub struct CreateAppRequest {
    #[validate(custom(function = "non_blank"))]
    pub name: String,
    pub git_url: String,
    #[serde(default = "default_git_branch")]
    pub git_branch: String,
    #[serde(default = "default_dockerfile_path")]
    pub dockerfile_path: String,
    pub build_command: Option<String>,
    pub run_command: Option<String>,
}

#[derive(Deserialize, Serialize, sqlx::FromRow)]
pub struct App {
    pub id: i64,
    pub project_id: i64,
    pub name: String,
    pub git_url: String,
    pub git_branch: String,
    pub dockerfile_path: String,
    pub build_command: Option<String>,
    pub run_command: Option<String>,
    pub created_at: String,
    pub exposed: bool,
    pub public_domain: Option<String>,
    pub status: String,
}

fn default_git_branch() -> String {
    "main".to_string()
}

fn default_dockerfile_path() -> String {
    "Dockerfile".to_string()
}

pub async fn create_app(
    Path(project_id): Path<i64>,
    State(state): State<AppState>,
    ValidatedJson(payload): ValidatedJson<CreateAppRequest>,
) -> Result<(StatusCode, Json<App>), AppError> {
    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        project_id
    )
    .fetch_optional(&state.pool)
    .await?;

    if project.is_none() {
        return Err(AppError::NotFound);
    }

    let created_at = chrono::Utc::now().to_rfc3339();
    sqlx::query!(
        "INSERT INTO apps (project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status)
        VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        project_id,
        payload.name,
        payload.git_url,
        payload.git_branch,
        payload.dockerfile_path,
        payload.build_command,
        payload.run_command,
        created_at,
        false,
        None::<String>,
        "pending"
    ).execute(&state.pool).await?;

    let app = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE project_id = ? AND name = ?",
        project_id,
        payload.name
    ).fetch_one(&state.pool).await?;

    Ok((StatusCode::CREATED, Json(app)))
}

pub async fn list_apps(
    Path(project_id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Json<Vec<App>>, AppError> {
    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        project_id
    )
    .fetch_optional(&state.pool)
    .await?;

    if project.is_none() {
        return Err(AppError::NotFound);
    }

    let apps = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE project_id = ?",
        project_id,
    ).fetch_all(&state.pool).await?;

    Ok(Json(apps))
}

pub async fn get_app(
    Path((project_id, app_id)): Path<(i64, i64)>,
    State(state): State<AppState>,
) -> Result<Json<App>, AppError> {
    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        project_id
    )
    .fetch_optional(&state.pool)
    .await?;

    if project.is_none() {
        return Err(AppError::NotFound);
    }

    let app = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE id = ? AND project_id = ?",
        app_id,
        project_id
    ).fetch_optional(&state.pool).await?;

    match app {
        Some(a) => Ok(Json(a)),
        None => Err(AppError::NotFound),
    }
}

/// `DELETE /api/apps/{id}` — route flat (cohérente avec deploy/stop/restart, HUSKER-17).
/// Destruction complète de l'app : stop + remove du container, suppression du volume `data/`,
/// puis suppression de la ligne DB. 404 si l'app n'existe pas ; 204 sinon.
pub async fn delete_app(
    Path(app_id): Path<i64>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    let app = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE id = ?",
        app_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        app.project_id
    )
    .fetch_one(&state.pool)
    .await?;

    destroy_app(&state, &app, &project).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Primitive de destruction d'une app, partagée par le handler flat `delete_app` et la
/// cascade `delete_project`. Ordre : stop + remove du container (force) -> suppression du
/// bind mount `data/` -> suppression de la ligne DB.
///
/// **Idempotent** : container absent (404 Docker) ou dossier `data/` absent -> traités comme
/// des succès. Une app jamais déployée est donc supprimée sans erreur (DB uniquement).
pub async fn destroy_app(state: &AppState, app: &App, project: &Project) -> Result<(), AppError> {
    // Garde défensive : `project.name` et `app.name` composent le chemin fs supprimé par
    // `remove_dir_all` plus bas. Les noms ne sont pas encore restreints à la création
    // (validation `non_blank` seule) ; un nom contenant un séparateur, `..` ou absolu
    // échapperait le data root et détruirait un dossier hors zone. On refuse AVANT tout
    // effet de bord. (Validation des noms à la source = HUSKER-19.)
    if !is_safe_path_segment(&project.name) || !is_safe_path_segment(&app.name) {
        return Err(AppError::Deploy(format!(
            "nom projet/app non sûr pour la suppression du volume: {}/{}",
            project.name, app.name
        )));
    }

    // 1. stop + remove du container (force = kill puis rm). 404 = déjà absent -> idempotent.
    let name = crate::deploy::run::container_name(&project.name, &app.name);
    let opts = RemoveContainerOptionsBuilder::default().force(true).build();
    match state.docker.remove_container(&name, Some(opts)).await {
        Ok(_) => {}
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(e) => return Err(AppError::Docker(e)),
    }

    // 2. suppression du bind mount `data/` de l'app. Absent -> idempotent.
    let data_root = std::env::var("HUSKER_DATA_ROOT").unwrap_or_else(|_| "data".to_string());
    let dir = crate::deploy::run::data_dir(&data_root, &project.name, &app.name);
    match std::fs::remove_dir_all(&dir) {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(AppError::Deploy(format!("remove data dir: {e}"))),
    }

    // 3. suppression DB, enfants avant parent : pas d'`ON DELETE CASCADE` dans le schéma, et
    //    sqlx active `PRAGMA foreign_keys` par défaut -> un enfant oublié fait échouer le
    //    DELETE du parent (constraint failed).
    sqlx::query!("DELETE FROM env_vars WHERE app_id = ?", app.id)
        .execute(&state.pool)
        .await?;
    // Idem pour l'historique des digests (HUSKER-15) : SQLite réattribue les rowid libérés,
    // une ligne orpheline serait héritée par une future app et déclencherait une fausse
    // alerte de dérive.
    sqlx::query!("DELETE FROM base_image_digests WHERE app_id = ?", app.id)
        .execute(&state.pool)
        .await?;
    // Historique de déploiement (HUSKER-21) : les signaux référencent `deployments`, qui
    // référence `apps` -> petits-enfants d'abord.
    sqlx::query!(
        "DELETE FROM deployment_signals
         WHERE deployment_id IN (SELECT id FROM deployments WHERE app_id = ?)",
        app.id
    )
    .execute(&state.pool)
    .await?;
    sqlx::query!("DELETE FROM deployments WHERE app_id = ?", app.id)
        .execute(&state.pool)
        .await?;
    sqlx::query!("DELETE FROM apps WHERE id = ?", app.id)
        .execute(&state.pool)
        .await?;

    Ok(())
}

/// Un nom de projet/app doit être un segment de chemin unique. Sinon il pourrait échapper le
/// data root lors du `remove_dir_all` de `destroy_app` (séparateur, `..`, chemin absolu).
fn is_safe_path_segment(s: &str) -> bool {
    !s.is_empty() && s != "." && s != ".." && !s.contains('/') && !s.contains('\\')
}

/// `POST /api/apps/{id}/deploy` — route flat (dérogation au nested CRUD, cf. refine).
/// Orchestration synchrone `git → build → run` ; 404 si l'app n'existe pas,
/// 502 sur échec git/docker (via `AppError`). Renvoie l'app à jour (status `running`).
pub async fn deploy_app(
    Path(app_id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Json<App>, AppError> {
    let app = crate::deploy::deploy(&state, app_id).await?;
    Ok(Json(app))
}

/// `POST /api/apps/{id}/stop` — arrête le container, `status = stopped` (HUSKER-14).
/// 404 si app/container absent ; 304 si déjà arrêté ; 502 sur erreur Docker.
pub async fn stop_app(
    Path(app_id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Response, AppError> {
    match crate::deploy::stop(&state, app_id).await? {
        crate::deploy::StopOutcome::Stopped(app) => Ok(Json(*app).into_response()),
        crate::deploy::StopOutcome::AlreadyStopped => Ok(StatusCode::NOT_MODIFIED.into_response()),
    }
}

/// `POST /api/apps/{id}/restart` — relance le container, `status = running` (HUSKER-14).
/// 404 si app/container absent ; 502 sur erreur Docker.
pub async fn restart_app(
    Path(app_id): Path<i64>,
    State(state): State<AppState>,
) -> Result<Json<App>, AppError> {
    Ok(Json(crate::deploy::restart(&state, app_id).await?))
}

#[derive(Deserialize)]
pub struct PaginationParams {
    pub limit: Option<u32>,
    pub offset: Option<u32>,
}

#[derive(Serialize, Deserialize)]
pub struct DeploymentSignal {
    pub id: i64,
    pub kind: String,
    pub message: String,
    pub created_at: String,
}

#[derive(Serialize, Deserialize)]
pub struct DeploymentEntry {
    pub id: i64,
    pub status: String,
    pub git_sha: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub log_path: Option<String>,
    pub signals: Vec<DeploymentSignal>,
}

#[derive(Serialize, Deserialize)]
pub struct DeploymentsPage {
    pub data: Vec<DeploymentEntry>,
    pub total: i64,
    pub limit: u32,
    pub offset: u32,
}

/// `GET /api/apps/{id}/deployments` — historique paginé des déploiements (HUSKER-25).
/// 404 si l'app n'existe pas ; app sans déploiement -> 200 + `data: []`.
///
/// Check d'existence + requête principale dans **une même transaction** (lecture seule,
/// jamais commitée explicitement en cas d'erreur -> rollback implicite au drop) : sans ça,
/// un `DELETE /api/apps/{id}` concurrent entre les deux statements ferait retourner
/// `200 + []` au lieu de `404` pour une app qui vient de disparaître.
///
/// Requête single-pass en 3 CTE : `matched` (deploys de l'app) -> `totals` (leur compte,
/// toujours 1 ligne) -> `paged` (tri + LIMIT/OFFSET). `totals` est jointe en `CROSS JOIN`
/// puis `paged`/`deployment_signals` en `LEFT JOIN` : le `total` reste lisible même quand
/// `offset` dépasse le total et que `paged` ne produit aucune ligne — évite un 2e `SELECT
/// COUNT(*)` séparé (race possible avec une écriture concurrente, cf. specs /refine).
/// Dédup des signaux multiples par déploiement par comparaison au dernier élément de `data` :
/// le tri (`ORDER BY ... paged.id`) garantit que les lignes d'un même déploiement arrivent
/// contiguës, donc un simple `data.last()` suffit (pas besoin de `HashMap`).
pub async fn list_deployments(
    Path(app_id): Path<i64>,
    State(state): State<AppState>,
    ValidatedQuery(pagination): ValidatedQuery<PaginationParams>,
) -> Result<Json<DeploymentsPage>, AppError> {
    let mut tx = state.pool.begin().await?;

    sqlx::query_scalar!("SELECT id FROM apps WHERE id = ?", app_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or(AppError::NotFound)?;

    let limit = pagination.limit.unwrap_or(20).clamp(1, 100);
    let offset = pagination.offset.unwrap_or(0);

    let rows = sqlx::query!(
        r#"
        WITH matched AS (
            SELECT id, status, git_sha, started_at, finished_at, log_path
            FROM deployments
            WHERE app_id = ?
        ),
        totals AS (
            SELECT COUNT(*) as total FROM matched
        ),
        paged AS (
            SELECT * FROM matched
            ORDER BY started_at DESC, id DESC
            LIMIT ? OFFSET ?
        )
        SELECT
            totals.total as "total!: i64",
            paged.id as "deployment_id?: i64",
            paged.status as "status?: String",
            paged.git_sha as "git_sha?: String",
            paged.started_at as "started_at?: String",
            paged.finished_at as "finished_at?: String",
            paged.log_path as "log_path?: String",
            s.id as "signal_id?: i64",
            s.kind as "signal_kind?: String",
            s.message as "signal_message?: String",
            s.created_at as "signal_created_at?: String"
        FROM totals
        LEFT JOIN paged ON 1 = 1
        LEFT JOIN deployment_signals s ON s.deployment_id = paged.id
        ORDER BY paged.started_at DESC, paged.id DESC, s.id ASC
        "#,
        app_id,
        limit,
        offset
    )
    .fetch_all(&mut *tx)
    .await?;

    tx.commit().await?;

    let total = rows
        .first()
        .expect("totals CTE produit toujours exactement une ligne de groupe")
        .total;

    let mut data: Vec<DeploymentEntry> = Vec::new();

    for row in rows {
        let Some(deployment_id) = row.deployment_id else {
            continue;
        };

        if data.last().map(|e: &DeploymentEntry| e.id) != Some(deployment_id) {
            data.push(DeploymentEntry {
                id: deployment_id,
                status: row.status.unwrap_or_default(),
                git_sha: row.git_sha,
                started_at: row.started_at.unwrap_or_default(),
                finished_at: row.finished_at,
                log_path: row.log_path,
                signals: Vec::new(),
            });
        }

        if let Some(signal_id) = row.signal_id {
            data.last_mut().unwrap().signals.push(DeploymentSignal {
                id: signal_id,
                kind: row.signal_kind.unwrap_or_default(),
                message: row.signal_message.unwrap_or_default(),
                created_at: row.signal_created_at.unwrap_or_default(),
            });
        }
    }

    Ok(Json(DeploymentsPage {
        data,
        total,
        limit,
        offset,
    }))
}

#[cfg(test)]
mod tests;
