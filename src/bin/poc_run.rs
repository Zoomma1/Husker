//! POC HUSKER-12 — run d'un container applicatif via `bollard`.
//!
//! 3e et dernier maillon du pipeline M3 (`git → build → run`). Binaire à part pour
//! défricher l'API container de `bollard` (create + start + inspect) avant intégration
//! (HUSKER-13). Couvre les 3 spécificités du run Husker :
//!   1. attache au **network Docker du projet** (`husker_<projet>`, cf. ADR-006) ;
//!   2. **injection d'env vars** dans le container ;
//!   3. **bind mount** d'un volume data persistant (`<root>/<projet>/<app>/data`).
//!
//! Choix d'implémentation :
//!   - on consomme une image **déjà buildée** (`husker/{project}_{app}:{sha}`, sortie de
//!     HUSKER-11) ; le build est hors scope ici ;
//!   - bind mount via `HostConfig.mounts` (struct `Mount`) plutôt que `binds`
//!     ("src:dst") : pas de découpage sur `:` qui casserait sur un chemin Windows
//!     (`C:\...`) — le projet vise la portabilité Windows <-> Linux ;
//!   - commande forcée à `sleep` : l'image de démo (HUSKER-11) sort tout de suite
//!     (`cat /husker.txt`), on la garde vivante pour démontrer la joignabilité réseau.
//!     En prod, la CMD long-running vient de l'image applicative.
//!
//! Usage : cargo run --bin poc_run -- [project] [app] [sha]
//!   defaults : demo hello dev  -> image husker/demo_hello:dev (buildée par poc_build)
//!   - 1er run            -> create + start
//!   - run suivant idem   -> remove du container précédent puis re-create (idempotent)

use bollard::models::{ContainerCreateBody, HostConfig, Mount, MountTypeEnum, NetworkCreateRequest};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, RemoveContainerOptionsBuilder,
};
use bollard::Docker;
use futures_util::StreamExt;
use std::error::Error;
use std::path::{Path, PathBuf};

/// Où le volume data de l'app est bind-monté dans le container.
const CONTAINER_DATA_DIR: &str = "/data";

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // --- Entrées (POC : via argv, avec defaults pour un run qui marche tout seul) ---
    let mut args = std::env::args().skip(1);
    let project = args.next().unwrap_or_else(|| "demo".to_string());
    let app = args.next().unwrap_or_else(|| "hello".to_string());
    let sha = args.next().unwrap_or_else(|| "dev".to_string());

    // Racine des données : variable d'env (portable Windows <-> Linux), defaut ./data.
    let root = std::env::var("HUSKER_DATA_ROOT").unwrap_or_else(|_| "data".to_string());

    let tag = image_tag(&project, &app, &sha);
    let network = network_name(&project);
    let name = container_name(&project, &app);
    let data_dir = data_dir(&root, &project, &app);

    // Le bind mount exige un chemin hôte **absolu** : on crée le dossier puis on le
    // canonicalise (sinon Docker refuse un chemin relatif).
    std::fs::create_dir_all(&data_dir)?;
    let data_abs = std::fs::canonicalize(&data_dir)?;

    // Env vars injectées : jeu de démo au POC ; viendront de la table `env_vars` en HUSKER-13.
    let env = vec![("HUSKER_GREETING".to_string(), "hello from husker".to_string())];

    println!("image     : {tag}");
    println!("network   : {network}");
    println!("container : {name}");
    println!("data mount: {} -> {CONTAINER_DATA_DIR}", data_abs.display());

    let config = build_container_config(&tag, &env, &network, &data_abs, keep_alive_cmd());

    let docker = Docker::connect_with_local_defaults()?;
    ensure_image(&docker, &tag).await?;
    ensure_network(&docker, &network).await?;
    run_container(&docker, &name, config).await?;
    report(&docker, &name).await?;

    println!("✓ container {name} démarré sur {network}");
    Ok(())
}

// --- Fonctions pures (testées offline) ---

/// Tag d'image d'app : `husker/{project}_{app}:{sha}` — même convention que HUSKER-11.
fn image_tag(project: &str, app: &str, sha: &str) -> String {
    format!("husker/{project}_{app}:{sha}")
}

/// Network d'un projet : `husker_<projet>` — préfixe serveur, cf. ADR-006.
fn network_name(project: &str) -> String {
    format!("husker_{project}")
}

/// Nom de container déterministe : `husker_{project}_{app}` — sert de clé d'idempotence.
fn container_name(project: &str, app: &str) -> String {
    format!("husker_{project}_{app}")
}

/// Volume data hôte d'une app : `<root>/<projet>/<app>/data`.
/// PathBuf::join, jamais de concaténation de string OS-spécifique (portabilité).
fn data_dir(root: &str, project: &str, app: &str) -> PathBuf {
    Path::new(root).join(project).join(app).join("data")
}

/// Commande keep-alive imposée au POC (cf. doc d'en-tête).
fn keep_alive_cmd() -> Vec<String> {
    vec!["sleep".to_string(), "3600".to_string()]
}

