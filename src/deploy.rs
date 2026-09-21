//! Pipeline de déploiement `git → build → run` (HUSKER-13) + lifecycle du container
//! déployé : `stop` / `restart` (HUSKER-14). Jalon M3.
//!
//! Logique extraite des POCs HUSKER-10/11/12 (binaires `src/bin/poc_*.rs`, gelés),
//! adaptée pour l'intégration : retours `Result<_, AppError>`, branche honorée,
//! network depuis la DB, CMD depuis `app.run_command`.
//!
//! Ordre d'orchestration « build d'abord » : `pull → build → (si OK) stop old → run new`.
//! Un build raté ne touche jamais le container qui tourne (cf. ADR à créer).

pub mod build;
pub mod digests;
pub mod git;
pub mod policy;
pub mod run;

use crate::errors::AppError;
use crate::routes::apps::App;
use crate::routes::projects::Project;
use crate::state::AppState;

/// Déploie une app : `git → build → run`, met `status = running` au succès.
/// 404 si l'app n'existe pas. Toute erreur git/build/run -> `status = failed` (best-effort)
/// puis propagation (mappée en 502 par `AppError`).
pub async fn deploy(state: &AppState, app_id: i64) -> Result<App, AppError> {
    let app = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE id = ?",
        app_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    // Join projet : network_name (créé par le CRUD projects) + name (tag image / container).
    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        app.project_id
    )
    .fetch_one(&state.pool)
    .await?;

    // Ouvre la ligne `deployments` (HUSKER-21) : une par tentative, `building` tant que
    // le pipeline tourne. Son id rattache les signaux sécu émis pendant le pipeline.
    let deployment_id = open_deployment(&state.pool, app_id).await?;

    match run_pipeline(state, &app, &project, deployment_id).await {
        Ok(_sha) => {
            // best-effort : le container tourne, une clôture DB ratée ne doit pas le déguiser
            // en 502 ni empêcher le passage de l'app en `running`.
            if let Err(e) = close_deployment(&state.pool, deployment_id, "success").await {
                tracing::error!(deployment_id, "clôture deployments (success) : {e}");
            }
            sqlx::query!("UPDATE apps SET status = 'running' WHERE id = ?", app_id)
                .execute(&state.pool)
                .await?;
        }
        Err(e) => {
            // best-effort : on n'écrase pas l'erreur d'origine si l'UPDATE échoue aussi.
            let _ = close_deployment(&state.pool, deployment_id, "failed").await;
            let _ = sqlx::query!("UPDATE apps SET status = 'failed' WHERE id = ?", app_id)
                .execute(&state.pool)
                .await;
            return Err(e);
        }
    }

    // Recharge l'app avec son status à jour.
    let updated = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE id = ?",
        app_id
    )
    .fetch_one(&state.pool)
    .await?;
    Ok(updated)
}

