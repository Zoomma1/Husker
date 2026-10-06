use bollard::Docker;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct AppState {
    pub pool: SqlitePool,
    pub docker: Docker,
    /// Un verrou par `app_id` : sérialise `deploy`/`stop`/`restart` sur une même app (elles
    /// touchent le même container Docker déterministe) sans bloquer les autres apps entre
    /// elles. Cf. `deploy::app_lock`.
    locks: Arc<Mutex<HashMap<i64, Arc<Mutex<()>>>>>,
}

impl AppState {
    pub fn new(pool: SqlitePool, docker: Docker) -> Self {
        Self {
            pool,
            docker,
            locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Le verrou de `app_id`, créé à la première demande. Cloné (`Arc`) : l'appelant le
    /// verrouille lui-même et le garde jusqu'à la fin de l'opération.
    pub async fn app_lock(&self, app_id: i64) -> Arc<Mutex<()>> {
        self.locks
            .lock()
            .await
            .entry(app_id)
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }
}
