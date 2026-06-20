//! E2E HUSKER-10 — pilote le binaire compilé `poc_git` comme un vrai process.
//!
//! Différent des tests du module dans `src/bin/poc_git.rs` (qui appellent les
//! fonctions internes) : ici on teste la chaîne COMPLÈTE — argv + variable d'env
//! `HUSKER_SOURCES_ROOT` + `main` + git2 + filesystem — sur l'artefact réel.
//!
//! Le chemin du binaire compilé est injecté par cargo via cette variable.
//! Lancer : `cargo test --test poc_git_e2e`           (e2e local, offline)
//!          `cargo test --test poc_git_e2e -- --ignored`  (+ e2e réseau GitHub)

use git2::{Repository, Signature};
use std::path::{Path, PathBuf};
use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_poc_git");

/// Répertoire temporaire unique (uuid pour l'isolation, cf. ADR-004).
fn tmp() -> PathBuf {
    let p = std::env::temp_dir().join(format!("husker-e2e-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&p).unwrap();
    p
}

/// Crée un commit qui écrit `file`=`content` dans le working tree de `repo`.
/// (Dupliqué du module de test du bin : helpers privés non partageables entre crates.)
fn commit_file(repo: &Repository, file: &str, content: &str, msg: &str) {
    std::fs::write(repo.workdir().unwrap().join(file), content).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(file)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let sig = Signature::now("test", "test@husker").unwrap();
    let parents = match repo.head() {
        Ok(h) => vec![h.peel_to_commit().unwrap()],
        Err(_) => vec![],
    };
    let parent_refs: Vec<&_> = parents.iter().collect();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)
        .unwrap();
}

/// Lance le binaire `poc_git url app_id` avec `HUSKER_SOURCES_ROOT=sources_root`.
/// Renvoie (stdout, succès du process).
fn run(sources_root: &Path, url: &str, app_id: &str) -> (String, bool) {
    let out = Command::new(BIN)
        .args([url, app_id])
        .env("HUSKER_SOURCES_ROOT", sources_root)
        .output()
        .expect("échec du lancement du binaire poc_git");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.success(),
    )
}

#[test]
fn e2e_local_clone_then_pull_via_binary() {
    let root = tmp();
    let remote_dir = root.join("remote");
    let remote = Repository::init(&remote_dir).unwrap();
    commit_file(&remote, "README.md", "v1", "initial");
    let url = remote_dir.to_str().unwrap();
    let sources = root.join("sources");

    // 1) Premier run -> clone, working tree peuplé.
    let (out1, ok1) = run(&sources, url, "myapp");
    assert!(ok1, "run 1 doit réussir.\n{out1}");
    assert!(out1.contains("clone"), "run 1 doit cloner.\n{out1}");
    assert!(out1.contains("HEAD sha"), "run 1 doit afficher le sha.\n{out1}");
    assert!(sources.join("myapp").join("README.md").exists());

    // Le remote avance d'un commit.
    commit_file(&remote, "README.md", "v2", "second");

    // 2) Deuxième run même app_id -> pull fast-forward (pas de re-clone).
    let (out2, ok2) = run(&sources, url, "myapp");
    assert!(ok2, "run 2 doit réussir.\n{out2}");
    assert!(
        out2.contains("repo existant, pull"),
        "run 2 doit puller, pas re-cloner.\n{out2}"
    );
    assert!(
        out2.contains("fast-forward"),
        "run 2 doit fast-forward.\n{out2}"
    );
    assert_eq!(
        std::fs::read_to_string(sources.join("myapp").join("README.md")).unwrap(),
        "v2",
        "le working tree reflète le nouveau commit"
    );

    let _ = std::fs::remove_dir_all(&root);
}

#[test]
#[ignore = "réseau: clone réel depuis GitHub — lancer avec `cargo test -- --ignored`"]
fn e2e_network_clone_then_pull_github() {
    let root = tmp();
    let sources = root.join("sources");
    let url = "https://github.com/octocat/Hello-World.git";

    // 1) Clone réel depuis GitHub.
    let (out1, ok1) = run(&sources, url, "hello");
    assert!(ok1, "run 1 doit réussir.\n{out1}");
    assert!(out1.contains("clone"), "run 1 doit cloner.\n{out1}");
    assert!(out1.contains("HEAD sha"), "run 1 doit afficher le sha.\n{out1}");

    // 2) Repo public stable -> pull, déjà à jour (et surtout : pas de re-clone).
    let (out2, ok2) = run(&sources, url, "hello");
    assert!(ok2, "run 2 doit réussir.\n{out2}");
    assert!(
        out2.contains("repo existant, pull"),
        "run 2 doit puller, pas re-cloner.\n{out2}"
    );

    let _ = std::fs::remove_dir_all(&root);
}
