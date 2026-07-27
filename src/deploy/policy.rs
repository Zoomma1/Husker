//! Politique supply-chain sur les images de base des apps déployées (HUSKER-15).
//!
//! Husker ne possède pas les Dockerfiles des apps : ils viennent du repo git de
//! l'utilisateur et sont ré-clonés à chaque deploy. Husker ne peut donc pas *pinner*
//! un `FROM` — il peut seulement **inspecter** ce qu'on lui demande de builder et
//! statuer avant que quoi que ce soit ne soit tiré du réseau.
//!
//! Verdict différencié : un registry hors allowlist est **refusé** (422, faute d'input),
//! une base non pinnée est **signalée** — refuser tout tag mouvant reviendrait à refuser
//! la quasi-totalité des Dockerfiles du monde réel.

use crate::errors::AppError;
use std::path::Path;

/// Registry implicite quand le `FROM` n'en nomme pas (`FROM alpine` -> Docker Hub).
pub const DEFAULT_REGISTRY: &str = "docker.io";

/// Registries autorisés, en clair : `HUSKER_ALLOWED_REGISTRIES=ghcr.io,docker.io`.
pub const ALLOWED_REGISTRIES_ENV: &str = "HUSKER_ALLOWED_REGISTRIES";

/// Une image de base référencée par une instruction `FROM`, telle qu'écrite dans le
/// Dockerfile. Les alias de stage (`FROM builder`) et `scratch` n'en sont pas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseImage {
    /// Référence brute, flags et `AS` retirés (ex: `ghcr.io/foo/bar:1.2`).
    pub raw: String,
    /// Host du registry, minuscule. `None` = référence non résoluble statiquement
    /// (`FROM $BASE`) : elle dépend d'un `ARG` évalué au build.
    pub registry: Option<String>,
    /// `Some("sha256:…")` si la référence est pinnée par digest.
    pub digest: Option<String>,
}

impl BaseImage {
    /// Une image pinnée par digest ne peut pas bouger sous nos pieds — c'est le seul
    /// état qui neutralise le risque « tag de confiance compromis upstream ».
    pub fn is_pinned(&self) -> bool {
        self.digest.is_some()
    }
}

/// Extrait les images de base d'un Dockerfile.
///
/// Ignore : commentaires, `scratch`, et les `FROM <alias>` qui pointent vers un stage
/// déclaré plus haut (`FROM x AS builder` … `FROM builder`) — un alias n'est pas une
/// image tirée d'un registry.
pub fn parse_base_images(dockerfile: &str) -> Vec<BaseImage> {
    let mut images = Vec::new();
    // Noms de stage déjà déclarés (minuscules — Docker les normalise ainsi).
    let mut stages: Vec<String> = Vec::new();

    for line in logical_lines(dockerfile) {
        let mut tokens = line.split_whitespace();

        match tokens.next() {
            Some(kw) if kw.eq_ignore_ascii_case("from") => {}
            _ => continue,
        }

        // `FROM --platform=linux/amd64 alpine` : les flags précèdent la référence.
        let Some(reference) = tokens.find(|t| !t.starts_with("--")) else {
            continue;
        };

        let is_alias = stages.iter().any(|s| s.eq_ignore_ascii_case(reference));

        // `FROM x AS builder` : enregistré après le test ci-dessus, un stage ne peut pas
        // se référencer lui-même.
        let rest: Vec<&str> = tokens.collect();
        if let Some(pos) = rest.iter().position(|t| t.eq_ignore_ascii_case("as")) {
            if let Some(alias) = rest.get(pos + 1) {
                stages.push(alias.to_ascii_lowercase());
            }
        }

        if is_alias || reference.eq_ignore_ascii_case("scratch") {
            continue;
        }

        images.push(BaseImage {
            raw: reference.to_string(),
            registry: registry_of(reference),
            digest: reference.split_once('@').map(|(_, d)| d.to_string()),
        });
    }

    images
}

/// Recolle les continuations (`\` en fin de ligne) et écarte commentaires et lignes vides.
fn logical_lines(dockerfile: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pending = String::new();

    for raw in dockerfile.lines() {
        let trimmed = raw.trim();
        if pending.is_empty() && (trimmed.is_empty() || trimmed.starts_with('#')) {
            continue;
        }
        if let Some(head) = trimmed.strip_suffix('\\') {
            pending.push_str(head);
            pending.push(' ');
            continue;
        }
        pending.push_str(trimmed);
        lines.push(std::mem::take(&mut pending));
    }
    if !pending.is_empty() {
        lines.push(pending);
    }
    lines
}

