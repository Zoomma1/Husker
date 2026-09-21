-- Signaux de sécurité rattachés à un déploiement (HUSKER-21, socle persistant d'ADR-020).
--
-- `run_pipeline` détecte (policy supply-chain, dérive de digest — plus tard le scan CVE)
-- mais ne faisait que logger : en home lab personne ne lit les logs du daemon. Chaque
-- warning devient une ligne relisible après coup, rattachée au deploy qui l'a produit.
-- Table fille plutôt qu'une colonne JSON sur `deployments` : requêtable (« quels deploys
-- ont eu une dérive ? »), typée par `kind`, extensible au rapport Trivy (HUSKER-16).
--
-- `id INTEGER PRIMARY KEY NOT NULL` : le NOT NULL est explicite, sinon sqlx infère
-- Option<i64> et casse `query_as!` (ADR-007).
CREATE TABLE deployment_signals (
    id INTEGER PRIMARY KEY NOT NULL,
    deployment_id INTEGER NOT NULL REFERENCES deployments(id),
    kind TEXT NOT NULL,            -- policy | digest_drift
    message TEXT NOT NULL,         -- le warning tel que loggé
    created_at TEXT NOT NULL
);

CREATE INDEX idx_deployment_signals_deployment ON deployment_signals(deployment_id);
