//! POC HUSKER-10 — clone + pull idempotent d'un repo GitHub public via git2.
//!
//! But : défricher le maillon `git` du pipeline de déploiement M3 (git → build → run)
//! dans un binaire à part, sans polluer le code de prod. On a choisi git2 (bindings
//! libgit2) plutôt que gix : libgit2 expose un checkout/fast-forward haut niveau qui
//! marche, là où gix laisse la mise à jour du working tree à la main.
//!
//! Usage : cargo run --bin poc_git -- [git_url] [app_id]
//!   - 1er run sur un app_id neuf      -> clone
//!   - run suivant sur le même app_id  -> fast-forward pull (pas de re-clone)

use git2::{
    build::CheckoutBuilder, AnnotatedCommit, AutotagOption, FetchOptions, Reference, Repository,
};
use std::path::{Path, PathBuf};

fn main() -> Result<(), git2::Error> {
    // --- Entrées (POC : via argv, avec defaults pour un run qui marche tout seul) ---
    let mut args = std::env::args().skip(1);
    let url = args
        .next()
        .unwrap_or_else(|| "https://github.com/octocat/Hello-World.git".to_string());
    let app_id = args.next().unwrap_or_else(|| "hello-world".to_string());

    // Racine des sources : variable d'env (portable Windows <-> Linux), defaut ./sources.
    let root = std::env::var("HUSKER_SOURCES_ROOT").unwrap_or_else(|_| "sources".to_string());
    let dest = dest_path(&root, &app_id);

    println!("URL      : {url}");
    println!("app_id   : {app_id}");
    println!("dest     : {}", dest.display());

    let repo = clone_or_update(&url, &dest)?;

    // sha du commit checkout : servira de tag d'image en HUSKER-11/13.
    let commit = repo.head()?.peel_to_commit()?;
    println!("HEAD sha : {}", commit.id());
    Ok(())
}

/// Chemin de destination d'un repo cloné : `<root>/<app_id>`.
/// PathBuf::join, jamais de concaténation de string OS-spécifique (portabilité).
fn dest_path(root: &str, app_id: &str) -> PathBuf {
    Path::new(root).join(app_id)
}

/// Idempotence : si `dest` est déjà un repo git valide -> pull, sinon -> clone.
/// On délègue à libgit2 le « est-ce un repo valide ? » (plus robuste qu'un test
/// d'existence de dossier : un dossier vide ou cassé tromperait `dest.exists()`).
fn clone_or_update(url: &str, dest: &Path) -> Result<Repository, git2::Error> {
    match Repository::open(dest) {
        Ok(repo) => {
            println!("-> repo existant, pull");
            pull(&repo)?;
            Ok(repo)
        }
        Err(_) => {
            println!("-> pas de repo, clone");
            Repository::clone(url, dest)
        }
    }
}

/// Pull fast-forward-only de la branche courante depuis `origin`.
/// Décomposé en deux temps libgit2 : fetch (réseau) puis fast-forward (local).
fn pull(repo: &Repository) -> Result<(), git2::Error> {
    // Branche courante (ex: "main" / "master") — on pull la même.
    // .to_string() pour relâcher l'emprunt sur `head` avant la suite.
    let branch = repo.head()?.shorthand().unwrap_or("HEAD").to_string();

    // 1) Fetch depuis origin (réseau). Ne touche ni le working tree ni la branche
    //    locale : remplit la base d'objets et déplace FETCH_HEAD.
    let mut remote = repo.find_remote("origin")?;
    let mut fo = FetchOptions::new();
    fo.download_tags(AutotagOption::All);
    remote.fetch(&[&branch], Some(&mut fo), None)?;

    // Le commit fraîchement fetché est pointé par FETCH_HEAD.
    let fetch_head = repo.find_reference("FETCH_HEAD")?;
    let fetched = repo.reference_to_annotated_commit(&fetch_head)?;

    // 2) Analyse : déjà à jour ? fast-forward possible ? sinon hors scope du POC.
    let (analysis, _) = repo.merge_analysis(&[&fetched])?;
    if analysis.is_up_to_date() {
        println!("   déjà à jour");
        Ok(())
    } else if analysis.is_fast_forward() {
        println!("   fast-forward vers {}", fetched.id());
        fast_forward(repo, &branch, &fetched)
    } else {
        // Divergence / merge nécessaire : explicitement hors scope (cf. ticket).
        Err(git2::Error::from_str(
            "pull non fast-forward (divergence) — hors scope POC",
        ))
    }
}