/// Host du registry d'une référence, `None` si un `ARG` le rend indécidable.
///
/// Règle Docker : le premier segment n'est un host que s'il ressemble à un host
/// (point, port, ou `localhost`) — sinon c'est un namespace Docker Hub (`bitnami/nginx`).
fn registry_of(reference: &str) -> Option<String> {
    let head = match reference.split_once('/') {
        Some((head, _)) => head,
        // Sans `/`, seul le nom compte : dans `alpine:$VERSION` la variable est dans le
        // tag, elle ne peut pas déplacer l'image vers un autre registry.
        None => reference.split([':', '@']).next().unwrap_or(reference),
    };

    if head.contains('$') {
        return None; // `FROM $BASE` : peut s'expandre vers n'importe quel registry.
    }

    let is_host = reference.contains('/')
        && (head.contains('.') || head.contains(':') || head == "localhost");

    Some(if is_host {
        head.to_ascii_lowercase()
    } else {
        DEFAULT_REGISTRY.to_string()
    })
}

/// Liste des registries autorisés depuis `HUSKER_ALLOWED_REGISTRIES`.
/// Non configuré -> Docker Hub seul (le défaut le plus restrictif qui laisse Husker utilisable).
pub fn allowed_registries() -> Vec<String> {
    parse_allowed(&std::env::var(ALLOWED_REGISTRIES_ENV).unwrap_or_default())
}

/// Parse la liste en clair. Séparée de la lecture d'env pour rester pure et testable.
pub fn parse_allowed(raw: &str) -> Vec<String> {
    let list: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();

    if list.is_empty() {
        vec![DEFAULT_REGISTRY.to_string()]
    } else {
        list
    }
}

/// Confronte les images de base à l'allowlist.
///
/// `Err` = au moins un registry non autorisé (refus du deploy, 422). `Ok(warnings)` = le
/// deploy peut continuer, chaque warning décrivant un risque résiduel non bloquant.
pub fn check(images: &[BaseImage], allowed: &[String]) -> Result<Vec<String>, AppError> {
    let mut rejected = Vec::new();
    let mut warnings = Vec::new();

    for image in images {
        match &image.registry {
            Some(registry) if !allowed.iter().any(|a| a == registry) => {
                rejected.push(format!("`{}` (registry `{registry}`)", image.raw));
                continue;
            }
            // Un `FROM $BASE` contourne l'allowlist par construction : on ne peut pas savoir
            // vers quoi il s'expand. Signalé plutôt que refusé — `ARG BASE` reste un pattern
            // légitime — mais c'est un trou assumé, pas un cas anodin.
            None => {
                warnings.push(format!(
                    "référence `{}` non résoluble (ARG) : l'allowlist de registries ne peut pas s'y appliquer",
                    image.raw
                ));
                continue; // rien de fiable à dire de plus sur cette image
            }
            _ => {}
        }

        if !image.is_pinned() {
            warnings.push(format!(
                "image de base `{}` non pinnée par digest : le tag peut être réécrit upstream",
                image.raw
            ));
        }
    }

    if !rejected.is_empty() {
        return Err(AppError::Validation(format!(
            "registry non autorisé pour {} — autorisés : {}",
            rejected.join(", "),
            allowed.join(", ")
        )));
    }

    Ok(warnings)
}

