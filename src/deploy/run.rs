//! Maillon `run` du pipeline de déploiement (extrait du POC HUSKER-12).
//!
//! Crée + démarre le container applicatif. Différences avec le POC :
//! - le network vient de la DB (`project.network_name`), pas recalculé ;
//! - la CMD vient de `app.run_command` enrobé en `sh -c` (sinon CMD par défaut de l'image,
//!   drop du hack `sleep 3600` du POC) ;
//! - l'image est déjà buildée localement (sortie du maillon build) -> pas de pull.
//!
//! `run_container` supprime l'éventuel container homonyme avant re-création (= stop old).

use crate::errors::AppError;
use bollard::models::{ContainerCreateBody, HostConfig, Mount, MountTypeEnum};
use bollard::query_parameters::{CreateContainerOptionsBuilder, RemoveContainerOptionsBuilder};
use bollard::Docker;
use std::path::{Path, PathBuf};

/// Où le volume data de l'app est bind-monté dans le container.
const CONTAINER_DATA_DIR: &str = "/data";

/// Nom de container déterministe : `husker_{project}_{app}` — clé d'idempotence + nom DNS
/// sur le network du projet.
pub fn container_name(project: &str, app: &str) -> String {
    format!("husker_{project}_{app}")
}

/// Volume data hôte d'une app : `<root>/<projet>/<app>/data`.
/// PathBuf::join, jamais de concaténation de string OS-spécifique (portabilité).
pub fn data_dir(root: &str, project: &str, app: &str) -> PathBuf {
    Path::new(root).join(project).join(app).join("data")
}

/// CMD du container : `run_command` enrobé en `sh -c` si présent, sinon `None`
/// (= CMD par défaut de l'image).
pub fn run_cmd(run_command: Option<&str>) -> Option<Vec<String>> {
    run_command.map(|c| vec!["sh".to_string(), "-c".to_string(), c.to_string()])
}

/// Crée le dossier data puis le canonicalise (le bind mount exige un chemin hôte absolu).
pub fn prepare_data_dir(root: &str, project: &str, app: &str) -> Result<PathBuf, AppError> {
    let dir = data_dir(root, project, app);
    std::fs::create_dir_all(&dir).map_err(|e| AppError::Deploy(format!("create data dir: {e}")))?;
    std::fs::canonicalize(&dir).map_err(|e| AppError::Deploy(format!("canonicalize data dir: {e}")))
}

/// Construit le corps de création du container : image + env + bind mount + network + cmd.
/// Pure (aucun appel daemon) -> testable unitairement.
pub fn build_container_config(
    image: &str,
    env: &[(String, String)],
    network: &str,
    data_host: &Path,
    cmd: Option<Vec<String>>,
) -> ContainerCreateBody {
    let mount = Mount {
        target: Some(CONTAINER_DATA_DIR.to_string()),
        source: Some(data_host.display().to_string()),
        typ: Some(MountTypeEnum::BIND),
        ..Default::default()
    };
    ContainerCreateBody {
        image: Some(image.to_string()),
        cmd,
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        host_config: Some(HostConfig {
            mounts: Some(vec![mount]),
            // network_mode = nom du network -> container attaché dès la création.
            network_mode: Some(network.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Crée puis démarre le container. Idempotent : un container homonyme d'un déploiement
/// précédent est supprimé (force = stop + rm) avant la re-création (= stop old) ;
/// absent = no-op (404 ignoré).
pub async fn run_container(
    docker: &Docker,
    name: &str,
    config: ContainerCreateBody,
) -> Result<(), AppError> {
    let rm = RemoveContainerOptionsBuilder::default().force(true).build();
    let _ = docker.remove_container(name, Some(rm)).await; // 404 si absent -> ignoré

    let opts = CreateContainerOptionsBuilder::default().name(name).build();
    docker.create_container(Some(opts), config).await?;
    docker.start_container(name, None).await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_name_is_deterministic() {
        assert_eq!(container_name("demo", "hello"), "husker_demo_hello");
    }

    #[test]
    fn data_dir_joins_root_project_app() {
        assert_eq!(
            data_dir("data", "demo", "hello"),
            Path::new("data").join("demo").join("hello").join("data")
        );
    }

    #[test]
    fn run_cmd_wraps_in_sh_c() {
        assert_eq!(
            run_cmd(Some("node server.js | tee log")),
            Some(vec![
                "sh".to_string(),
                "-c".to_string(),
                "node server.js | tee log".to_string()
            ])
        );
    }

    #[test]
    fn run_cmd_none_keeps_image_default() {
        assert_eq!(run_cmd(None), None);
    }

    #[test]
    fn config_injects_image_env_network_mount_and_cmd() {
        let env = vec![("KEY".to_string(), "VAL".to_string())];
        let host = Path::new("/srv/data/demo/hello/data");
        let cfg = build_container_config(
            "husker/demo_hello:dev",
            &env,
            "husker_demo",
            host,
            run_cmd(Some("run")),
        );

        assert_eq!(cfg.image.as_deref(), Some("husker/demo_hello:dev"));
        assert_eq!(cfg.env.as_ref().unwrap(), &vec!["KEY=VAL".to_string()]);
        assert_eq!(
            cfg.cmd.as_ref().unwrap(),
            &vec!["sh".to_string(), "-c".to_string(), "run".to_string()]
        );

        let hc = cfg.host_config.expect("host_config présent");
        assert_eq!(hc.network_mode.as_deref(), Some("husker_demo"));

        let mounts = hc.mounts.expect("mounts présents");
        let mount = &mounts[0];
        assert_eq!(mount.typ, Some(MountTypeEnum::BIND));
        assert_eq!(mount.target.as_deref(), Some(CONTAINER_DATA_DIR));
        assert_eq!(mount.source.as_deref(), Some("/srv/data/demo/hello/data"));
    }

    #[test]
    fn config_without_run_command_has_no_cmd() {
        let cfg = build_container_config(
            "husker/demo_hello:dev",
            &[],
            "husker_demo",
            Path::new("/d"),
            run_cmd(None),
        );
        assert!(cfg.cmd.is_none(), "CMD par défaut de l'image conservée");
    }
}
