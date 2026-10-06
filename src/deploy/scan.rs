//! Maillon `scan` du pipeline de déploiement (HUSKER-16, M4).
//!
//! Scan CVE de l'image buildée via **Trivy**, en container — jamais le binaire hôte
//! (promesse « un binaire + Docker »), jamais le socket Docker monté dans un tiers (un
//! outil de sécu qui ouvre un trou root sur l'hôte). Place retenue (ADR-020) :
//! `export_image` (= `docker save`) vers un tar, puis Trivy scanne ce tar en bind mount.
//!
//! **Reporting-first** : aucun chemin de cette fonction ne fait jamais échouer un deploy.
//! Un Trivy indisponible, un JSON illisible, ou des CVE trouvées deviennent chacun un
//! warning (même contrat que [`super::digests::track`]), persisté par l'appelant en
//! `deployment_signals` (kind = `"cve"`).
//!
//! Granularité volontairement minimale pour cette première version : un seul message
//! résumant le décompte par sévérité, pas le dump des CVE individuelles (le dump complet
//! serait ignoré dès le 2e deploy — cf. notes du ticket). Le delta deploy-à-deploy
//! (« est-ce pire que la dernière fois ? ») est laissé à une itération ultérieure,
//! explicitement ouverte dans le ticket.

use crate::errors::AppError;
use bollard::models::{ContainerCreateBody, HostConfig, Mount, MountTypeEnum};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use serde::Deserialize;
use std::path::Path;

/// Image Trivy, pinnée par tag précis — contrairement aux images des apps déployées,
/// celle de l'outil de sécu lui-même ne doit pas bouger sous nos pieds sans qu'on l'ait
/// choisi.
const TRIVY_IMAGE: &str = "aquasec/trivy:0.56.2";
/// Volume nommé : cache de la DB de vulnérabilités Trivy (~50 Mo), partagé entre deploys —
/// sans lui, retéléchargé à chaque scan.
const TRIVY_CACHE_VOLUME: &str = "husker_trivy_cache";

#[derive(Debug, Default, Deserialize)]
struct TrivyReport {
    #[serde(rename = "Results", default)]
    results: Vec<TrivyResult>,
}

#[derive(Debug, Default, Deserialize)]
struct TrivyResult {
    #[serde(rename = "Vulnerabilities", default)]
    vulnerabilities: Vec<TrivyVulnerability>,
}

#[derive(Debug, Deserialize)]
struct TrivyVulnerability {
    #[serde(rename = "Severity")]
    severity: String,
}

/// Sévérités Trivy, de la plus grave à la moins grave — ordre d'affichage du résumé.
const SEVERITY_ORDER: [&str; 4] = ["CRITICAL", "HIGH", "MEDIUM", "LOW"];

/// Décompte les CVE par sévérité dans un rapport JSON Trivy (`trivy image --format json`).
/// Pure, testable sans Docker. Les sévérités hors [`SEVERITY_ORDER`] (`UNKNOWN`, ajout futur
/// de Trivy) sont conservées, triées, en fin de liste plutôt que perdues.
pub fn count_by_severity(json: &str) -> Result<Vec<(String, usize)>, AppError> {
    let report: TrivyReport =
        serde_json::from_str(json).map_err(|e| AppError::Deploy(format!("trivy JSON: {e}")))?;

    let mut counts: std::collections::BTreeMap<String, usize> = std::collections::BTreeMap::new();
    for result in &report.results {
        for vuln in &result.vulnerabilities {
            *counts.entry(vuln.severity.clone()).or_insert(0) += 1;
        }
    }

    let mut ordered: Vec<(String, usize)> = SEVERITY_ORDER
        .iter()
        .filter_map(|s| counts.remove(*s).map(|n| (s.to_string(), n)))
        .collect();
    ordered.extend(counts); // sévérités inconnues restantes, déjà triées par BTreeMap
    Ok(ordered)
}

/// Résume un décompte en un unique message actionnable. `None` si aucune CVE — pas de
/// signal à émettre, pas de bruit pour un scan propre.
pub fn summarize(counts: &[(String, usize)]) -> Option<String> {
    let total: usize = counts.iter().map(|(_, n)| n).sum();
    if total == 0 {
        return None;
    }
    let detail: Vec<String> = counts
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(sev, n)| format!("{n} {sev}"))
        .collect();
    Some(format!("{total} CVE connues ({})", detail.join(", ")))
}