/// Lit le Dockerfile d'un contexte cloné et en extrait les images de base.
/// `dockerfile_path` est relatif à la racine du contexte (même résolution que le build).
///
/// Lecture unique : le pipeline enchaîne ensuite [`check`] (statique, hors ligne) puis le
/// suivi des digests, sur la même liste.
pub fn read_base_images(
    context_dir: &Path,
    dockerfile_path: &str,
) -> Result<Vec<BaseImage>, AppError> {
    let path = context_dir.join(dockerfile_path);
    let content = std::fs::read_to_string(&path).map_err(|e| {
        AppError::Deploy(format!("lecture du Dockerfile `{dockerfile_path}` : {e}"))
    })?;

    Ok(parse_base_images(&content))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper : la liste des refs brutes, pour les assertions de cardinalité.
    fn raws(df: &str) -> Vec<String> {
        parse_base_images(df).into_iter().map(|b| b.raw).collect()
    }

    #[test]
    fn parses_simple_tag() {
        let images = parse_base_images("FROM alpine:3.20\nRUN echo hi\n");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].raw, "alpine:3.20");
        assert_eq!(images[0].registry.as_deref(), Some(DEFAULT_REGISTRY));
        assert_eq!(images[0].digest, None);
        assert!(!images[0].is_pinned());
    }

    #[test]
    fn implicit_latest_is_still_a_moving_tag() {
        let images = parse_base_images("FROM alpine\n");
        assert_eq!(images[0].registry.as_deref(), Some(DEFAULT_REGISTRY));
        assert!(!images[0].is_pinned(), "pas de digest -> pas pinnée");
    }

    #[test]
    fn detects_digest_pin() {
        let images = parse_base_images("FROM alpine@sha256:deadbeef\n");
        assert_eq!(images[0].digest.as_deref(), Some("sha256:deadbeef"));
        assert!(images[0].is_pinned());
    }

    #[test]
    fn tag_and_digest_together_counts_as_pinned() {
        // Forme légale : le digest fait autorité, le tag n'est que décoratif.
        let images = parse_base_images("FROM alpine:3.20@sha256:deadbeef\n");
        assert_eq!(images[0].digest.as_deref(), Some("sha256:deadbeef"));
        assert_eq!(images[0].registry.as_deref(), Some(DEFAULT_REGISTRY));
    }

    #[test]
    fn explicit_registry_is_extracted() {
        let images = parse_base_images("FROM ghcr.io/zoomma1/husker:1.0\n");
        assert_eq!(images[0].registry.as_deref(), Some("ghcr.io"));
    }

    #[test]
    fn registry_with_port_is_a_registry() {
        let images = parse_base_images("FROM registry.local:5000/img:1\n");
        assert_eq!(images[0].registry.as_deref(), Some("registry.local:5000"));
    }

    #[test]
    fn namespaced_image_stays_on_docker_hub() {
        // `bitnami` n'est pas un host (ni point, ni port) -> namespace Docker Hub.
        let images = parse_base_images("FROM bitnami/nginx:1.25\n");
        assert_eq!(images[0].registry.as_deref(), Some(DEFAULT_REGISTRY));
    }

    #[test]
    fn localhost_is_a_registry_without_a_dot() {
        let images = parse_base_images("FROM localhost/img:1\n");
        assert_eq!(images[0].registry.as_deref(), Some("localhost"));
    }

    #[test]
    fn registry_host_is_lowercased() {
        let images = parse_base_images("FROM GHCR.IO/foo/bar:1\n");
        assert_eq!(images[0].registry.as_deref(), Some("ghcr.io"));
    }

    #[test]
    fn skips_scratch() {
        // `scratch` n'est tirée d'aucun registry : rien à pinner, rien à autoriser.
        assert!(raws("FROM scratch\nCOPY app /app\n").is_empty());
    }

    #[test]
    fn skips_stage_aliases() {
        let df = "FROM golang:1.22 AS builder\n\
                  RUN go build\n\
                  FROM builder\n\
                  CMD [\"/app\"]\n";
        assert_eq!(
            raws(df),
            vec!["golang:1.22"],
            "`FROM builder` = alias, pas une image"
        );
    }

    #[test]
    fn stage_aliases_match_case_insensitively() {
        // Docker normalise les noms de stage en minuscules.
        let df = "FROM golang:1.22 AS Builder\nFROM builder\n";
        assert_eq!(raws(df), vec!["golang:1.22"]);
    }

    #[test]
    fn platform_flag_is_ignored() {
        let images = parse_base_images("FROM --platform=linux/amd64 alpine:3.20\n");
        assert_eq!(images[0].raw, "alpine:3.20");
        assert_eq!(images[0].registry.as_deref(), Some(DEFAULT_REGISTRY));
    }

    #[test]
    fn arg_interpolation_is_unresolvable() {
        // `FROM $BASE` dépend d'un ARG évalué au build : impossible de statuer
        // statiquement sur son registry. Signalé comme tel, jamais deviné.
        let images = parse_base_images("ARG BASE=alpine:3.20\nFROM $BASE\n");
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].registry, None);
        assert!(!images[0].is_pinned());

        let braced = parse_base_images("FROM ${BASE}\n");
        assert_eq!(braced[0].registry, None);
    }

    #[test]
    fn ignores_comments_and_accepts_lowercase_from() {
        let df = "# FROM evil.io/malware:latest\n   from alpine:3.20\n";
        assert_eq!(raws(df), vec!["alpine:3.20"]);
    }

    #[test]
    fn follows_line_continuations() {
        let df = "FROM alpine:3.20 \\\n    AS base\nFROM base\n";
        assert_eq!(raws(df), vec!["alpine:3.20"]);
    }

    #[test]
    fn collects_every_distinct_base_image() {
        let df = "FROM golang:1.22 AS builder\n\
                  FROM ghcr.io/foo/runtime@sha256:cafe AS runtime\n\
                  FROM builder\n";
        let images = parse_base_images(df);
        assert_eq!(images.len(), 2);
        assert_eq!(images[1].registry.as_deref(), Some("ghcr.io"));
        assert!(images[1].is_pinned());
    }

    #[test]
    fn empty_dockerfile_yields_nothing() {
        assert!(parse_base_images("").is_empty());
        assert!(parse_base_images("RUN echo hi\n").is_empty());
    }

    // --- Politique (phase 2) ---

    /// Helper : verdict sur un Dockerfile littéral, contre une allowlist explicite.
    fn verdict(df: &str, allowed: &[&str]) -> Result<Vec<String>, AppError> {
        let allowed: Vec<String> = allowed.iter().map(|s| s.to_string()).collect();
        check(&parse_base_images(df), &allowed)
    }

    #[test]
    fn parse_allowed_defaults_to_docker_hub() {
        assert_eq!(parse_allowed(""), vec![DEFAULT_REGISTRY.to_string()]);
        assert_eq!(parse_allowed("  ,  "), vec![DEFAULT_REGISTRY.to_string()]);
    }

    #[test]
    fn parse_allowed_splits_trims_and_lowercases() {
        assert_eq!(
            parse_allowed(" GHCR.io ,docker.io, registry.local:5000 "),
            vec!["ghcr.io", "docker.io", "registry.local:5000"]
        );
    }

    #[test]
    fn allowed_registry_passes_but_moving_tag_warns() {
        let warnings = verdict("FROM alpine:3.20\n", &["docker.io"]).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("alpine:3.20") && warnings[0].contains("non pinnée"),
            "warning inattendu : {}",
            warnings[0]
        );
    }

    #[test]
    fn pinned_image_on_allowed_registry_is_silent() {
        let warnings = verdict("FROM alpine@sha256:deadbeef\n", &["docker.io"]).unwrap();
        assert!(warnings.is_empty(), "rien à signaler : {warnings:?}");
    }

    #[test]
    fn rejects_registry_outside_allowlist() {
        let err = verdict("FROM evil.io/malware:latest\n", &["docker.io"]).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("evil.io/malware:latest"),
            "image non nommée : {msg}"
        );
        assert!(msg.contains("docker.io"), "allowlist non rappelée : {msg}");
    }

    #[test]
    fn rejection_is_a_422_not_a_502() {
        // Faute d'input (le Dockerfile du repo), pas une panne du daemon.
        use axum::response::IntoResponse;
        let err = verdict("FROM evil.io/x:1\n", &["docker.io"]).unwrap_err();
        assert_eq!(
            err.into_response().status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[test]
    fn accepts_registry_present_in_allowlist() {
        let warnings = verdict("FROM ghcr.io/foo/bar:1\n", &["docker.io", "ghcr.io"]).unwrap();
        assert_eq!(warnings.len(), 1, "autorisée, mais non pinnée -> warn");
    }

    #[test]
    fn every_offender_is_named_in_one_error() {
        // Un seul refus listant tout, plutôt qu'un aller-retour par image.
        let df = "FROM evil.io/a:1\nFROM quay.io/b:2 AS s\n";
        let msg = verdict(df, &["docker.io"]).unwrap_err().to_string();
        assert!(
            msg.contains("evil.io/a:1") && msg.contains("quay.io/b:2"),
            "{msg}"
        );
    }

    #[test]
    fn unresolved_reference_warns_instead_of_rejecting() {
        // `FROM $BASE` contourne l'allowlist : signalé, pas refusé (ARG BASE reste légitime).
        let warnings = verdict("ARG BASE\nFROM $BASE\n", &["docker.io"]).unwrap();
        assert_eq!(
            warnings.len(),
            1,
            "un seul warning, pas de doublon : {warnings:?}"
        );
        assert!(warnings[0].contains("ARG"), "{}", warnings[0]);
    }

    #[test]
    fn warns_once_per_unpinned_image() {
        let df = "FROM golang:1.22 AS builder\nFROM alpine:3.20\n";
        assert_eq!(verdict(df, &["docker.io"]).unwrap().len(), 2);
    }

    #[test]
    fn read_base_images_reads_the_dockerfile_from_disk() {
        // Fixture réelle du repo : `FROM alpine:3.20`.
        let images =
            read_base_images(Path::new("tests/fixtures/build-context"), "Dockerfile").unwrap();
        assert_eq!(images.len(), 1);
        assert_eq!(images[0].raw, "alpine:3.20");
    }

    #[test]
    fn read_base_images_missing_dockerfile_is_a_deploy_error() {
        let err = read_base_images(Path::new("tests/fixtures/build-context"), "Nope").unwrap_err();
        assert!(matches!(err, AppError::Deploy(_)), "{err:?}");
    }
}
