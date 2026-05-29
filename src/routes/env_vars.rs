use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::Json;
use serde::{Deserialize, Serialize};
use crate::errors::AppError;
use crate::routes::apps::App;
use crate::routes::projects::Project;
use crate::state::AppState;

#[derive(Deserialize)]
pub struct CreateEnvVarRequest {
    pub key: String,
    pub value: String,
}

#[derive(Deserialize, Serialize, sqlx::FromRow)]
pub struct EnvVar {
    pub id: i64,
    pub app_id: i64,
    pub key: String,
    pub value: String,
}

pub async fn create_env(
    Path((project_id, app_id)): Path<(i64, i64)>,
    State(state): State<AppState>,
    Json(payload): Json<CreateEnvVarRequest>,
) -> Result<(StatusCode, Json<EnvVar>), AppError> {
    if payload.key.trim().is_empty() {
        return Err(AppError::BadRequest("key cannot be empty".into()));
    }

    if !payload.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(AppError::BadRequest("key can only contain letters, numbers, and underscores".into()));
    }

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

    let dup = sqlx::query_as!(
        EnvVar,
        "SELECT id, app_id, key, value FROM env_vars WHERE app_id = ? AND key = ?",
        app_id,
        payload.key
    ).fetch_optional(&state.pool).await?;

    if dup.is_some() {
        return Err(AppError::Conflict(format!(
            "env var with key '{}' already exists for this app", payload.key
        )));
    }

    let res = sqlx::query_as!(
        EnvVar,
        "INSERT INTO env_vars (app_id, key, value)
         VALUES (?, ?, ?)
         RETURNING id, app_id, key, value",
        app_id,
        payload.key,
        payload.value
    ).fetch_one(&state.pool).await?;

    Ok((StatusCode::CREATED, Json(res)))
}
#[cfg(test)]
mod tests;