/// Scanne l'image buildée et renvoie les warnings à persister (0 ou 1, aujourd'hui). Ne
/// renvoie jamais d'`Err` : un Trivy indisponible ou un rapport illisible devient un warning
/// explicite plutôt qu'un deploy bloqué — reporting-first jusqu'au bout.
pub async fn scan(docker: &Docker, image_ref: &str) -> Vec<String> {
    let json = match scan_image(docker, image_ref).await {
        Ok(json) => json,
        Err(e) => return vec![format!("trivy : scan indisponible ({e})")],
    };

    match count_by_severity(&json) {
        Ok(counts) => summarize(&counts)
            .map(|s| format!("trivy : {s}"))
            .into_iter()
            .collect(),
        Err(e) => vec![format!("trivy : rapport illisible ({e})")],
    }
}

/// Exporte l'image (`docker save` via l'API, jamais le socket monté dans Trivy), lance
/// Trivy en container sur ce tar, renvoie le rapport JSON brut.
async fn scan_image(docker: &Docker, image_ref: &str) -> Result<String, AppError> {
    let tmp_dir = std::env::temp_dir().join(format!("husker-scan-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&tmp_dir)
        .map_err(|e| AppError::Deploy(format!("scan tmp dir: {e}")))?;
    let tar_path = tmp_dir.join("image.tar");

    let result = async {
        export_image_to_file(docker, image_ref, &tar_path).await?;
        ensure_trivy_image(docker).await?;
        run_trivy_container(docker, &tar_path).await
    }
    .await;

    let _ = std::fs::remove_dir_all(&tmp_dir); // best-effort : pas d'espace disque qui traîne
    result
}

/// `docker save` côté API : exporte l'image entière vers un fichier tar local, chunk par
/// chunk (pas de double-bufferisation en mémoire — une image applicative peut peser
/// plusieurs centaines de Mo).
async fn export_image_to_file(
    docker: &Docker,
    image_ref: &str,
    dest: &Path,
) -> Result<(), AppError> {
    let mut file = std::fs::File::create(dest)
        .map_err(|e| AppError::Deploy(format!("scan tar create: {e}")))?;
    let mut stream = docker.export_image(image_ref);
    while let Some(chunk) = stream.next().await {
        std::io::Write::write_all(&mut file, &chunk?)
            .map_err(|e| AppError::Deploy(format!("scan tar write: {e}")))?;
    }
    Ok(())
}

/// `docker pull` de l'image Trivy, seulement si elle n'est pas déjà présente localement —
/// elle est pinnée par tag exact (cf. [`TRIVY_IMAGE`]) et ne bouge jamais entre deux deploys,
/// inutile de payer un aller-retour registry à chaque fois.
async fn ensure_trivy_image(docker: &Docker) -> Result<(), AppError> {
    if docker.inspect_image(TRIVY_IMAGE).await.is_ok() {
        return Ok(());
    }
    let opts = CreateImageOptionsBuilder::default()
        .from_image(TRIVY_IMAGE)
        .build();
    let mut stream = docker.create_image(Some(opts), None, None);
    while let Some(item) = stream.next().await {
        item?;
    }
    Ok(())
}

/// Lance Trivy sur le tar exporté (bind mount RO) avec cache DB en volume nommé persistant,
/// attend sa fin, récupère stdout (= JSON), nettoie le container quoi qu'il arrive.
async fn run_trivy_container(docker: &Docker, tar_path: &Path) -> Result<String, AppError> {
    let name = format!("husker-scan-{}", uuid::Uuid::new_v4());

    let tar_mount = Mount {
        target: Some("/image.tar".to_string()),
        source: Some(tar_path.display().to_string()),
        typ: Some(MountTypeEnum::BIND),
        read_only: Some(true),
        ..Default::default()
    };
    let cache_mount = Mount {
        target: Some("/root/.cache/trivy".to_string()),
        source: Some(TRIVY_CACHE_VOLUME.to_string()),
        typ: Some(MountTypeEnum::VOLUME),
        ..Default::default()
    };

    let config = ContainerCreateBody {
        image: Some(TRIVY_IMAGE.to_string()),
        cmd: Some(
            [
                "image",
                "--input",
                "/image.tar",
                "--format",
                "json",
                "--quiet",
            ]
            .iter()
            .map(|s| s.to_string())
            .collect(),
        ),
        host_config: Some(HostConfig {
            mounts: Some(vec![tar_mount, cache_mount]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let opts = CreateContainerOptionsBuilder::default().name(&name).build();
    docker.create_container(Some(opts), config).await?;
    docker.start_container(&name, None).await?;

    // Avance jusqu'à l'arrêt du container ; le code de sortie de Trivy n'importe pas (un
    // scan "vulnérabilités trouvées" sort non-zéro), mais une `Err` du stream (container tué,
    // erreur de transport) doit interrompre avant de lire des logs qui n'ont plus de sens.
    let mut wait = docker.wait_container(&name, None::<WaitContainerOptions>);
    while let Some(item) = wait.next().await {
        if let Err(e) = item {
            let rm = RemoveContainerOptionsBuilder::default().force(true).build();
            let _ = docker.remove_container(&name, Some(rm)).await;
            return Err(e.into());
        }
    }

    let logs_opts = LogsOptionsBuilder::default().stdout(true).build();
    let mut logs = docker.logs(&name, Some(logs_opts));
    let mut output = String::new();
    while let Some(chunk) = logs.next().await {
        output.push_str(&chunk?.to_string());
    }

    let rm = RemoveContainerOptionsBuilder::default().force(true).build();
    let _ = docker.remove_container(&name, Some(rm)).await;

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NO_VULN: &str = r#"{"Results":[{"Target":"img","Vulnerabilities":[]}]}"#;
    const MIXED: &str = r#"{
        "Results": [
            {"Target": "a", "Vulnerabilities": [
                {"Severity": "HIGH"},
                {"Severity": "CRITICAL"},
                {"Severity": "HIGH"}
            ]},
            {"Target": "b", "Vulnerabilities": [
                {"Severity": "LOW"}
            ]}
        ]
    }"#;
    const NO_RESULTS_KEY: &str = r#"{}"#;
    const RESULT_WITHOUT_VULN_KEY: &str = r#"{"Results":[{"Target":"img"}]}"#;

    #[test]
    fn no_vulnerabilities_counts_to_nothing() {
        assert_eq!(count_by_severity(NO_VULN).unwrap(), vec![]);
    }

    #[test]
    fn missing_results_key_counts_to_nothing() {
        assert_eq!(count_by_severity(NO_RESULTS_KEY).unwrap(), vec![]);
    }

    #[test]
    fn result_without_vulnerabilities_key_counts_to_nothing() {
        assert_eq!(count_by_severity(RESULT_WITHOUT_VULN_KEY).unwrap(), vec![]);
    }

    #[test]
    fn counts_are_grouped_across_results_and_ordered_by_severity() {
        assert_eq!(
            count_by_severity(MIXED).unwrap(),
            vec![
                ("CRITICAL".to_string(), 1),
                ("HIGH".to_string(), 2),
                ("LOW".to_string(), 1),
            ]
        );
    }

    #[test]
    fn unknown_severity_is_kept_after_the_known_ones() {
        let json =
            r#"{"Results":[{"Vulnerabilities":[{"Severity":"UNKNOWN"},{"Severity":"HIGH"}]}]}"#;
        assert_eq!(
            count_by_severity(json).unwrap(),
            vec![("HIGH".to_string(), 1), ("UNKNOWN".to_string(), 1)]
        );
    }

    #[test]
    fn invalid_json_is_an_error_not_a_panic() {
        assert!(count_by_severity("pas du json").is_err());
    }

    #[test]
    fn summarize_empty_counts_is_none() {
        assert_eq!(summarize(&[]), None);
    }

    #[test]
    fn summarize_all_zero_is_none() {
        assert_eq!(summarize(&[("HIGH".to_string(), 0)]), None);
    }

    #[test]
    fn summarize_formats_total_and_detail() {
        let counts = vec![("CRITICAL".to_string(), 1), ("HIGH".to_string(), 2)];
        assert_eq!(
            summarize(&counts),
            Some("3 CVE connues (1 CRITICAL, 2 HIGH)".to_string())
        );
    }

    #[tokio::test]
    #[ignore = "Docker réel : export + Trivy sur une petite image (cargo test -- --ignored)"]
    async fn scan_a_real_small_image_returns_a_readable_report() {
        let docker = Docker::connect_with_local_defaults().unwrap();
        // `scratch`-based n'existe pas en tag facile ; `alpine` est petit et quasi toujours
        // présent localement dans ce repo (autres tests l'utilisent déjà).
        let image = "alpine:3.20";
        let mut pull = docker.create_image(
            Some(
                CreateImageOptionsBuilder::default()
                    .from_image(image)
                    .build(),
            ),
            None,
            None,
        );
        while let Some(item) = pull.next().await {
            item.unwrap();
        }

        let json = scan_image(&docker, image).await.expect("scan doit réussir");
        let counts = count_by_severity(&json).expect("rapport JSON lisible");
        // Pas d'assertion sur le nombre de CVE (ça bouge avec la DB Trivy) : seule la
        // capacité à produire un rapport structuré est testée ici.
        assert!(
            counts.iter().all(|(_, n)| *n > 0),
            "pas d'entrée à 0 : {counts:?}"
        );
    }
}