/// Le pipeline proprement dit. Renvoie le `sha` déployé en cas de succès.
/// Ordre « build d'abord » : `run::run_container` (qui supprime l'ancien container) n'est
/// atteint que si git ET build ont réussi.
async fn run_pipeline(
    state: &AppState,
    app: &App,
    project: &Project,
    deployment_id: i64,
) -> Result<String, AppError> {
    // 1. git clone/pull — git2 est synchrone : on le sort du runtime async via spawn_blocking.
    let sources_root =
        std::env::var("HUSKER_SOURCES_ROOT").unwrap_or_else(|_| "sources".to_string());
    let dest = git::dest_path(&sources_root, &app.id.to_string());
    let url = app.git_url.clone();
    let branch = app.git_branch.clone();
    let dest_for_git = dest.clone();
    let sha =
        tokio::task::spawn_blocking(move || git::clone_or_update(&url, &branch, &dest_for_git))
            .await
            .map_err(|e| AppError::Deploy(format!("git task panicked: {e}")))??;
    sqlx::query!(
        "UPDATE deployments SET git_sha = ? WHERE id = ?",
        sha,
        deployment_id
    )
    .execute(&state.pool)
    .await?;

    // 2. politique supply-chain sur les `FROM` du Dockerfile cloné (HUSKER-15). Placée avant
    //    le build : rien n'a encore été tiré d'un registry à ce stade. Un registry hors
    //    allowlist refuse le deploy (422) ; une base non pinnée est seulement signalée.
    //    Chaque warning est persisté en `deployment_signals` (ADR-020 : le log seul se perd).
    let base_images = policy::read_base_images(&dest, &app.dockerfile_path)?;
    for warning in policy::check(&base_images, &policy::allowed_registries())? {
        tracing::warn!(app = %app.name, "supply-chain : {warning}");
        record_signal(&state.pool, deployment_id, "policy", &warning).await;
    }

    // Puis la détection de dérive : ce que les tags résolvent aujourd'hui vs au deploy
    // précédent. Best-effort — un registry injoignable ne fait pas échouer le deploy.
    for warning in digests::track(&state.pool, &state.docker, app.id, &base_images).await {
        tracing::warn!(app = %app.name, "supply-chain : {warning}");
        record_signal(&state.pool, deployment_id, "digest_drift", &warning).await;
    }

    // 3. build de l'image depuis le contexte cloné (build d'abord : un échec ici ne touche
    //    pas le container qui tourne).
    let image = build::image_ref(&project.name, &app.name, &sha);
    let context = build::make_context_targz(&dest)?;
    build::build_with_buildkit(&state.docker, context, &image, &app.dockerfile_path).await?;

    // 4. env vars depuis la DB.
    let env_rows = sqlx::query!("SELECT key, value FROM env_vars WHERE app_id = ?", app.id)
        .fetch_all(&state.pool)
        .await?;
    let env: Vec<(String, String)> = env_rows.into_iter().map(|r| (r.key, r.value)).collect();

    // 5. run du nouveau container (stop old + run new dans run_container).
    let data_root = std::env::var("HUSKER_DATA_ROOT").unwrap_or_else(|_| "data".to_string());
    let data_abs = run::prepare_data_dir(&data_root, &project.name, &app.name)?;
    let name = run::container_name(&project.name, &app.name);
    let cmd = run::run_cmd(app.run_command.as_deref());
    let config = run::build_container_config(&image, &env, &project.network_name, &data_abs, cmd);
    run::run_container(&state.docker, &name, config).await?;

    Ok(sha)
}

/// Ouvre une ligne `deployments` en `building` ; renvoie son id (HUSKER-21).
async fn open_deployment(pool: &sqlx::SqlitePool, app_id: i64) -> Result<i64, AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    let id = sqlx::query!(
        "INSERT INTO deployments (app_id, status, started_at) VALUES (?, 'building', ?)",
        app_id,
        now
    )
    .execute(pool)
    .await?
    .last_insert_rowid();
    Ok(id)
}

/// Clôt la ligne `deployments` : `status` final (`success` | `failed`) + `finished_at`.
async fn close_deployment(
    pool: &sqlx::SqlitePool,
    deployment_id: i64,
    status: &str,
) -> Result<(), AppError> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query!(
        "UPDATE deployments SET status = ?, finished_at = ? WHERE id = ?",
        status,
        now,
        deployment_id
    )
    .execute(pool)
    .await?;
    Ok(())
}

/// Persiste un signal sécu (warning supply-chain, plus tard CVE) rattaché au déploiement.
/// Best-effort comme le reste du suivi supply-chain : un INSERT raté ne fait pas échouer le
/// deploy, mais il est loggé en `error` — un signal perdu en silence serait le défaut
/// qu'ADR-020 combat.
async fn record_signal(pool: &sqlx::SqlitePool, deployment_id: i64, kind: &str, message: &str) {
    let now = chrono::Utc::now().to_rfc3339();
    let result = sqlx::query!(
        "INSERT INTO deployment_signals (deployment_id, kind, message, created_at) VALUES (?, ?, ?, ?)",
        deployment_id,
        kind,
        message,
        now
    )
    .execute(pool)
    .await;
    if let Err(e) = result {
        tracing::error!(deployment_id, kind, "signal non persisté : {e}");
    }
}

/// Résultat d'un `stop` : container effectivement arrêté, ou déjà arrêté (304 Docker).
/// Distingués pour mapper `AlreadyStopped` en HTTP 304 côté handler (idempotence).
pub enum StopOutcome {
    // `App` est volumineux (~240 o) : boxé pour équilibrer la taille des variantes (clippy).
    Stopped(Box<App>),
    AlreadyStopped,
}

