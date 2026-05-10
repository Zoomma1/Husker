mod errors;
mod docker;
mod state;
mod routes;

use axum::{routing::get, routing::post, Json, Router};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::net::TcpListener;
use bollard::Docker;
use crate::state::AppState;

#[derive(Serialize)]
struct PingResponse {
    status: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    let options = SqliteConnectOptions::from_str(&database_url)
        .expect("Invalid DATABASE_URL")
        .create_if_missing(true);
    let pool = SqlitePool::connect_with(options)
        .await
        .expect("Failed to connect to DB");
    sqlx::migrate!()
        .run(&pool)
        .await
        .expect("Failed to run migrations");

    let listener = TcpListener::bind(format!("0.0.0.0:{port}")).await.unwrap();

    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

    tracing_subscriber::registry()
        .with(EnvFilter::from_default_env())
        .with(tracing_subscriber::fmt::layer())
        .init();

    tracing::info!("Starting Husker on port {port}");

    let docker = Docker::connect_with_local_defaults().unwrap();

    let state = AppState {
        pool,
        docker,
    };
    let app = app(state);
    
    axum::serve(listener, app).await.unwrap();
}

fn app(state: AppState) -> Router {
    Router::new()
        .route("/ping", get(ping))
        .route("/api/projects", post(routes::projects::create_project).get(routes::projects::list_projects))
        .route("/api/projects/{id}", get(routes::projects::get_project).delete(routes::projects::delete_project))
        .with_state(state)
}

async fn ping() -> Json<PingResponse> {
    PingResponse {
        status: "ok".to_string(),
    }
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http};
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
    use tower::ServiceExt;
    #[tokio::test]
    async fn test_ping() {
        let state = AppState {
            pool: test_pool().await,
            docker: Docker::connect_with_local_defaults().unwrap(),
        };
        let app = app(state);
        let request = http::request::Request::builder()
            .uri("/ping")
            .body(Body::empty())
            .unwrap();

        let response = app.oneshot(request).await.unwrap();
        assert_eq!(response.status(), 200);

        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(std::str::from_utf8(&bytes).unwrap(), "{\"status\":\"ok\"}");
    }

    #[cfg(test)]
    async fn test_pool() -> SqlitePool {
        let pool = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(pool).await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    #[tokio::test]
    async fn test_migrations_run() {
        let _pool = test_pool().await;
        // si on arrive ici sans panic, les migrations sont passées
    }
}
