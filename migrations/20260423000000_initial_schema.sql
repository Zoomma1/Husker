CREATE TABLE projects (
    id INTEGER PRIMARY KEY NOT NULL,
    name TEXT NOT NULL UNIQUE,
    network_name TEXT NOT NULL UNIQUE,
    created_at TEXT NOT NULL
);

CREATE TABLE apps (
    id INTEGER PRIMARY KEY NOT NULL ,
    project_id INTEGER NOT NULL REFERENCES projects(id),
    name TEXT NOT NULL,
    git_url TEXT NOT NULL,
    git_branch TEXT NOT NULL DEFAULT 'main',
    dockerfile_path TEXT NOT NULL DEFAULT 'Dockerfile',
    build_command TEXT,
    run_command TEXT,
    exposed BOOLEAN NOT NULL DEFAULT FALSE,
    public_domain TEXT,
    status TEXT NOT NULL,  -- pending | building | running | failed | stopped
    created_at TEXT NOT NULL,
    UNIQUE(project_id, name)
);

CREATE TABLE env_vars (
    id INTEGER PRIMARY KEY NOT NULL ,
    app_id INTEGER NOT NULL REFERENCES apps(id),
    key TEXT NOT NULL,
    value TEXT NOT NULL,  -- plain text au MVP → chiffré plus tard
    UNIQUE(app_id, key)
);

CREATE TABLE deployments (
    id INTEGER PRIMARY KEY NOT NULL ,
    app_id INTEGER NOT NULL REFERENCES apps(id),
    git_sha TEXT,
    status TEXT NOT NULL,  -- building | success | failed
    started_at TEXT NOT NULL,
    finished_at TEXT,
    log_path TEXT
);