/// Charge l'app (404 si absente) et son projet (network / nom). Helper partagé stop/restart.
async fn load_app_and_project(state: &AppState, app_id: i64) -> Result<(App, Project), AppError> {
    let app = sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE id = ?",
        app_id
    )
    .fetch_optional(&state.pool)
    .await?
    .ok_or(AppError::NotFound)?;

    let project = sqlx::query_as!(
        Project,
        "SELECT id, name, network_name, created_at FROM projects WHERE id = ?",
        app.project_id
    )
    .fetch_one(&state.pool)
    .await?;

    Ok((app, project))
}

/// Recharge l'app avec son status à jour.
async fn reload_app(state: &AppState, app_id: i64) -> Result<App, AppError> {
    Ok(sqlx::query_as!(
        App,
        "SELECT id, project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status
         FROM apps WHERE id = ?",
        app_id
    )
    .fetch_one(&state.pool)
    .await?)
}

/// Arrête le container d'une app et passe `status = stopped` (HUSKER-14).
/// Le container est conservé (pas de `rm`) -> `restart` peut le relancer sans redéployer.
/// 404 si l'app n'existe pas (DB) OU si son container n'existe pas (app jamais déployée).
/// Container déjà arrêté -> `StopOutcome::AlreadyStopped` (idempotence -> 304), pas d'UPDATE.
///
/// On inspecte AVANT de stopper : bollard renvoie `Ok` même quand le container est déjà
/// arrêté (le 304 Docker n'est pas surfacé comme erreur), donc l'état doit être lu via
/// `inspect_container`. L'inspection donne aussi l'existence (404 -> NotFound).
pub async fn stop(state: &AppState, app_id: i64) -> Result<StopOutcome, AppError> {
    let (app, project) = load_app_and_project(state, app_id).await?;
    let name = run::container_name(&project.name, &app.name);

    let info = match state.docker.inspect_container(&name, None).await {
        Ok(info) => info,
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => return Err(AppError::NotFound),
        Err(e) => return Err(AppError::Docker(e)),
    };

    let running = info.state.and_then(|s| s.running).unwrap_or(false);
    if !running {
        return Ok(StopOutcome::AlreadyStopped);
    }

    state
        .docker
        .stop_container(
            &name,
            None::<bollard::query_parameters::StopContainerOptions>,
        )
        .await?;
    sqlx::query!("UPDATE apps SET status = 'stopped' WHERE id = ?", app_id)
        .execute(&state.pool)
        .await?;
    Ok(StopOutcome::Stopped(Box::new(
        reload_app(state, app_id).await?,
    )))
}

