//! Suivi des digests des images de base (HUSKER-15, phase 3).
//!
//! Complément de [`super::policy`], qui ne fait que de l'analyse statique. Ici on interroge
//! le registry pour savoir ce que la référence résout **aujourd'hui**, et on le compare à ce
//! qu'elle résolvait au deploy précédent. Même `alpine:3.20`, digest différent = le tag a
//! été réécrit upstream.
//!
//! Rien ici ne fait échouer un deploy : un registry injoignable est une panne d'infra, pas
//! une faute de l'app. Tout remonte en warnings.

use super::policy::BaseImage;
use crate::errors::AppError;
use bollard::Docker;
use sqlx::SqlitePool;

/// Ce que le digest observé dit par rapport à l'historique de l'app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Drift {
    /// Première observation de cette référence pour cette app — rien à comparer.
    First,
    /// Le tag résout toujours vers le même contenu.
    Unchanged,
    /// Même référence, contenu différent : le tag a bougé sous nos pieds.
    Drifted { previous: String, current: String },
}

/// Compare le digest observé à celui du deploy précédent. Pure — le cœur de la détection.
pub fn classify(previous: Option<&str>, current: &str) -> Drift {
    match previous {
        None => Drift::First,
        Some(p) if p == current => Drift::Unchanged,
        Some(p) => Drift::Drifted {
            previous: p.to_string(),
            current: current.to_string(),
        },
    }
}

/// Digest que le registry associe *actuellement* à cette référence, sans pull de l'image
/// (endpoint `/distribution/{ref}/json`).
pub async fn resolve_digest(docker: &Docker, reference: &str) -> Result<String, AppError> {
    let inspect = docker.inspect_registry_image(reference, None).await?;
    inspect.descriptor.digest.ok_or_else(|| {
        AppError::Deploy(format!(
            "le registry n'a pas renvoyé de digest pour `{reference}`"
        ))
    })
}

/// Digest mémorisé au deploy précédent, s'il y en a un.
pub async fn previous_digest(
    pool: &SqlitePool,
    app_id: i64,
    reference: &str,
) -> Result<Option<String>, AppError> {
    let row = sqlx::query!(
        "SELECT digest FROM base_image_digests WHERE app_id = ? AND reference = ?",
        app_id,
        reference
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.digest))
}

/// Mémorise le digest observé. `first_seen_at` est préservé lors d'une mise à jour : il date
/// la première fois que Husker a vu cette référence, pas la dernière.
pub async fn record_digest(
    pool: &SqlitePool,
    app_id: i64,
    reference: &str,
    digest: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query!(
        "INSERT INTO base_image_digests (app_id, reference, digest, first_seen_at, last_seen_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(app_id, reference)
         DO UPDATE SET digest = excluded.digest, last_seen_at = excluded.last_seen_at",
        app_id,
        reference,
        digest,
        now,
        now
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Images qui valent un aller-retour registry, dans l'ordre du Dockerfile.
///
/// Écartées : les images **pinnées** (leur référence *est* leur digest, elle ne peut pas
/// dériver), les références **non résolubles** (`FROM $BASE` — interroger la chaîne
/// littérale échouerait, et `policy::check` a déjà signalé le cas), et les **doublons**
/// (une même base réutilisée par plusieurs stages ne vaut qu'une résolution).
pub fn resolvable_targets(images: &[BaseImage]) -> Vec<&BaseImage> {
    let mut seen = std::collections::HashSet::new();
    images
        .iter()
        .filter(|i| !i.is_pinned() && i.registry.is_some())
        .filter(|i| seen.insert(i.raw.as_str()))
        .collect()
}

/// Résout puis compare le digest de chaque image de base, et mémorise le résultat.
///
/// Ne renvoie jamais d'`Err` : chaque échec (registry injoignable, référence exotique)
/// devient un warning et n'empêche pas le deploy.
pub async fn track(
    pool: &SqlitePool,
    docker: &Docker,
    app_id: i64,
    images: &[BaseImage],
) -> Vec<String> {
    let mut warnings = Vec::new();

    for image in resolvable_targets(images) {
        let current = match resolve_digest(docker, &image.raw).await {
            Ok(d) => d,
            Err(e) => {
                warnings.push(format!(
                    "digest de `{}` non résolu ({e}) : dérive de tag non vérifiable ce deploy",
                    image.raw
                ));
                continue;
            }
        };

        let previous = match previous_digest(pool, app_id, &image.raw).await {
            Ok(p) => p,
            Err(e) => {
                warnings.push(format!(
                    "lecture du digest mémorisé de `{}` : {e}",
                    image.raw
                ));
                continue;
            }
        };

        if let Drift::Drifted { previous, current } = classify(previous.as_deref(), &current) {
            warnings.push(format!(
                "⚠ le tag `{}` a changé de contenu : {previous} -> {current}",
                image.raw
            ));
        }

        if let Err(e) = record_digest(pool, app_id, &image.raw, &current).await {
            warnings.push(format!("mémorisation du digest de `{}` : {e}", image.raw));
        }
    }

    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use std::str::FromStr;

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    /// Seed minimal : une app rattachée à un projet (FK non contraintes, mais on reste réaliste).
    async fn seed_app(pool: &SqlitePool) -> i64 {
        sqlx::query!(
            "INSERT INTO projects (name, network_name, created_at) VALUES ('p', 'husker_p', 'now')"
        )
        .execute(pool)
        .await
        .unwrap();
        sqlx::query!(
            "INSERT INTO apps (project_id, name, git_url, git_branch, dockerfile_path, created_at, status)
             VALUES (1, 'a', 'url', 'main', 'Dockerfile', 'now', 'pending')"
        )
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid()
    }

    /// Helper : une image de base non pinnée sur un registry résoluble.
    fn img(raw: &str) -> BaseImage {
        BaseImage {
            raw: raw.to_string(),
            registry: Some("docker.io".to_string()),
            digest: None,
        }
    }

    #[test]
    fn pinned_images_are_never_resolved() {
        // Leur référence est déjà le digest : rien à interroger.
        let images = vec![BaseImage {
            raw: "alpine@sha256:deadbeef".to_string(),
            registry: Some("docker.io".to_string()),
            digest: Some("sha256:deadbeef".to_string()),
        }];
        assert!(resolvable_targets(&images).is_empty());
    }

    #[test]
    fn unresolvable_references_are_never_resolved() {
        // `FROM $BASE` : interroger la chaîne littérale échouerait à coup sûr, et
        // `policy::check` a déjà émis un warning dédié -> pas de doublon de bruit.
        let images = vec![BaseImage {
            raw: "$BASE".to_string(),
            registry: None,
            digest: None,
        }];
        assert!(resolvable_targets(&images).is_empty());
    }

    #[test]
    fn a_base_reused_across_stages_is_resolved_once() {
        // `FROM node:20 AS builder` + `FROM node:20` : deux entrées, une seule résolution.
        let images = vec![img("node:20"), img("node:20")];
        let targets = resolvable_targets(&images);
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[0].raw, "node:20");
    }

    #[test]
    fn distinct_bases_are_all_kept_in_dockerfile_order() {
        let images = vec![img("golang:1.22"), img("alpine:3.20"), img("golang:1.22")];
        let raws: Vec<&str> = resolvable_targets(&images)
            .iter()
            .map(|i| i.raw.as_str())
            .collect();
        assert_eq!(raws, vec!["golang:1.22", "alpine:3.20"]);
    }

    #[test]
    fn first_observation_has_nothing_to_compare() {
        assert_eq!(classify(None, "sha256:aaa"), Drift::First);
    }

    #[test]
    fn same_digest_is_unchanged() {
        assert_eq!(classify(Some("sha256:aaa"), "sha256:aaa"), Drift::Unchanged);
    }

    #[test]
    fn different_digest_for_same_reference_is_drift() {
        assert_eq!(
            classify(Some("sha256:aaa"), "sha256:bbb"),
            Drift::Drifted {
                previous: "sha256:aaa".to_string(),
                current: "sha256:bbb".to_string(),
            }
        );
    }

    #[tokio::test]
    async fn unknown_reference_has_no_previous_digest() {
        let pool = test_pool().await;
        let app_id = seed_app(&pool).await;
        assert_eq!(
            previous_digest(&pool, app_id, "alpine:3.20").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn recorded_digest_is_read_back() {
        let pool = test_pool().await;
        let app_id = seed_app(&pool).await;
        record_digest(&pool, app_id, "alpine:3.20", "sha256:aaa")
            .await
            .unwrap();
        assert_eq!(
            previous_digest(&pool, app_id, "alpine:3.20").await.unwrap(),
            Some("sha256:aaa".to_string())
        );
    }

    #[tokio::test]
    async fn re_recording_updates_in_place_and_keeps_first_seen() {
        let pool = test_pool().await;
        let app_id = seed_app(&pool).await;
        record_digest(&pool, app_id, "alpine:3.20", "sha256:aaa")
            .await
            .unwrap();
        let first = sqlx::query!("SELECT first_seen_at FROM base_image_digests")
            .fetch_one(&pool)
            .await
            .unwrap()
            .first_seen_at;

        record_digest(&pool, app_id, "alpine:3.20", "sha256:bbb")
            .await
            .unwrap();

        let rows = sqlx::query!("SELECT digest, first_seen_at FROM base_image_digests")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "upsert, pas d'accumulation de lignes");
        assert_eq!(rows[0].digest, "sha256:bbb");
        assert_eq!(
            rows[0].first_seen_at, first,
            "première observation préservée"
        );
    }

    #[tokio::test]
    async fn digests_are_scoped_per_app() {
        // Deux apps peuvent utiliser `alpine:3.20` sans partager leur historique.
        let pool = test_pool().await;
        let app_a = seed_app(&pool).await;
        let app_b = sqlx::query!(
            "INSERT INTO apps (project_id, name, git_url, git_branch, dockerfile_path, created_at, status)
             VALUES (1, 'b', 'url', 'main', 'Dockerfile', 'now', 'pending')"
        )
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();

        record_digest(&pool, app_a, "alpine:3.20", "sha256:aaa")
            .await
            .unwrap();
        assert_eq!(
            previous_digest(&pool, app_b, "alpine:3.20").await.unwrap(),
            None
        );
    }

    #[tokio::test]
    #[ignore = "réseau: interroge Docker Hub (cargo test -- --ignored)"]
    async fn resolves_a_real_digest_from_docker_hub() {
        let docker = Docker::connect_with_local_defaults().unwrap();
        let digest = resolve_digest(&docker, "alpine:3.20").await.unwrap();
        assert!(digest.starts_with("sha256:"), "digest inattendu : {digest}");
    }

    #[tokio::test]
    #[ignore = "réseau: interroge Docker Hub (cargo test -- --ignored)"]
    async fn track_flags_a_stale_digest_as_drift_and_refreshes_it() {
        // Le chemin qui justifie tout le dispositif : un digest mémorisé qui ne correspond
        // plus à ce que le tag résout aujourd'hui. Simulé en semant un digest bidon.
        let pool = test_pool().await;
        let app_id = seed_app(&pool).await;
        record_digest(&pool, app_id, "alpine:3.20", "sha256:stale")
            .await
            .unwrap();

        let images = vec![
            BaseImage {
                raw: "alpine:3.20".to_string(),
                registry: Some("docker.io".to_string()),
                digest: None,
            },
            // Déjà pinnée : sa référence *est* son digest -> jamais interrogée ni mémorisée.
            BaseImage {
                raw: "alpine@sha256:deadbeef".to_string(),
                registry: Some("docker.io".to_string()),
                digest: Some("sha256:deadbeef".to_string()),
            },
        ];

        let docker = Docker::connect_with_local_defaults().unwrap();
        let warnings = track(&pool, &docker, app_id, &images).await;

        assert!(
            warnings.iter().any(|w| w.contains("a changé de contenu")),
            "dérive non signalée : {warnings:?}"
        );

        let refreshed = previous_digest(&pool, app_id, "alpine:3.20")
            .await
            .unwrap()
            .unwrap();
        assert_ne!(refreshed, "sha256:stale", "digest non rafraîchi");

        assert_eq!(
            previous_digest(&pool, app_id, "alpine@sha256:deadbeef")
                .await
                .unwrap(),
            None,
            "une image pinnée ne doit rien écrire"
        );
    }
}
