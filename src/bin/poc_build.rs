//! POC HUSKER-11 — build d'image Docker via BuildKit (bollard) + streaming du progress.
//!
//! 2e maillon du pipeline M3 (`git → build → run`). Binaire à part pour défricher
//! l'API build de `bollard` (streaming async) avant intégration (HUSKER-13).
//!
//! Décision ADR-013 : builder = BuildKit (pas le builder classique). L'API réutilise
//! `docker.build_image()` avec `BuilderVersion::BuilderBuildKit` + une session ; le
//! progress arrive en `BuildInfoAux::BuildKit` (statut structuré), pas en texte brut.
//!
//! Usage : cargo run --bin poc_build -- [project] [app] [sha] [context_dir]
//!   defaults : demo hello dev tests/fixtures/build-context

use bollard::models::{BuildInfo, BuildInfoAux};
use bollard::query_parameters::{BuildImageOptionsBuilder, BuilderVersion};
use bollard::Docker;
use flate2::{write::GzEncoder, Compression};
use futures_util::StreamExt;
use std::path::Path;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let project = args.next().unwrap_or_else(|| "demo".to_string());
    let app = args.next().unwrap_or_else(|| "hello".to_string());
    let sha = args.next().unwrap_or_else(|| "dev".to_string());
    let context_dir = args
        .next()
        .unwrap_or_else(|| "tests/fixtures/build-context".to_string());

    let tag = build_tag(&project, &app, &sha);
    println!("tag      : {tag}");
    println!("contexte : {context_dir}");

    let docker = Docker::connect_with_local_defaults()?;
    let context = make_context_targz(Path::new(&context_dir))?;

    build_with_buildkit(&docker, context, &tag).await?;
    println!("✓ image {tag} buildée");
    Ok(())
}

/// Tag d'image d'app : `husker/{project}_{app}:{sha}` (cf. ticket).
fn build_tag(project: &str, app: &str, sha: &str) -> String {
    format!("husker/{project}_{app}:{sha}")
}

/// Empaquette le dossier `dir` en tar.gz — le format de contexte de build attendu
/// par l'API Docker (le Dockerfile doit être à la racine de l'archive).
fn make_context_targz(dir: &Path) -> std::io::Result<Vec<u8>> {
    let enc = GzEncoder::new(Vec::new(), Compression::default());
    let mut tar = tar::Builder::new(enc);
    tar.append_dir_all(".", dir)?; // "." = racine de l'archive
    tar.into_inner()?.finish() // finir le tar, puis le gzip -> Vec<u8>
}

/// Build via BuildKit + streaming du progress. Échec remonté en `Err`, jamais de panic.
async fn build_with_buildkit(
    docker: &Docker,
    context: Vec<u8>,
    tag: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    // BuildKit corrèle un build à une session (variante providerless = sans secrets/ssh).
    let session = format!("husker-{}", uuid::Uuid::new_v4());
    let opts = BuildImageOptionsBuilder::default()
        .dockerfile("Dockerfile")
        .t(tag)
        .version(BuilderVersion::BuilderBuildKit)
        .pull("true")
        .session(&session)
        .build();

    let mut stream = docker.build_image(opts, None, Some(bollard::body_full(context.into())));

    // BuildKit ré-émet le statut de chaque étape à chaque update : on ne logge chaque
    // transition (start / done) qu'une fois, clé = digest du vertex + état.
    let mut seen = std::collections::HashSet::new();

    while let Some(item) = stream.next().await {
        // Une erreur de transport/build remonte ici proprement (`?`), pas un panic.
        let BuildInfo {
            aux,
            stream,
            error_detail,
            ..
        } = item?;

        if let Some(BuildInfoAux::BuildKit(status)) = aux {
            // Progress structuré BuildKit : étapes (vertexes) + logs au fil de l'eau.
            for v in status.vertexes {
                let done = v.completed.is_some();
                if !done && v.started.is_none() {
                    continue;
                }
                if seen.insert(format!("{}:{done}", v.digest)) {
                    let sym = if !v.error.is_empty() {
                        "✗"
                    } else if done {
                        "✓"
                    } else {
                        "→"
                    };
                    println!("  {sym} {}", v.name);
                }
            }
            for log in status.logs {
                print!("{}", String::from_utf8_lossy(&log.msg));
            }
        } else if let Some(s) = stream {
            // Fallback (builder classique) — au cas où BuildKit ne serait pas actif.
            print!("{s}");
        }

        if let Some(err) = error_detail {
            return Err(format!("build échoué : {}", err.message.unwrap_or_default()).into());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_tag_format() {
        assert_eq!(build_tag("demo", "hello", "abc123"), "husker/demo_hello:abc123");
    }

    #[test]
    fn make_context_targz_contains_dockerfile() {
        let gz = make_context_targz(Path::new("tests/fixtures/build-context")).unwrap();
        assert!(!gz.is_empty(), "archive vide");

        // Décode l'archive et vérifie que le Dockerfile y est.
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
        let tag = build_tag("test", "hello", "deadbeef");
        let ctx = make_context_targz(Path::new("tests/fixtures/build-context")).unwrap();
        // Le build réel doit réussir (et avoir streamé du progress en sortie).
        build_with_buildkit(&docker, ctx, &tag).await.unwrap();
    }
}
