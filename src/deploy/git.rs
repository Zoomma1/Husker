//! Maillon `git` du pipeline de déploiement (extrait du POC HUSKER-10).
//!
//! Différence avec le POC : on **honore la branche configurée** sur l'app
//! (`RepoBuilder::branch` au clone, fetch + fast-forward de cette branche au pull),
//! là où le POC se contentait de la branche par défaut du remote / courante.
//! Erreurs libgit2 mappées vers `AppError::Git` (→ 502).

use crate::errors::AppError;
use git2::{
    build::{CheckoutBuilder, RepoBuilder},
    AnnotatedCommit, AutotagOption, FetchOptions, Reference, Repository,
};
use std::path::{Path, PathBuf};

/// Chemin de destination d'un repo cloné : `<root>/<app_id>`.
/// PathBuf::join, jamais de concaténation de string OS-spécifique (portabilité).
pub fn dest_path(root: &str, app_id: &str) -> PathBuf {
    Path::new(root).join(app_id)
}

/// Idempotent : si `dest` est déjà un repo git valide -> pull `branch`, sinon -> clone
/// sur `branch`. Renvoie le sha HEAD après opération (sert de tag d'image).
pub fn clone_or_update(url: &str, branch: &str, dest: &Path) -> Result<String, AppError> {
    let repo = match Repository::open(dest) {
        Ok(repo) => {
            pull(&repo, branch)?;
            repo
        }
        Err(_) => RepoBuilder::new()
            .branch(branch)
            .clone(url, dest)
            .map_err(git_err)?,
    };

    let sha = repo
        .head()
        .map_err(git_err)?
        .peel_to_commit()
        .map_err(git_err)?
        .id()
        .to_string();
    Ok(sha)
}

/// Pull fast-forward-only de `branch` depuis `origin` (fetch réseau puis ff local).
fn pull(repo: &Repository, branch: &str) -> Result<(), AppError> {
    let mut remote = repo.find_remote("origin").map_err(git_err)?;
    let mut fo = FetchOptions::new();
    fo.download_tags(AutotagOption::All);
    remote
        .fetch(&[branch], Some(&mut fo), None)
        .map_err(git_err)?;

    let fetch_head = repo.find_reference("FETCH_HEAD").map_err(git_err)?;
    let fetched = repo
        .reference_to_annotated_commit(&fetch_head)
        .map_err(git_err)?;

    let (analysis, _) = repo.merge_analysis(&[&fetched]).map_err(git_err)?;
    if analysis.is_up_to_date() {
        Ok(())
    } else if analysis.is_fast_forward() {
        fast_forward(repo, branch, &fetched)
    } else {
        // Divergence / merge nécessaire : explicitement hors scope.
        Err(AppError::Git(
            "pull non fast-forward (divergence)".to_string(),
        ))
    }
}

/// Déplace la ref locale de branche sur le commit fetché, puis aligne le working tree.
fn fast_forward(repo: &Repository, branch: &str, target: &AnnotatedCommit) -> Result<(), AppError> {
    let refname = format!("refs/heads/{branch}");
    let mut reference: Reference = repo.find_reference(&refname).map_err(git_err)?;
    reference
        .set_target(target.id(), "husker: fast-forward")
        .map_err(git_err)?;
    repo.set_head(&refname).map_err(git_err)?;
    // force() : sources/<app_id> est jetable, aucune modif locale à préserver.
    repo.checkout_head(Some(CheckoutBuilder::default().force()))
        .map_err(git_err)?;
    Ok(())
}

fn git_err(e: git2::Error) -> AppError {
    AppError::Git(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{Oid, Signature};
    use std::fs;

    /// Répertoire temporaire unique, nettoyé à la fin (RAII, robuste au panic).
    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("husker-deploy-{}", uuid::Uuid::new_v4()));
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

    fn commit_file(repo: &Repository, file: &str, content: &str, msg: &str) -> Oid {
        fs::write(repo.workdir().unwrap().join(file), content).unwrap();
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
            .unwrap()
    }

    /// Init un repo "remote" local (non-bare) avec un commit initial — source de clone,
    /// sans réseau. Renvoie (repo, nom de sa branche courante).
    fn init_remote(path: &Path) -> (Repository, String) {
        let repo = Repository::init(path).unwrap();
        commit_file(&repo, "README.md", "v1", "initial");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        (repo, branch)
    }

    #[test]
    fn dest_path_joins_root_and_app_id() {
        assert_eq!(dest_path("sources", "42"), Path::new("sources").join("42"));
    }

    #[test]
    fn clone_then_pull_fast_forwards_and_is_idempotent() {
        let tmp = TmpDir::new();
        let (remote, branch) = init_remote(&tmp.path().join("remote"));
        let url = tmp.path().join("remote");
        let url = url.to_str().unwrap();
        let dest = tmp.path().join("clone");

        // 1) Premier appel -> clone sur la branche, working tree peuplé, sha renvoyé.
        let sha1 = clone_or_update(url, &branch, &dest).unwrap();
        assert!(!sha1.is_empty(), "sha renvoyé au clone");
        assert!(dest.join("README.md").exists(), "working tree peuplé");

        // 2) Le remote avance d'un commit.
        commit_file(&remote, "README.md", "v2", "second");

        // 3) Deuxième appel sur le MÊME dest -> pull fast-forward (pas de re-clone).
        let sha2 = clone_or_update(url, &branch, &dest).unwrap();
        assert_ne!(sha1, sha2, "le pull a avancé le HEAD");
        assert_eq!(
            fs::read_to_string(dest.join("README.md")).unwrap(),
            "v2",
            "le working tree reflète le nouveau commit"
        );
    }

    #[test]
    fn pull_non_fast_forward_is_rejected() {
        let tmp = TmpDir::new();
        let (remote, branch) = init_remote(&tmp.path().join("remote"));
        let url = tmp.path().join("remote");
        let url = url.to_str().unwrap();
        let dest = tmp.path().join("clone");

        clone_or_update(url, &branch, &dest).unwrap();

        // Le clone ET le remote divergent (commits différents sur la même branche).
        let clone = Repository::open(&dest).unwrap();
        commit_file(&clone, "local.txt", "local", "commit local");
        commit_file(&remote, "remote.txt", "remote", "commit remote");

        // -> ni up-to-date ni fast-forward : hors scope, on attend une erreur Git.
        let err = clone_or_update(url, &branch, &dest).unwrap_err();
        assert!(matches!(err, AppError::Git(_)));
    }
}
