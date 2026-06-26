//! Maillon `build` du pipeline de déploiement (extrait du POC HUSKER-11).
//!
//! Build d'image via BuildKit (ADR-013). Différence avec le POC : `dockerfile_path`
//! paramétrable (honore `app.dockerfile_path`). Erreurs de transport bollard mappées
//! vers `AppError::Docker` (→ 502) ; un build qui échoue côté contenu -> `AppError::Deploy`.

use crate::errors::AppError;
use bollard::models::{BuildInfo, BuildInfoAux};
use bollard::query_parameters::{BuildImageOptionsBuilder, BuilderVersion};
use bollard::Docker;
use flate2::{write::GzEncoder, Compression};
use futures_util::StreamExt;
use std::path::Path;

/// Image reference complète d'une app : `husker/{project}_{app}:{sha}` (`repository:tag`).
pub fn image_ref(project: &str, app: &str, sha: &str) -> String {
    format!("husker/{project}_{app}:{sha}")
}

/// Empaquette le dossier `dir` en tar.gz — format de contexte attendu par l'API Docker.
/// Le Dockerfile (à `dockerfile_path` relatif) est inclus tel quel dans l'archive.
pub fn make_context_targz(dir: &Path) -> Result<Vec<u8>, AppError> {
    fn inner(dir: &Path) -> std::io::Result<Vec<u8>> {
        let enc = GzEncoder::new(Vec::new(), Compression::default());
        let mut tar = tar::Builder::new(enc);
        tar.append_dir_all(".", dir)?; // "." = racine de l'archive
        tar.into_inner()?.finish()
    }
    inner(dir).map_err(|e| AppError::Deploy(format!("build context: {e}")))
}

/// Build via BuildKit + log du progress (tracing). Échec remonté en `Err`, jamais de panic.
pub async fn build_with_buildkit(
    docker: &Docker,
    context: Vec<u8>,
    image_ref: &str,
    dockerfile_path: &str,
) -> Result<(), AppError> {
    // BuildKit corrèle un build à une session (variante providerless = sans secrets/ssh).
    let session = format!("husker-{}", uuid::Uuid::new_v4());
    let opts = BuildImageOptionsBuilder::default()
        .dockerfile(dockerfile_path)
        .t(image_ref)
        .version(BuilderVersion::BuilderBuildKit)
        .pull("true")
        .session(&session)
        .build();

    let mut stream = docker.build_image(opts, None, Some(bollard::body_full(context.into())));

    // BuildKit ré-émet le statut de chaque étape : on ne logge chaque transition qu'une fois.
    let mut seen = std::collections::HashSet::new();

    while let Some(item) = stream.next().await {
        // Erreur de transport/build -> AppError::Docker (via #[from]), pas un panic.
        let BuildInfo {
            aux,
            stream,
            error_detail,
            ..
        } = item?;

        if let Some(BuildInfoAux::BuildKit(status)) = aux {
            for v in status.vertexes {
                let done = v.completed.is_some();
                if !done && v.started.is_none() {
                    continue;
                }
                if seen.insert(format!("{}:{done}", v.digest)) {
                    tracing::debug!(step = %v.name, done, "build step");
                }
            }
            for log in status.logs {
                tracing::debug!("{}", String::from_utf8_lossy(&log.msg));
            }
        } else if let Some(s) = stream {
            tracing::debug!("{s}");
        }

        if let Some(err) = error_detail {
            return Err(AppError::Deploy(format!(
                "build échoué : {}",
                err.message.unwrap_or_default()
            )));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_ref_format() {
        assert_eq!(image_ref("demo", "hello", "abc123"), "husker/demo_hello:abc123");
    }

    #[test]
    fn make_context_targz_contains_dockerfile() {
        let gz = make_context_targz(Path::new("tests/fixtures/build-context")).unwrap();
        assert!(!gz.is_empty(), "archive vide");

        use flate2::read::GzDecoder;
        let mut archive = tar::Archive::new(GzDecoder::new(&gz[..]));
        let names: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| e.path().ok().map(|p| p.to_string_lossy().into_owned()))
            .collect();
        assert!(
            names.iter().any(|n| n.contains("Dockerfile")),
            "Dockerfile absent de l'archive : {names:?}"
        );
    }

    #[tokio::test]
    #[ignore = "Docker: build BuildKit réel — nécessite un daemon (cargo test -- --ignored)"]
    async fn build_real_image_via_buildkit() {
        let docker = Docker::connect_with_local_defaults().unwrap();
        let img = image_ref("test", "hello", "deadbeef");
        let ctx = make_context_targz(Path::new("tests/fixtures/build-context")).unwrap();
        build_with_buildkit(&docker, ctx, &img, "Dockerfile")
            .await
            .unwrap();
    }
}