/// Construit le corps de création du container : image + env + bind mount + network.
/// Pure (aucun appel daemon) -> testable unitairement.
fn build_container_config(
    image: &str,
    env: &[(String, String)],
    network: &str,
    data_host: &Path,
    cmd: Vec<String>,
) -> ContainerCreateBody {
    let mount = Mount {
        target: Some(CONTAINER_DATA_DIR.to_string()),
        source: Some(data_host.display().to_string()),
        typ: Some(MountTypeEnum::BIND),
        ..Default::default()
    };
    ContainerCreateBody {
        image: Some(image.to_string()),
        cmd: Some(cmd),
        env: Some(env.iter().map(|(k, v)| format!("{k}={v}")).collect()),
        host_config: Some(HostConfig {
            mounts: Some(vec![mount]),
            // network_mode = nom du network -> le container y est attaché dès la création.
            network_mode: Some(network.to_string()),
            ..Default::default()
        }),
        ..Default::default()
    }
}

// --- Fonctions daemon (nécessitent Docker) ---

/// S'assure que l'image est présente localement : sinon, pull. Une image `husker/*`
/// absente n'existe dans aucun registre -> l'erreur de pull guide vers `poc_build`.
async fn ensure_image(docker: &Docker, tag: &str) -> Result<(), Box<dyn Error>> {
    if docker.inspect_image(tag).await.is_ok() {
        println!("  image présente");
        return Ok(());
    }
    println!("  image absente -> pull");
    // /images/create veut image et tag séparés.
    let (img, tg) = match tag.rsplit_once(':') {
        Some((i, t)) => (i, Some(t)),
        None => (tag, None),
    };
    let mut builder = CreateImageOptionsBuilder::default().from_image(img);
    if let Some(t) = tg {
        builder = builder.tag(t);
    }
    let mut stream = docker.create_image(Some(builder.build()), None, None);
    while let Some(item) = stream.next().await {
        item?; // une erreur de pull (image husker/* introuvable, réseau) remonte ici
    }
    Ok(())
}

/// S'assure que le network projet existe (en prod il est créé par le CRUD projects ;
/// pour ce POC standalone on le crée si besoin). Idempotent : inspect puis create.
async fn ensure_network(docker: &Docker, network: &str) -> Result<(), Box<dyn Error>> {
    if docker.inspect_network(network, None).await.is_ok() {
        println!("  network présent");
        return Ok(());
    }
    println!("  network absent -> create");
    docker
        .create_network(NetworkCreateRequest {
            name: network.to_string(),
            ..Default::default()
        })
        .await?;
    Ok(())
}

/// Crée puis démarre le container. Idempotent : un container homonyme d'un run
/// précédent est supprimé (force = stop + rm) avant la re-création ; absent = no-op.
async fn run_container(
    docker: &Docker,
    name: &str,
    config: ContainerCreateBody,
) -> Result<(), Box<dyn Error>> {
    let rm = RemoveContainerOptionsBuilder::default().force(true).build();
    let _ = docker.remove_container(name, Some(rm)).await; // 404 si absent -> ignoré

    let opts = CreateContainerOptionsBuilder::default().name(name).build();
    docker.create_container(Some(opts), config).await?;
    docker.start_container(name, None).await?;
    Ok(())
}

/// Inspecte le container démarré et affiche la preuve des 3 specs : running, networks
/// attachés, env vars injectées. Lecture seule.
async fn report(docker: &Docker, name: &str) -> Result<(), Box<dyn Error>> {
    let info = docker.inspect_container(name, None).await?;

    let running = info
        .state
        .and_then(|s| s.running)
        .unwrap_or(false);
    println!("  running   : {running}");

    if let Some(networks) = info.network_settings.and_then(|n| n.networks) {
        let names: Vec<&String> = networks.keys().collect();
        println!("  networks  : {names:?}");
    }
    if let Some(env) = info.config.and_then(|c| c.env) {
        println!("  env       : {env:?}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_tag_format() {
        assert_eq!(image_tag("demo", "hello", "abc123"), "husker/demo_hello:abc123");
    }

    #[test]
    fn network_name_is_prefixed() {
        // ADR-006 : préfixe serveur `husker_<projet>`.
        assert_eq!(network_name("demo"), "husker_demo");
    }

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
    fn config_injects_image_env_network_and_mount() {
        let env = vec![("KEY".to_string(), "VAL".to_string())];
        let host = Path::new("/srv/data/demo/hello/data");
        let cfg = build_container_config("husker/demo_hello:dev", &env, "husker_demo", host, keep_alive_cmd());

        assert_eq!(cfg.image.as_deref(), Some("husker/demo_hello:dev"));
        assert_eq!(cfg.env.as_ref().unwrap(), &vec!["KEY=VAL".to_string()]);

        let hc = cfg.host_config.expect("host_config présent");
        assert_eq!(hc.network_mode.as_deref(), Some("husker_demo"));

        let mounts = hc.mounts.expect("mounts présents");
        let mount = &mounts[0];
        assert_eq!(mount.typ, Some(MountTypeEnum::BIND));
        assert_eq!(mount.target.as_deref(), Some(CONTAINER_DATA_DIR));
        assert_eq!(mount.source.as_deref(), Some("/srv/data/demo/hello/data"));
    }
}
