//! E2E HUSKER-11 — pilote le binaire compilé `poc_build` (build BuildKit réel).
//!
//! Teste la chaîne complète : argv + main + bollard/BuildKit + Docker daemon.
//! Gated `#[ignore]` (nécessite un daemon Docker + pull alpine).
//! Lancer : `cargo test --test poc_build_e2e -- --ignored`

use std::process::Command;

const BIN: &str = env!("CARGO_BIN_EXE_poc_build");

fn run(args: &[&str]) -> std::process::Output {
    Command::new(BIN)
        .args(args)
        .output()
        .expect("échec du lancement du binaire poc_build")
}

#[test]
#[ignore = "Docker: build BuildKit réel — cargo test -- --ignored"]
fn e2e_build_success_via_binary() {
    let tag = "husker/e2e_app:e2etest";
    let _ = Command::new("docker").args(["rmi", "-f", tag]).output();

    let out = run(&["e2e", "app", "e2etest", "tests/fixtures/build-context"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "exit != 0\n{stdout}\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("✓ image"), "pas de confirmation de build :\n{stdout}");

    // L'image est-elle réellement taggée côté Docker ?
    let imgs = Command::new("docker")
        .args(["images", "-q", tag])
        .output()
        .unwrap();
    assert!(
        !imgs.stdout.is_empty(),
        "image {tag} absente de `docker images`"
    );

    let _ = Command::new("docker").args(["rmi", "-f", tag]).output();
}

#[test]
#[ignore = "Docker: build BuildKit réel — cargo test -- --ignored"]
fn e2e_broken_dockerfile_fails_without_panic() {
    let out = run(&["demo", "broken", "dev", "tests/fixtures/build-context-broken"]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        !out.status.success(),
        "le build cassé aurait dû échouer (exit ≠ 0) :\n{combined}"
    );
    assert!(
        !combined.contains("panicked"),
        "panic au lieu d'une erreur propre :\n{combined}"
    );
    assert!(
        combined.contains("Error") || combined.contains("build échoué"),
        "pas de message d'erreur lisible :\n{combined}"
    );
}