/// Relance le container d'une app (depuis l'état stopped ou running) et passe `status = running`.
/// `restart_container` est idempotent côté état : il démarre un container arrêté, bounce un
/// container vivant. 404 si l'app n'existe pas (DB) OU si son container n'existe pas.
pub async fn restart(state: &AppState, app_id: i64) -> Result<App, AppError> {
    let (app, project) = load_app_and_project(state, app_id).await?;
    let name = run::container_name(&project.name, &app.name);

    match state
        .docker
        .restart_container(
            &name,
            None::<bollard::query_parameters::RestartContainerOptions>,
        )
        .await
    {
        Ok(_) => {
            sqlx::query!("UPDATE apps SET status = 'running' WHERE id = ?", app_id)
                .execute(&state.pool)
                .await?;
            reload_app(state, app_id).await
        }
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => Err(AppError::NotFound),
        Err(e) => Err(AppError::Docker(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bollard::models::NetworkCreateRequest;
    use bollard::query_parameters::{RemoveContainerOptionsBuilder, RemoveImageOptions};
    use bollard::Docker;
    use git2::{Repository, Signature};
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::SqlitePool;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;

    // Lock de sérialisation des roots (process-global), partagé avec la pipeline E2E.
    use crate::routes::test_routes_helpers::ENV_ROOTS_LOCK as DEPLOY_IT_LOCK;

    /// Image légère, container qui reste vivant (CMD de l'image, `run_command` laissé None
    /// -> on teste le drop du hack `sleep` du POC).
    const RUNNING_DOCKERFILE: &str = "FROM alpine:3.20\nCMD [\"sleep\", \"3600\"]\n";
    /// Build voué à échouer (le `RUN` sort non-zéro) -> erreur côté build.
    const BROKEN_DOCKERFILE: &str = "FROM alpine:3.20\nRUN exit 1\n";

    struct TmpDir(PathBuf);
    impl TmpDir {
        fn new() -> Self {
            let p = std::env::temp_dir().join(format!("husker-deploy-it-{}", uuid::Uuid::new_v4()));
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

    async fn test_pool() -> SqlitePool {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .create_if_missing(true);
        let pool = SqlitePool::connect_with(opts).await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        pool
    }

    /// Init un repo git fixture local (déterministe : on contrôle son contenu).
    fn init_repo(dir: &Path) -> Repository {
        Repository::init(dir).unwrap()
    }

    /// (Re)commit le `Dockerfile` avec `content` sur le HEAD courant ; renvoie le sha produit.
    fn commit_dockerfile(repo: &Repository, content: &str, msg: &str) -> String {
        commit_file(repo, "Dockerfile", content, msg)
    }

    /// Noms uniques pour ne pas collisionner entre runs ; renvoie (project, app, network).
    fn unique_names() -> (String, String, String) {
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let project = format!("itp{suffix}");
        let network = format!("husker_{project}");
        (project, "app".to_string(), network)
    }

    async fn create_project_network(docker: &Docker, name: &str) {
        docker
            .create_network(NetworkCreateRequest {
                name: name.to_string(),
                ..Default::default()
            })
            .await
            .unwrap();
    }

    /// Seed un projet + une app + une env var. Renvoie l'app_id.
    async fn seed_app(
        pool: &SqlitePool,
        project_name: &str,
        network_name: &str,
        app_name: &str,
        git_url: &str,
        branch: &str,
    ) -> i64 {
        let now = chrono::Utc::now().to_rfc3339();
        let project_id = sqlx::query!(
            "INSERT INTO projects (name, network_name, created_at) VALUES (?, ?, ?)",
            project_name,
            network_name,
            now
        )
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();

        let app_id = sqlx::query!(
            "INSERT INTO apps (project_id, name, git_url, git_branch, dockerfile_path, build_command, run_command, created_at, exposed, public_domain, status)
             VALUES (?, ?, ?, ?, 'Dockerfile', NULL, NULL, ?, 0, NULL, 'pending')",
            project_id,
            app_name,
            git_url,
            branch,
            now
        )
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();

        sqlx::query!(
            "INSERT INTO env_vars (app_id, key, value) VALUES (?, 'HUSKER_GREETING', 'hello')",
            app_id
        )
        .execute(pool)
        .await
        .unwrap();

        app_id
    }

    async fn app_status(pool: &SqlitePool, app_id: i64) -> String {
        sqlx::query_scalar!("SELECT status FROM apps WHERE id = ?", app_id)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    /// Ligne `deployments` observée (HUSKER-21) : (status, git_sha, finished_at).
    struct DeploymentRow {
        status: String,
        git_sha: Option<String>,
        finished_at: Option<String>,
    }

    async fn deployments_of(pool: &SqlitePool, app_id: i64) -> Vec<DeploymentRow> {
        sqlx::query_as!(
            DeploymentRow,
            "SELECT status, git_sha, finished_at FROM deployments WHERE app_id = ? ORDER BY id",
            app_id
        )
        .fetch_all(pool)
        .await
        .unwrap()
    }

    /// Signaux persistés pour une app, via son/ses déploiements : (kind, message).
    async fn signals_of(pool: &SqlitePool, app_id: i64) -> Vec<(String, String)> {
        sqlx::query!(
            "SELECT s.kind, s.message FROM deployment_signals s
             JOIN deployments d ON d.id = s.deployment_id
             WHERE d.app_id = ? ORDER BY s.id",
            app_id
        )
        .fetch_all(pool)
        .await
        .unwrap()
        .into_iter()
        .map(|r| (r.kind, r.message))
        .collect()
    }

    /// Commit un fichier arbitraire (pour un repo fixture SANS Dockerfile, ou nommé autrement).
    fn commit_file(repo: &Repository, file: &str, content: &str, msg: &str) -> String {
        let wd = repo.workdir().unwrap();
        fs::write(wd.join(file), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(file)).unwrap();
        index.write().unwrap();
        let tree_oid = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_oid).unwrap();
        let sig = Signature::now("husker-test", "test@husker").unwrap();
        let parents: Vec<git2::Commit> = match repo.head() {
            Ok(h) => vec![h.peel_to_commit().unwrap()],
            Err(_) => vec![],
        };
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)
            .unwrap()
            .to_string()
    }

    /// Cleanup best-effort : container -> network -> image(s) buildée(s).
    async fn cleanup(docker: &Docker, project: &str, app: &str, network: &str, shas: &[&str]) {
        let container = run::container_name(project, app);
        let _ = docker
            .remove_container(
                &container,
                Some(RemoveContainerOptionsBuilder::default().force(true).build()),
            )
            .await;
        let _ = docker.remove_network(network).await;
        for sha in shas {
            let img = build::image_ref(project, app, sha);
            let _ = docker
                .remove_image(&img, None::<RemoveImageOptions>, None)
                .await;
        }
    }

    fn set_roots(tmp: &TmpDir) {
        std::env::set_var("HUSKER_SOURCES_ROOT", tmp.path().join("sources"));
        std::env::set_var("HUSKER_DATA_ROOT", tmp.path().join("data"));
    }

    // --- Offline (suite normale) ---

    #[tokio::test]
    async fn deploy_unknown_app_is_not_found() {
        // Branche 404 : pas de Docker ni de git touchés (retour avant tout I/O).
        let state = AppState {
            pool: test_pool().await,
            docker: Docker::connect_with_local_defaults().unwrap(),
        };
        let result = deploy(&state, 999_999).await;
        assert!(matches!(result, Err(AppError::NotFound)));
    }

    #[tokio::test]
    async fn deploy_invalid_git_url_is_git_error_and_marks_failed() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let pool = test_pool().await;
        let (project, app, network) = unique_names();
        // URL = chemin local inexistant -> git2 échoue immédiatement (pas de réseau, pas de Docker).
        let bad_url = tmp.path().join("does-not-exist");
        let app_id = seed_app(
            &pool,
            &project,
            &network,
            &app,
            bad_url.to_str().unwrap(),
            "main",
        )
        .await;

        let state = AppState {
            pool: pool.clone(),
            docker: Docker::connect_with_local_defaults().unwrap(),
        };
        let result = deploy(&state, app_id).await;

        assert!(
            matches!(result, Err(AppError::Git(_))),
            "échec git attendu (-> 502)"
        );
        assert_eq!(
            app_status(&pool, app_id).await,
            "failed",
            "status DB doit passer à failed"
        );

        // HUSKER-21 : une ligne `deployments` close en failed, sans sha (échec avant le clone).
        let rows = deployments_of(&pool, app_id).await;
        assert_eq!(rows.len(), 1, "exactement une ligne deployments");
        assert_eq!(rows[0].status, "failed");
        assert_eq!(rows[0].git_sha, None, "pas de sha : git a échoué");
        assert!(
            rows[0].finished_at.is_some(),
            "finished_at renseigné à la clôture"
        );
    }

    #[tokio::test]
    async fn deploy_missing_dockerfile_records_failed_with_sha() {
        // 2e étape d'échec distincte : git OK (repo local), puis lecture du Dockerfile KO.
        // Offline : aucune image tirée, Docker jamais contacté.
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let pool = test_pool().await;
        let repo = init_repo(&tmp.path().join("repo"));
        let sha = commit_file(&repo, "README.md", "no dockerfile here\n", "init");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;
        let state = AppState {
            pool: pool.clone(),
            docker: Docker::connect_with_local_defaults().unwrap(),
        };

        let result = deploy(&state, app_id).await;

        assert!(
            matches!(result, Err(AppError::Deploy(_))),
            "Dockerfile absent -> Deploy"
        );
        assert_eq!(app_status(&pool, app_id).await, "failed");
        let rows = deployments_of(&pool, app_id).await;
        assert_eq!(rows.len(), 1, "exactement une ligne deployments");
        assert_eq!(rows[0].status, "failed");
        assert_eq!(
            rows[0].git_sha.as_deref(),
            Some(sha.as_str()),
            "le sha est posé dès le clone, même si le pipeline échoue après"
        );
        assert!(rows[0].finished_at.is_some());
        assert!(
            signals_of(&pool, app_id).await.is_empty(),
            "aucun signal sans Dockerfile"
        );
    }

    #[tokio::test]
    async fn policy_warning_is_persisted_as_deployment_signal() {
        // `FROM $BASE` : signalé par la policy (allowlist inapplicable), ignoré par le suivi
        // de digests (non résoluble). Le build échoue ensuite (ARG vide / daemon absent) :
        // le signal doit survivre à l'échec du deploy — c'est tout l'objet d'ADR-020.
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let pool = test_pool().await;
        let repo = init_repo(&tmp.path().join("repo"));
        commit_dockerfile(&repo, "FROM $BASE\n", "arg base");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;
        let state = AppState {
            pool: pool.clone(),
            docker: Docker::connect_with_local_defaults().unwrap(),
        };

        let result = deploy(&state, app_id).await;
        assert!(
            result.is_err(),
            "un `FROM $BASE` sans ARG ne peut pas se builder"
        );

        let rows = deployments_of(&pool, app_id).await;
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, "failed");

        let signals = signals_of(&pool, app_id).await;
        assert_eq!(signals.len(), 1, "un signal policy : {signals:?}");
        assert_eq!(signals[0].0, "policy");
        assert!(
            signals[0].1.contains("non résoluble (ARG)"),
            "message du warning conservé tel quel : {}",
            signals[0].1
        );
    }

    // --- E2E (git + Docker réels, #[ignore]) ---

    #[tokio::test]
    #[ignore = "Docker+git réels : clone repo fixture -> build -> run (cargo test -- --ignored)"]
    async fn deploy_runs_full_git_build_run_chain() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let repo = init_repo(&tmp.path().join("repo"));
        let sha = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "init");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        create_project_network(&docker, &network).await;
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;

        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };
        let result = deploy(&state, app_id).await;

        let container = run::container_name(&project, &app);
        let inspect = docker.inspect_container(&container, None).await;
        cleanup(&docker, &project, &app, &network, &[&sha]).await;

        let deployed = result.expect("deploy doit réussir");
        assert_eq!(deployed.status, "running", "status DB -> running");

        // HUSKER-21 : une ligne `deployments` en success, complète.
        let rows = deployments_of(&pool, app_id).await;
        assert_eq!(rows.len(), 1, "exactement une ligne deployments");
        assert_eq!(rows[0].status, "success");
        assert_eq!(rows[0].git_sha.as_deref(), Some(sha.as_str()));
        assert!(rows[0].finished_at.is_some());
        // `alpine:3.20` non pinné -> warning policy persisté, relisible sans les logs.
        let signals = signals_of(&pool, app_id).await;
        assert!(
            signals
                .iter()
                .any(|(k, m)| k == "policy" && m.contains("non pinnée")),
            "signal policy attendu : {signals:?}"
        );

        let info = inspect.expect("container inspectable");
        assert_eq!(
            info.state.and_then(|s| s.running),
            Some(true),
            "container running"
        );

        let networks = info
            .network_settings
            .and_then(|n| n.networks)
            .unwrap_or_default();
        assert!(
            networks.contains_key(&network),
            "attaché au network projet {network} : {:?}",
            networks.keys().collect::<Vec<_>>()
        );

        let env = info.config.and_then(|c| c.env).unwrap_or_default();
        assert!(
            env.iter().any(|e| e == "HUSKER_GREETING=hello"),
            "env var injectée : {env:?}"
        );
    }

    #[tokio::test]
    #[ignore = "Docker+git réels : redeploy (cargo test -- --ignored)"]
    async fn redeploy_advances_sha_and_replaces_container() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let repo = init_repo(&tmp.path().join("repo"));
        let sha1 = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "v1");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        create_project_network(&docker, &network).await;
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;
        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };

        // Deploy 1.
        let r1 = deploy(&state, app_id).await;
        // Le repo avance d'un commit (même contenu suffit : parent différent -> sha différent).
        let sha2 = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "v2");
        // Deploy 2 (redeploy).
        let r2 = deploy(&state, app_id).await;

        let container = run::container_name(&project, &app);
        let inspect = docker.inspect_container(&container, None).await;
        cleanup(&docker, &project, &app, &network, &[&sha1, &sha2]).await;

        assert_ne!(sha1, sha2, "le 2e commit produit un sha différent");
        r1.expect("deploy 1 ok");
        let after = r2.expect("deploy 2 (redeploy) ok");
        assert_eq!(after.status, "running");

        // Le container tourne sur la NOUVELLE image (sha2) -> l'ancien a bien été remplacé.
        let info = inspect.expect("container inspectable");
        assert_eq!(info.state.and_then(|s| s.running), Some(true));
        assert_eq!(
            info.config.and_then(|c| c.image),
            Some(build::image_ref(&project, &app, &sha2)),
            "redeploy doit faire tourner l'image du nouveau sha"
        );
    }

    #[tokio::test]
    #[ignore = "Docker+git réels : branche non-défaut (cargo test -- --ignored)"]
    async fn deploy_honors_configured_branch() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let repo = init_repo(&tmp.path().join("repo"));
        let sha_default = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "default branch");

        // Crée une branche "feature" à partir du HEAD, puis commit dessus.
        let head_commit = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("feature", &head_commit, false).unwrap();
        repo.set_head("refs/heads/feature").unwrap();
        repo.checkout_head(Some(git2::build::CheckoutBuilder::default().force()))
            .unwrap();
        let sha_feature = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "feature branch");
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        create_project_network(&docker, &network).await;
        // App configurée sur "feature".
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, "feature").await;
        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };

        let result = deploy(&state, app_id).await;

        // Le code cloné est bien le HEAD de "feature", pas celui de la branche par défaut.
        let cloned = Repository::open(tmp.path().join("sources").join(app_id.to_string())).unwrap();
        let cloned_sha = cloned
            .head()
            .unwrap()
            .peel_to_commit()
            .unwrap()
            .id()
            .to_string();
        cleanup(&docker, &project, &app, &network, &[&sha_feature]).await;

        result.expect("deploy ok");
        assert_ne!(sha_feature, sha_default, "les deux branches divergent");
        assert_eq!(
            cloned_sha, sha_feature,
            "le sha déployé doit être le HEAD de la branche configurée"
        );
    }

    #[tokio::test]
    #[ignore = "Docker+git réels : build raté garde l'ancien container (cargo test -- --ignored)"]
    async fn build_failure_keeps_previous_container_and_marks_failed() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let repo = init_repo(&tmp.path().join("repo"));
        let sha1 = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "good");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        create_project_network(&docker, &network).await;
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;
        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };

        // Deploy 1 OK -> container running (sha1).
        let r1 = deploy(&state, app_id).await;
        // On casse le Dockerfile puis on redéploie.
        let sha2 = commit_dockerfile(&repo, BROKEN_DOCKERFILE, "broken");
        let r2 = deploy(&state, app_id).await;

        let container = run::container_name(&project, &app);
        let inspect = docker.inspect_container(&container, None).await;
        let status = app_status(&pool, app_id).await;
        cleanup(&docker, &project, &app, &network, &[&sha1, &sha2]).await;

        r1.expect("deploy 1 ok");
        assert!(r2.is_err(), "le build raté doit faire échouer le redeploy");
        assert_eq!(status, "failed", "status DB -> failed");

        // HUSKER-21 : l'historique garde les deux tentatives, dans l'ordre.
        let rows = deployments_of(&pool, app_id).await;
        assert_eq!(rows.len(), 2, "deux lignes deployments (une par tentative)");
        assert_eq!(
            (rows[0].status.as_str(), rows[0].git_sha.as_deref()),
            ("success", Some(sha1.as_str()))
        );
        assert_eq!(
            (rows[1].status.as_str(), rows[1].git_sha.as_deref()),
            ("failed", Some(sha2.as_str()))
        );

        // « build d'abord » : l'ancien container (sha1) tourne TOUJOURS, intact.
        let info = inspect.expect("ancien container toujours présent");
        assert_eq!(
            info.state.and_then(|s| s.running),
            Some(true),
            "l'app précédente reste running après un build raté"
        );
        assert_eq!(
            info.config.and_then(|c| c.image),
            Some(build::image_ref(&project, &app, &sha1)),
            "le container intact est bien celui du sha précédent"
        );
    }

    // --- Stop / restart (HUSKER-14) ---

    #[tokio::test]
    #[ignore = "Docker+git réels : stop arrête le container sans le supprimer (cargo test -- --ignored)"]
    async fn stop_marks_stopped_and_keeps_container() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let repo = init_repo(&tmp.path().join("repo"));
        let sha = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "init");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        create_project_network(&docker, &network).await;
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;
        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };

        deploy(&state, app_id).await.expect("deploy ok");
        let outcome = stop(&state, app_id).await;

        let container = run::container_name(&project, &app);
        let inspect = docker.inspect_container(&container, None).await;
        cleanup(&docker, &project, &app, &network, &[&sha]).await;

        assert!(
            matches!(outcome, Ok(StopOutcome::Stopped(_))),
            "stop doit réussir"
        );
        assert_eq!(
            app_status(&pool, app_id).await,
            "stopped",
            "status DB -> stopped"
        );
        let info = inspect.expect("le container doit toujours exister après stop");
        assert_eq!(
            info.state.and_then(|s| s.running),
            Some(false),
            "container arrêté, pas supprimé"
        );
    }

    #[tokio::test]
    #[ignore = "Docker+git réels : 2e stop -> AlreadyStopped (304) (cargo test -- --ignored)"]
    async fn stop_already_stopped_returns_already_stopped() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let repo = init_repo(&tmp.path().join("repo"));
        let sha = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "init");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        create_project_network(&docker, &network).await;
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;
        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };

        deploy(&state, app_id).await.expect("deploy ok");
        stop(&state, app_id).await.expect("1er stop ok");
        let second = stop(&state, app_id).await;

        cleanup(&docker, &project, &app, &network, &[&sha]).await;

        assert!(
            matches!(second, Ok(StopOutcome::AlreadyStopped)),
            "stop idempotent : 2e stop -> AlreadyStopped (304), pas d'erreur"
        );
    }

    #[tokio::test]
    #[ignore = "Docker+git réels : restart relance un container stoppé (cargo test -- --ignored)"]
    async fn restart_brings_stopped_container_back_running() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let tmp = TmpDir::new();
        set_roots(&tmp);

        let repo = init_repo(&tmp.path().join("repo"));
        let sha = commit_dockerfile(&repo, RUNNING_DOCKERFILE, "init");
        let branch = repo.head().unwrap().shorthand().unwrap().to_string();
        let git_url = tmp.path().join("repo").to_str().unwrap().to_string();

        let (project, app, network) = unique_names();
        create_project_network(&docker, &network).await;
        let app_id = seed_app(&pool, &project, &network, &app, &git_url, &branch).await;
        let state = AppState {
            pool: pool.clone(),
            docker: docker.clone(),
        };

        deploy(&state, app_id).await.expect("deploy ok");
        stop(&state, app_id).await.expect("stop ok");
        let result = restart(&state, app_id).await;

        let container = run::container_name(&project, &app);
        let inspect = docker.inspect_container(&container, None).await;
        cleanup(&docker, &project, &app, &network, &[&sha]).await;

        let app_after = result.expect("restart doit réussir");
        assert_eq!(app_after.status, "running", "status DB -> running");
        let info = inspect.expect("container présent");
        assert_eq!(
            info.state.and_then(|s| s.running),
            Some(true),
            "container redémarré après restart"
        );
    }

    #[tokio::test]
    #[ignore = "Docker réel : stop d'une app jamais déployée (container absent) -> 404 (cargo test -- --ignored)"]
    async fn stop_app_without_container_is_not_found() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        // Seed DB uniquement : pas de network, pas de deploy -> aucun container.
        let (project, app, network) = unique_names();
        let app_id = seed_app(
            &pool,
            &project,
            &network,
            &app,
            "https://example.invalid/repo",
            "main",
        )
        .await;
        let state = AppState {
            pool: pool.clone(),
            docker,
        };

        let result = stop(&state, app_id).await;
        assert!(
            matches!(result, Err(AppError::NotFound)),
            "container absent (app jamais déployée) -> NotFound (404)"
        );
    }

    #[tokio::test]
    #[ignore = "Docker réel : restart d'une app jamais déployée (container absent) -> 404 (cargo test -- --ignored)"]
    async fn restart_app_without_container_is_not_found() {
        let _guard = DEPLOY_IT_LOCK.lock().await;
        let docker = Docker::connect_with_local_defaults().unwrap();
        let pool = test_pool().await;
        let (project, app, network) = unique_names();
        let app_id = seed_app(
            &pool,
            &project,
            &network,
            &app,
            "https://example.invalid/repo",
            "main",
        )
        .await;
        let state = AppState {
            pool: pool.clone(),
            docker,
        };

        let result = restart(&state, app_id).await;
        assert!(
            matches!(result, Err(AppError::NotFound)),
            "container absent (app jamais déployée) -> NotFound (404)"
        );
    }
}
