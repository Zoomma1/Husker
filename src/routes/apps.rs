use axum::extract::{State, Path};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use crate::errors::AppError;
use crate::routes::projects::Project;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateAppRequest {
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
    Json(payload): Json<CreateAppRequest>,
) -> Result<(StatusCode, Json<App>), AppError> {
    if payload.name.trim().is_empty() {
        return Err(AppError::Validation("name cannot be empty".into()));
    }

    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        project_id
    ).fetch_optional(&state.pool).await?;

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
    ).fetch_optional(&state.pool).await?;

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
    ).fetch_optional(&state.pool).await?;

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

pub async fn delete_app(
    Path((project_id, app_id)): Path<(i64, i64)>,
    State(state): State<AppState>,
) -> Result<StatusCode, AppError> {
    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        project_id
    ).fetch_optional(&state.pool).await?;

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

    if app.is_none() {
        return Err(AppError::NotFound);
    }

    sqlx::query!(
        "DELETE FROM apps WHERE id = ? AND project_id = ?",
        app_id,
        project_id
    ).execute(&state.pool).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;