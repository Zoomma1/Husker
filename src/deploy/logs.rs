//! Log de déploiement persisté sur disque (HUSKER-23) + rétention.
//!
//! Écriture et rétention **best-effort** : un disque plein ou une racine non inscriptible
//! ne fait jamais échouer un déploiement qui aurait réussi — même principe qu'ADR-020 côté
//! signaux sécu (un défaut d'écriture est loggé en `tracing`, jamais propagé).

use std::io::Write;
use std::path::{Path, PathBuf};

/// Taille max d'un fichier de log : protège contre un build très bavard.
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

/// Nombre de déploiements dont le log est conservé, par app : protège contre une app
/// redéployée cent fois.
const MAX_DEPLOYMENTS_KEPT: i64 = 20;

/// Racine des logs de déploiement (`HUSKER_LOGS_ROOT`, défaut `logs`) — même convention que
/// `HUSKER_SOURCES_ROOT` / `HUSKER_DATA_ROOT`.
pub fn logs_root() -> String {
    std::env::var("HUSKER_LOGS_ROOT").unwrap_or_else(|_| "logs".to_string())
}

/// `<root>/<deployment_id>.log` — `PathBuf::join` uniquement (portabilité, ADR-016).
pub fn log_path(root: &str, deployment_id: i64) -> PathBuf {
    Path::new(root).join(format!("{deployment_id}.log"))
}

/// Fichier de log ouvert pour un déploiement. `file` est `None` si l'ouverture a échoué
/// (disque plein, racine non inscriptible) — les écritures deviennent alors des no-op.
pub struct DeploymentLog {
    file: Option<std::fs::File>,
    written: u64,
}

impl DeploymentLog {
    /// Crée `<root>/<deployment_id>.log`. Best-effort : une erreur est loggée en `tracing`,
    /// jamais propagée — le pipeline continue sans fichier.
    pub fn open(root: &str, deployment_id: i64) -> Self {
        let path = log_path(root, deployment_id);
        let file = std::fs::create_dir_all(root)
            .and_then(|_| std::fs::File::create(&path))
            .map_err(|e| {
                tracing::warn!(
                    deployment_id,
                    path = %path.display(),
                    "log de déploiement non ouvert : {e}"
                );
            })
            .ok();
        Self { file, written: 0 }
    }

    /// Écrit une ligne. No-op si le fichier n'a pas pu s'ouvrir ou si la taille max est
    /// atteinte (le log est tronqué, pas le déploiement).
    pub fn write_line(&mut self, line: &str) {
        let Some(file) = self.file.as_mut() else {
            return;
        };
        if self.written >= MAX_LOG_BYTES {
            return;
        }
        let bytes = format!("{line}\n");
        match file.write_all(bytes.as_bytes()) {
            Ok(()) => self.written += bytes.len() as u64,
            Err(e) => {
                tracing::warn!("écriture log de déploiement : {e}");
                self.file = None;
            }
        }
    }
}

/// Supprime les fichiers de log des déploiements de `app_id` au-delà des
/// `MAX_DEPLOYMENTS_KEPT` plus récents. Best-effort, appelée après la clôture du déploiement
/// — jamais dans le chemin critique du deploy.
pub async fn apply_retention(pool: &sqlx::SqlitePool, app_id: i64, root: &str) {
    let stale_ids = sqlx::query_scalar!(
        "SELECT id FROM deployments WHERE app_id = ? ORDER BY id DESC LIMIT -1 OFFSET ?",
        app_id,
        MAX_DEPLOYMENTS_KEPT
    )
    .fetch_all(pool)
    .await;

    let stale_ids = match stale_ids {
        Ok(ids) => ids,
        Err(e) => {
            tracing::warn!(app_id, "rétention logs : lecture deployments : {e}");
            return;
        }
    };

    for id in stale_ids {
        let path = log_path(root, id);
        if let Err(e) = std::fs::remove_file(&path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(deployment_id = id, "rétention logs : suppression : {e}");
                continue; // fichier potentiellement toujours là -> ne pas désynchroniser la DB
            }
        }
        // Le fichier a disparu (supprimé ci-dessus ou déjà absent) : `log_path` ne doit plus
        // pointer vers rien, sinon l'historique (HUSKER-25) renvoie un chemin mort.
        if let Err(e) = sqlx::query!("UPDATE deployments SET log_path = NULL WHERE id = ?", id)
            .execute(pool)
            .await
        {
            tracing::warn!(
                deployment_id = id,
                "rétention logs : log_path non vidé : {e}"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_path_joins_root_and_id() {
        assert_eq!(log_path("logs", 42), Path::new("logs/42.log"));
    }

    #[test]
    fn write_line_is_noop_when_root_unwritable() {
        let tmp = std::env::temp_dir().join(format!("husker-logs-ut-{}", uuid::Uuid::new_v4()));
        std::fs::write(&tmp, b"je suis un fichier, pas un dossier").unwrap();

        // `root` collisionne avec un fichier existant -> create_dir_all échoue.
        let mut log = DeploymentLog::open(tmp.to_str().unwrap(), 1);
        log.write_line("ne doit jamais paniquer");

        std::fs::remove_file(&tmp).unwrap();
    }

    #[test]
    fn write_line_caps_at_max_bytes() {
        let tmp = std::env::temp_dir().join(format!("husker-logs-ut-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();

        let mut log = DeploymentLog::open(tmp.to_str().unwrap(), 1);
        // Une ligne dépassant déjà le cap à elle seule.
        log.written = MAX_LOG_BYTES;
        log.write_line("cette ligne ne doit pas être écrite");

        let content = std::fs::read_to_string(log_path(tmp.to_str().unwrap(), 1)).unwrap();
        assert!(
            content.is_empty(),
            "rien écrit au-delà du cap : {content:?}"
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}
