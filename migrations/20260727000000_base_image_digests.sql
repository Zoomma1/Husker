-- Digest observé côté registry pour chaque image de base d'une app (HUSKER-15).
--
-- Husker ne peut pas pinner le `FROM` d'un Dockerfile qu'il ne possède pas. Il peut en
-- revanche mémoriser ce que la référence résolvait la dernière fois : même `reference`,
-- digest différent = le tag a été réécrit upstream. C'est le signal supply-chain.
--
-- `id INTEGER PRIMARY KEY NOT NULL` : le NOT NULL est explicite, sinon sqlx infère
-- Option<i64> et casse `query_as!` (ADR-007).
CREATE TABLE base_image_digests (
    id INTEGER PRIMARY KEY NOT NULL,
    app_id INTEGER NOT NULL REFERENCES apps(id),
    reference TEXT NOT NULL,       -- la ref telle qu'écrite : "alpine:3.20"
    digest TEXT NOT NULL,          -- "sha256:..." résolu au registry
    first_seen_at TEXT NOT NULL,
    last_seen_at TEXT NOT NULL,
    UNIQUE(app_id, reference)
);
