use axum::{extract::{State, Path}, Json};
use axum::http::StatusCode;
use serde::{Deserialize, Serialize};
use crate::state::AppState;
use crate::errors::AppError;
use crate::docker;

#[derive(Deserialize)]
pub struct CreateProjectRequest {
    pub name: String,
}

#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub struct Project {
    pub id: i64,
    pub name: String,
    pub network_name: String,
    pub created_at: String,
}

pub async fn create_project(
    State(state): State<AppState>,
    Json(payload): Json<CreateProjectRequest>,
) -> Result<(StatusCode, Json<Project>), AppError> {
    if payload.name.trim().is_empty() {
        return Err(AppError::Validation("name cannot be empty".into()));
    }
    let existing = sqlx::query!(
        "SELECT id FROM projects WHERE name = ?",
        payload.name
    ).fetch_optional(&state.pool).await?;

    if existing.is_some() {
        return Err(AppError::Validation(format!(
            "project '{}' already exists", payload.name
        )));
    }

    let network_name = format!("husker_{}", payload.name);

    docker::network::create_network(&state.docker, &network_name).await?;
    let created_at = chrono::Utc::now().to_rfc3339();
    let res = sqlx::query_as!(
        Project,
        "INSERT INTO projects (name, network_name, created_at)
         VALUES (?, ?, ?)
         RETURNING id, name, network_name, created_at",
        payload.name,
        network_name,
        created_at
    ).fetch_one(&state.pool).await;
    match res {
        Ok(project) => Ok((StatusCode::CREATED, Json(project))),
        Err(e) => {
            docker::network::delete_network(&state.docker, &network_name).await.ok();
            Err(e.into())
        }
    }
}

pub async fn list_projects(
    State(state): State<AppState>,
) -> Result<Json<Vec<Project>>, AppError> {
    let projects = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects ORDER BY id ASC"
    ).fetch_all(&state.pool).await?;
    Ok(Json(projects))
}

pub async fn get_project(
    State(state): State<AppState>,
    Path(project_id): Path<i64>,
) -> Result<Json<Project>, AppError> {
    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        project_id
    ).fetch_optional(&state.pool).await?;

    match project {
        Some(p) => Ok(Json(p)),
        None => Err(AppError::NotFound),
    }
}

pub async fn delete_project(
    State(state): State<AppState>,
    Path(project_id): Path<i64>,
) -> Result<StatusCode, AppError> {
    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        project_id
    ).fetch_optional(&state.pool).await?;

    let project = match project {
        Some(p) => p,
        None => return Err(AppError::NotFound),
    };

    match docker::network::delete_network(&state.docker, &project.network_name).await {
        Ok(_) => {}
        Err(bollard::errors::Error::DockerResponseServerError { status_code: 404, .. }) => {
            tracing::warn!(
                network = %project.network_name,
                "Docker network already gone, proceeding with DB delete"
            );
        }
        Err(e) => return Err(e.into()),
    }

    sqlx::query!("DELETE FROM projects WHERE id = ?", project_id)
        .execute(&state.pool).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests;