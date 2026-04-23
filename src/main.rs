use axum::{routing::get, Json, Router};
use serde::Serialize;
use sqlx::SqlitePool;
use tokio::net::TcpListener;

#[derive(Serialize)]
struct PingResponse {
    status: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".to_string());
    let app = app();

    let database_url = std::env::var("DATABASE_URL").expect("DATABASE_URL must be set");
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;
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
    println!("Listening on port {port}");
    axum::serve(listener, app).await.unwrap();
}

fn app() -> Router {
    Router::new().route("/ping", get(ping))
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
        let app = app();
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
