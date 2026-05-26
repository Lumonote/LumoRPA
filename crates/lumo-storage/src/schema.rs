/// Embedded DDL applied at first open. Idempotent (uses IF NOT EXISTS).
pub const DDL: &str = r#"
PRAGMA journal_mode = WAL;
PRAGMA synchronous  = NORMAL;
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS flows (
  id            TEXT NOT NULL,
  version       TEXT NOT NULL,
  yaml          TEXT NOT NULL,
  hash          BLOB NOT NULL,
  created_at    INTEGER NOT NULL,
  updated_at    INTEGER NOT NULL,
  tags          TEXT NOT NULL DEFAULT '[]',
  PRIMARY KEY (id, version)
);

CREATE TABLE IF NOT EXISTS flow_runs (
  id              TEXT PRIMARY KEY,
  flow_id         TEXT NOT NULL,
  flow_version    TEXT NOT NULL,
  trigger_kind    TEXT NOT NULL,
  inputs          TEXT NOT NULL,
  outputs         TEXT,
  state           TEXT NOT NULL,
  worker_id       TEXT,
  started_at      INTEGER,
  finished_at     INTEGER,
  cost_token      INTEGER NOT NULL DEFAULT 0,
  cost_usd_micro  INTEGER NOT NULL DEFAULT 0,
  trace_id        TEXT
);
CREATE INDEX IF NOT EXISTS idx_flow_runs_flow ON flow_runs(flow_id, started_at DESC);

CREATE TABLE IF NOT EXISTS step_runs (
  flow_run_id   TEXT NOT NULL,
  step_id       TEXT NOT NULL,
  idx           INTEGER NOT NULL,
  state         TEXT NOT NULL,
  attempt       INTEGER NOT NULL DEFAULT 1,
  input_hash    BLOB NOT NULL,
  output_json   TEXT,
  error         TEXT,
  started_at    INTEGER,
  finished_at   INTEGER,
  span_id       TEXT,
  PRIMARY KEY (flow_run_id, step_id, attempt),
  FOREIGN KEY (flow_run_id) REFERENCES flow_runs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS artifacts (
  id            TEXT PRIMARY KEY,
  flow_run_id   TEXT NOT NULL,
  step_id       TEXT,
  kind          TEXT NOT NULL,
  mime          TEXT NOT NULL,
  size          INTEGER NOT NULL,
  blob_path     TEXT NOT NULL,
  sha256        BLOB NOT NULL,
  created_at    INTEGER NOT NULL,
  FOREIGN KEY (flow_run_id) REFERENCES flow_runs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS vault_items (
  name            TEXT PRIMARY KEY,
  age_ciphertext  BLOB NOT NULL,
  metadata        TEXT NOT NULL,
  updated_at      INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS triggers (
  id              TEXT PRIMARY KEY,
  flow_id         TEXT NOT NULL,
  kind            TEXT NOT NULL,
  spec_json       TEXT NOT NULL,
  enabled         INTEGER NOT NULL DEFAULT 1,
  last_fired_at   INTEGER
);

CREATE TABLE IF NOT EXISTS queue (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  topic           TEXT NOT NULL,
  payload         TEXT NOT NULL,
  priority        INTEGER NOT NULL DEFAULT 5,
  available_at    INTEGER NOT NULL,
  attempts        INTEGER NOT NULL DEFAULT 0,
  visible_until   INTEGER,
  done_at         INTEGER
);
CREATE INDEX IF NOT EXISTS idx_queue_topic_avail
  ON queue(topic, available_at) WHERE done_at IS NULL;
"#;
