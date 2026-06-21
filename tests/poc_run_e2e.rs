//! E2E HUSKER-12 — pilote le binaire compilé `poc_run` (run container réel).
//!
//! Teste la chaîne complète : argv + main + bollard + Docker daemon (create + start +
//! attache network + bind mount + env). Gated `#[ignore]` (nécessite un daemon Docker).
//! Lancer : `cargo test --test poc_run_e2e -- --ignored`
//!
//! Self-contained : on re-tagge une image publique légère sur le tag `husker/*` que le
//! binaire dérive de (project, app, sha) — pas de dépendance au POC build.

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_poc_run");

fn docker(args: &[&str]) -> std::process::Output {
    Command::new("docker")
        .args(args)
        .output()
        .expect("docker introuvable")
}

#[test]
#[ignore = "Docker: run container réel — cargo test -- --ignored"]
fn e2e_run_attaches_network_env_and_mount() {
    let (project, app, sha) = ("e2e", "app", "test");
    let image = "husker/e2e_app:test"; // = image_tag(project, app, sha)
    let container = "husker_e2e_app";
    let network = "husker_e2e";

    // Pré-condition : garantir l'image présente sans dépendre de poc_build — on tire une
    // image publique minuscule et on la re-tagge au format husker/*.
    docker(&["pull", "alpine:3.20"]);
    docker(&["tag", "alpine:3.20", image]);

    // Run via le binaire.
    let out = Command::new(BIN)
        .args([project, app, sha])
        .output()
        .expect("échec du lancement du binaire poc_run");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "exit != 0\n{stdout}\n{stderr}");
    assert!(stdout.contains("running   : true"), "container pas running :\n{stdout}");
    assert!(stdout.contains(network), "network projet absent du rapport :\n{stdout}");

    // Vérifs côté Docker, indépendantes du stdout du binaire.
    let net = docker(&["inspect", "-f", "{{json .NetworkSettings.Networks}}", container]);
    let net = String::from_utf8_lossy(&net.stdout);
    assert!(net.contains(network), "container pas attaché à {network} : {net}");

    let mounts = docker(&["inspect", "-f", "{{json .Mounts}}", container]);
    let mounts = String::from_utf8_lossy(&mounts.stdout);
    assert!(mounts.contains("/data"), "bind mount /data absent : {mounts}");

    let env = docker(&["inspect", "-f", "{{json .Config.Env}}", container]);
    let env = String::from_utf8_lossy(&env.stdout);
    assert!(env.contains("HUSKER_GREETING"), "env var absente : {env}");

    // Idempotence : un second run sur les mêmes (project, app, sha) ne doit pas échouer
    // (le container homonyme est recréé, pas un conflit 409).
    let out2 = Command::new(BIN)
        .args([project, app, sha])
        .output()
        .expect("échec du 2e lancement");
    assert!(
        out2.status.success(),
        "2e run (idempotence) a échoué :\n{}\n{}",
        String::from_utf8_lossy(&out2.stdout),
        String::from_utf8_lossy(&out2.stderr)
    );

    // Cleanup.
    docker(&["rm", "-f", container]);
    docker(&["network", "rm", network]);
    docker(&["rmi", "-f", image]);
}