/// Déplace la ref locale de branche sur le commit fetché, puis aligne le working tree.
fn fast_forward(
    repo: &Repository,
    branch: &str,
    target: &AnnotatedCommit,
) -> Result<(), git2::Error> {
    let refname = format!("refs/heads/{branch}");
    let mut reference: Reference = repo.find_reference(&refname)?;
    reference.set_target(target.id(), "husker poc: fast-forward")?; // (a) déplace la branche
    repo.set_head(&refname)?; // (b) HEAD suit la branche
                              // (c) seul moment où les fichiers du working tree changent.
                              // force() : on aligne sur la ref (sources/<app_id> jetable,
                              // aucune modif locale à préserver).
    repo.checkout_head(Some(CheckoutBuilder::default().force()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Oid, Signature};
    use std::fs;

    /// Répertoire temporaire unique, nettoyé à la fin (RAII, robuste au panic).
    /// uuid pour l'isolation, cohérent avec la convention de test du projet (ADR-004).
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("husker-poc-{}", uuid::Uuid::new_v4()));
            fs::create_dir_all(&p).unwrap();
            TmpDir(p)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for TmpDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// Crée un commit qui écrit `file`=`content` dans le working tree de `repo`.
    fn commit_file(repo: &Repository, file: &str, content: &str, msg: &str) -> Oid {
        fs::write(repo.workdir().unwrap().join(file), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(file)).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::now("test", "test@husker").unwrap();
        // Premier commit -> pas de parent ; sinon parent = HEAD courant.
        let parents = match repo.head() {
            Ok(h) => vec![h.peel_to_commit().unwrap()],
            Err(_) => vec![],
        };
        let parent_refs: Vec<&_> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)
            .unwrap()
    }

    /// Init un repo "remote" local (non-bare) avec un commit initial — sert de source
    /// de clone, sans réseau ni dépendance GitHub.
    fn init_remote(path: &Path) -> Repository {
        let repo = Repository::init(path).unwrap();
        commit_file(&repo, "README.md", "v1", "initial");
        repo
    }

    fn head_sha(repo: &Repository) -> Oid {
        repo.head().unwrap().peel_to_commit().unwrap().id()
    }

    // --- Unitaire ---

    #[test]
    fn dest_path_joins_root_and_app_id() {
        assert_eq!(
            dest_path("sources", "my-app"),
            Path::new("sources").join("my-app")
        );
    }

    #[test]
    fn pull_up_to_date_keeps_same_sha() {
        let tmp = TmpDir::new();
        init_remote(&tmp.path().join("remote"));
        let url = tmp.path().join("remote");
        let url = url.to_str().unwrap();

        let repo = clone_or_update(url, &tmp.path().join("clone")).unwrap();
        let before = head_sha(&repo);

        // Re-pull sans changement côté remote -> up to date, sha inchangé.
        pull(&repo).unwrap();
        assert_eq!(before, head_sha(&repo));
    }

    #[test]
    fn pull_non_fast_forward_is_rejected() {
        let tmp = TmpDir::new();
        let remote = init_remote(&tmp.path().join("remote"));
        let url = tmp.path().join("remote");
        let url = url.to_str().unwrap();

        let repo = clone_or_update(url, &tmp.path().join("clone")).unwrap();

        // Le clone ET le remote divergent (commits différents sur la même branche).
        commit_file(&repo, "local.txt", "local", "commit local");
        commit_file(&remote, "remote.txt", "remote", "commit remote");

        // -> ni up-to-date ni fast-forward : hors scope POC, on attend une erreur.
        assert!(pull(&repo).is_err());
    }

    // --- E2E : clone puis pull idempotent fast-forward, tout local ---

    #[test]
    fn clone_then_pull_fast_forwards_and_is_idempotent() {
        let tmp = TmpDir::new();
        let remote = init_remote(&tmp.path().join("remote"));
        let url = tmp.path().join("remote");
        let url = url.to_str().unwrap();
        let dest = tmp.path().join("clone");

        // 1) Premier appel -> clone, working tree peuplé.
        let repo = clone_or_update(url, &dest).unwrap();
        let sha1 = head_sha(&repo);
        assert!(dest.join("README.md").exists(), "working tree peuplé au clone");

        // 2) Le remote avance d'un commit.
        commit_file(&remote, "README.md", "v2", "second");

        // 3) Deuxième appel sur le MÊME dest -> pull fast-forward (pas de re-clone).
        let repo = clone_or_update(url, &dest).unwrap();
        let sha2 = head_sha(&repo);

        assert_ne!(sha1, sha2, "le pull a avancé le HEAD");
        assert_eq!(
            fs::read_to_string(dest.join("README.md")).unwrap(),
            "v2",
            "le working tree reflète le nouveau commit"
        );
    }
}
