/// Embedded baseline DDL applied by the v0->v1 migration step. Idempotent
/// (uses IF NOT EXISTS). Connection-scoped PRAGMAs (journal_mode, synchronous,
/// foreign_keys, busy_timeout) live in `repo::init_connection`, not here, so
/// they are applied to every connection regardless of migration state.
pub const DDL: &str = r#"
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
  seq           INTEGER NOT NULL,
  path          TEXT NOT NULL,
  parent_path   TEXT,
  depth         INTEGER NOT NULL DEFAULT 0,
  step_id       TEXT NOT NULL,
  idx           INTEGER NOT NULL,
  state         TEXT NOT NULL,
  attempt       INTEGER NOT NULL DEFAULT 1,
  input_hash    BLOB NOT NULL,
  output_json   TEXT,
  vars_json     TEXT,
  error         TEXT,
  started_at    INTEGER,
  finished_at   INTEGER,
  span_id       TEXT,
  PRIMARY KEY (flow_run_id, seq),
  FOREIGN KEY (flow_run_id) REFERENCES flow_runs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_step_runs_flow_path
  ON step_runs(flow_run_id, path);

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

-- X-10 AI call accounting. One row per LLM/vision invocation; rolled up into
-- flow_runs.cost_token / cost_usd_micro when the run finishes.
CREATE TABLE IF NOT EXISTS ai_calls (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  flow_run_id       TEXT NOT NULL,
  step_id           TEXT,
  helper            TEXT NOT NULL DEFAULT 'chat',
  provider          TEXT NOT NULL,
  model             TEXT NOT NULL,
  input_tokens      INTEGER NOT NULL DEFAULT 0,
  output_tokens     INTEGER NOT NULL DEFAULT 0,
  latency_ms        INTEGER NOT NULL DEFAULT 0,
  cost_usd_micro    INTEGER NOT NULL DEFAULT 0,
  created_at        INTEGER NOT NULL,
  FOREIGN KEY (flow_run_id) REFERENCES flow_runs(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_ai_calls_run ON ai_calls(flow_run_id, created_at);
"#;

/// Schema additions introduced by migration v4. Kept separate from the v2
/// baseline so existing migration semantics remain unchanged.
pub const V4_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS voice_profiles (
  id TEXT PRIMARY KEY,
  config_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS mcp_servers (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  transport TEXT NOT NULL,
  config_json TEXT NOT NULL,
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  health TEXT NOT NULL DEFAULT 'unknown',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS mcp_tools (
  server_id TEXT NOT NULL,
  name TEXT NOT NULL,
  description TEXT NOT NULL DEFAULT '',
  input_schema TEXT NOT NULL,
  output_schema TEXT,
  risk TEXT NOT NULL DEFAULT 'L0',
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  version_hash TEXT NOT NULL,
  discovered_at INTEGER NOT NULL,
  PRIMARY KEY (server_id, name),
  FOREIGN KEY (server_id) REFERENCES mcp_servers(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS capability_aliases (
  capability_id TEXT NOT NULL,
  alias TEXT NOT NULL,
  examples_json TEXT NOT NULL DEFAULT '[]',
  enabled INTEGER NOT NULL DEFAULT 1 CHECK (enabled IN (0, 1)),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (capability_id, alias)
);

CREATE TABLE IF NOT EXISTS agent_profiles (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  config_json TEXT NOT NULL,
  is_default INTEGER NOT NULL DEFAULT 0 CHECK (is_default IN (0, 1)),
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_runs (
  id TEXT PRIMARY KEY,
  profile_id TEXT,
  utterance TEXT,
  plan_json TEXT,
  approval_json TEXT,
  state TEXT NOT NULL,
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  error TEXT
);
CREATE INDEX IF NOT EXISTS idx_agent_runs_state_started
  ON agent_runs(state, started_at DESC);

CREATE TABLE IF NOT EXISTS agent_events (
  run_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  kind TEXT NOT NULL,
  node_id TEXT,
  parent_node_id TEXT,
  payload TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (run_id, seq),
  FOREIGN KEY (run_id) REFERENCES agent_runs(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS improvement_proposals (
  id TEXT PRIMARY KEY,
  target_kind TEXT NOT NULL,
  target_id TEXT NOT NULL,
  patch_json TEXT NOT NULL,
  rationale TEXT NOT NULL,
  status TEXT NOT NULL,
  base_version_hash TEXT NOT NULL,
  evaluation_json TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS improvement_approvals (
  proposal_id TEXT NOT NULL,
  patch_hash TEXT NOT NULL,
  base_version_hash TEXT NOT NULL,
  approver TEXT NOT NULL,
  decision TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (proposal_id, created_at),
  FOREIGN KEY (proposal_id) REFERENCES improvement_proposals(id) ON DELETE CASCADE
);
"#;

pub const V5_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS agent_jobs (
  id TEXT PRIMARY KEY,
  idempotency_key TEXT NOT NULL UNIQUE,
  payload_json TEXT NOT NULL,
  schedule_kind TEXT NOT NULL CHECK (schedule_kind IN ('one_shot', 'cron')),
  schedule_spec_json TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'queued'
    CHECK (state IN ('queued', 'running', 'waiting', 'paused', 'completed', 'failed', 'unknown')),
  priority INTEGER NOT NULL DEFAULT 5,
  available_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts > 0),
  worker_id TEXT,
  lease_until INTEGER,
  heartbeat_at INTEGER,
  last_error TEXT,
  cancelled_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_agent_jobs_ready
  ON agent_jobs(state, available_at, priority DESC)
  WHERE state IN ('queued', 'waiting');
CREATE INDEX IF NOT EXISTS idx_agent_jobs_lease
  ON agent_jobs(state, lease_until)
  WHERE state = 'running';

CREATE TABLE IF NOT EXISTS agent_job_nodes (
  job_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  state TEXT NOT NULL
    CHECK (state IN ('queued', 'running', 'waiting', 'paused', 'completed', 'failed', 'unknown')),
  risk TEXT NOT NULL CHECK (risk IN ('L0', 'L1', 'L2', 'L3')),
  idempotent INTEGER NOT NULL CHECK (idempotent IN (0, 1)),
  attempt INTEGER NOT NULL DEFAULT 1 CHECK (attempt > 0),
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (job_id, node_id),
  FOREIGN KEY (job_id) REFERENCES agent_jobs(id) ON DELETE CASCADE
);
"#;

/// Rebuild the two job tables together so CHECK constraints and foreign keys
/// are upgraded atomically while preserving legacy checkpoints.
pub const V6_DDL: &str = r#"
CREATE TABLE agent_jobs_v6 (
  id TEXT PRIMARY KEY,
  idempotency_key TEXT NOT NULL UNIQUE,
  payload_json TEXT NOT NULL,
  schedule_kind TEXT NOT NULL CHECK (schedule_kind IN ('one_shot', 'cron')),
  schedule_spec_json TEXT NOT NULL,
  state TEXT NOT NULL DEFAULT 'queued'
    CHECK (state IN ('queued', 'running', 'waiting', 'waiting_approval', 'paused', 'completed', 'failed', 'unknown')),
  priority INTEGER NOT NULL DEFAULT 5,
  available_at INTEGER NOT NULL,
  attempts INTEGER NOT NULL DEFAULT 0 CHECK (attempts >= 0),
  max_attempts INTEGER NOT NULL DEFAULT 3 CHECK (max_attempts > 0),
  worker_id TEXT,
  lease_until INTEGER,
  heartbeat_at INTEGER,
  last_error TEXT,
  cancelled_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  run_id TEXT
);
INSERT INTO agent_jobs_v6 SELECT agent_jobs.*,NULL FROM agent_jobs;
CREATE TABLE agent_job_nodes_v6 (
  job_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  state TEXT NOT NULL
    CHECK (state IN ('queued', 'running', 'waiting', 'paused', 'completed', 'failed', 'unknown')),
  risk TEXT NOT NULL CHECK (risk IN ('L0', 'L1', 'L2', 'L3')),
  idempotent INTEGER NOT NULL CHECK (idempotent IN (0, 1)),
  attempt INTEGER NOT NULL DEFAULT 1 CHECK (attempt > 0),
  updated_at INTEGER NOT NULL,
  node_hash TEXT NOT NULL DEFAULT '',
  result_json TEXT,
  PRIMARY KEY (job_id, node_id),
  FOREIGN KEY (job_id) REFERENCES agent_jobs_v6(id) ON DELETE CASCADE
);
INSERT INTO agent_job_nodes_v6 SELECT agent_job_nodes.*,'',NULL FROM agent_job_nodes;
DROP TABLE agent_job_nodes;
DROP TABLE agent_jobs;
ALTER TABLE agent_jobs_v6 RENAME TO agent_jobs;
ALTER TABLE agent_job_nodes_v6 RENAME TO agent_job_nodes;
UPDATE agent_job_nodes SET state='unknown'
  WHERE state='completed' AND (node_hash='' OR result_json IS NULL)
    AND job_id IN (SELECT id FROM agent_jobs WHERE state IN ('queued','running','waiting','paused','unknown'));
UPDATE agent_jobs SET state='unknown',worker_id=NULL,lease_until=NULL,heartbeat_at=NULL,
  last_error='legacy checkpoint has no durable output; human review required'
  WHERE state IN ('queued','running','waiting','paused')
    AND EXISTS(SELECT 1 FROM agent_job_nodes WHERE job_id=agent_jobs.id AND state='unknown');
CREATE INDEX idx_agent_jobs_ready ON agent_jobs(state, available_at, priority DESC)
  WHERE state IN ('queued', 'waiting');
CREATE INDEX idx_agent_jobs_lease ON agent_jobs(state, lease_until) WHERE state = 'running';
CREATE UNIQUE INDEX idx_agent_jobs_run ON agent_jobs(run_id) WHERE run_id IS NOT NULL;
"#;

/// Invocation attempts retain metered failures and cancellation independently
/// of the latest durable node output. Flow associations survive flow pruning.
pub const V7_DDL: &str = r#"
CREATE TABLE agent_invocation_attempts (
  run_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  attempt INTEGER NOT NULL CHECK(attempt > 0),
  capability_id TEXT NOT NULL,
  source_kind TEXT NOT NULL CHECK(source_kind IN ('flow','skill','mcp')),
  flow_run_id TEXT,
  state TEXT NOT NULL CHECK(state IN ('running','completed','failed','cancelled','timed_out','dropped')),
  usage_state TEXT NOT NULL CHECK(usage_state IN ('pending','recorded','unknown')),
  budget_sensitive INTEGER NOT NULL CHECK(budget_sensitive IN (0,1)),
  tokens_used INTEGER CHECK(tokens_used >= 0),
  cost_usd_micro INTEGER CHECK(cost_usd_micro >= 0),
  duration_ms INTEGER NOT NULL DEFAULT 0 CHECK(duration_ms >= 0),
  started_at INTEGER NOT NULL,
  finished_at INTEGER,
  error TEXT,
  revision INTEGER NOT NULL DEFAULT 0 CHECK(revision >= 0),
  resolved_by TEXT,
  resolution_reason TEXT,
  resolved_at INTEGER,
  PRIMARY KEY(run_id,node_id,attempt)
);
CREATE INDEX idx_invocation_attempts_run ON agent_invocation_attempts(run_id,started_at);
CREATE TABLE agent_invocation_flow_runs (
  flow_run_id TEXT PRIMARY KEY,
  run_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  attempt INTEGER NOT NULL,
  parent_flow_run_id TEXT,
  state TEXT NOT NULL,
  usage_unknown INTEGER NOT NULL DEFAULT 1 CHECK(usage_unknown IN (0,1)),
  tokens_used INTEGER CHECK(tokens_used >= 0),
  cost_usd_micro INTEGER CHECK(cost_usd_micro >= 0),
  duration_ms INTEGER NOT NULL DEFAULT 0 CHECK(duration_ms >= 0),
  FOREIGN KEY(run_id,node_id,attempt) REFERENCES agent_invocation_attempts(run_id,node_id,attempt) ON DELETE CASCADE
);
CREATE INDEX idx_invocation_flow_runs_attempt ON agent_invocation_flow_runs(run_id,node_id,attempt);
CREATE TABLE agent_invocation_usage_resolutions (
  run_id TEXT NOT NULL,
  node_id TEXT NOT NULL,
  attempt INTEGER NOT NULL,
  revision INTEGER NOT NULL,
  tokens_used INTEGER NOT NULL CHECK(tokens_used >= 0),
  cost_usd_micro INTEGER NOT NULL CHECK(cost_usd_micro >= 0),
  resolved_by TEXT NOT NULL,
  reason TEXT NOT NULL,
  resolved_at INTEGER NOT NULL,
  PRIMARY KEY(run_id,node_id,attempt,revision),
  FOREIGN KEY(run_id,node_id,attempt) REFERENCES agent_invocation_attempts(run_id,node_id,attempt) ON DELETE CASCADE
);
"#;

/// Local CRM records are independent of execution history retention.
pub const V8_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS acquisition_campaigns (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  product TEXT NOT NULL,
  target_audience TEXT NOT NULL,
  keywords TEXT NOT NULL,
  platforms TEXT NOT NULL,
  max_leads INTEGER NOT NULL CHECK(max_leads BETWEEN 1 AND 200),
  status TEXT NOT NULL CHECK(status IN ('draft','running','completed','failed','cancelled')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  last_error TEXT,
  outreach TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS acquisition_leads (
  id TEXT PRIMARY KEY,
  campaign_id TEXT NOT NULL REFERENCES acquisition_campaigns(id) ON DELETE CASCADE,
  platform TEXT NOT NULL CHECK(platform IN ('xiaohongshu','douyin','weibo')),
  platform_user_id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  profile_url TEXT NOT NULL,
  source_url TEXT NOT NULL,
  evidence TEXT NOT NULL,
  bio TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('new','qualified','contacted','replied','converted','ignored')),
  intent_score INTEGER NOT NULL CHECK(intent_score BETWEEN 0 AND 100),
  tags TEXT NOT NULL,
  profile TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  UNIQUE(campaign_id,platform,platform_user_id)
);
CREATE INDEX IF NOT EXISTS idx_acquisition_leads_campaign ON acquisition_leads(campaign_id,updated_at DESC);
CREATE INDEX IF NOT EXISTS idx_acquisition_leads_identity ON acquisition_leads(platform,platform_user_id);
CREATE TABLE IF NOT EXISTS acquisition_messages (
  id TEXT PRIMARY KEY,
  lead_id TEXT NOT NULL REFERENCES acquisition_leads(id) ON DELETE CASCADE,
  direction TEXT NOT NULL CHECK(direction IN ('inbound','outbound')),
  body TEXT NOT NULL,
  status TEXT NOT NULL CHECK(status IN ('draft','approved','sending','sent','failed','received','uncertain')),
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  error TEXT,
  attempted_at INTEGER,
  CHECK((direction='inbound' AND status='received') OR (direction='outbound' AND status!='received'))
);
CREATE INDEX IF NOT EXISTS idx_acquisition_messages_lead ON acquisition_messages(lead_id,created_at);
CREATE UNIQUE INDEX IF NOT EXISTS idx_acquisition_outbound_draft ON acquisition_messages(lead_id,body) WHERE direction='outbound' AND status!='failed';
CREATE INDEX IF NOT EXISTS idx_acquisition_messages_status ON acquisition_messages(status,updated_at);
CREATE TABLE IF NOT EXISTS acquisition_requirements (
  id TEXT PRIMARY KEY,
  lead_id TEXT NOT NULL REFERENCES acquisition_leads(id) ON DELETE CASCADE,
  summary TEXT NOT NULL,
  category TEXT NOT NULL,
  urgency TEXT NOT NULL,
  evidence TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  UNIQUE(lead_id,summary,evidence)
);
CREATE INDEX IF NOT EXISTS idx_acquisition_requirements_lead ON acquisition_requirements(lead_id,created_at);
CREATE TABLE IF NOT EXISTS acquisition_activity (
  id TEXT PRIMARY KEY,
  campaign_id TEXT REFERENCES acquisition_campaigns(id) ON DELETE CASCADE,
  lead_id TEXT REFERENCES acquisition_leads(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  detail TEXT NOT NULL,
  created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_acquisition_activity_campaign ON acquisition_activity(campaign_id,created_at DESC);
"#;

/// Ordered analysis inputs. Private replies reference the stored message so
/// the queue does not retain another copy of conversation text.
pub const V9_DDL: &str = r#"
CREATE TABLE IF NOT EXISTS acquisition_inputs (
  seq INTEGER PRIMARY KEY,
  lead_id TEXT NOT NULL REFERENCES acquisition_leads(id) ON DELETE CASCADE,
  source_url TEXT,
  excerpt TEXT,
  message_id TEXT UNIQUE REFERENCES acquisition_messages(id) ON DELETE CASCADE,
  observed_at INTEGER NOT NULL,
  CHECK((source_url IS NOT NULL AND excerpt IS NOT NULL AND message_id IS NULL)
     OR (source_url IS NULL AND excerpt IS NULL AND message_id IS NOT NULL)),
  UNIQUE(lead_id,source_url,excerpt)
);
CREATE INDEX IF NOT EXISTS idx_acquisition_inputs_lead ON acquisition_inputs(lead_id,seq);
CREATE TABLE IF NOT EXISTS acquisition_analysis_progress (
  lead_id TEXT PRIMARY KEY REFERENCES acquisition_leads(id) ON DELETE CASCADE,
  consumed INTEGER NOT NULL DEFAULT 0,
  revision INTEGER NOT NULL DEFAULT 0
);
INSERT OR IGNORE INTO acquisition_inputs(lead_id,source_url,excerpt,observed_at)
  SELECT id,source_url,evidence,created_at FROM acquisition_leads ORDER BY created_at,id;
INSERT OR IGNORE INTO acquisition_inputs(lead_id,message_id,observed_at)
  SELECT lead_id,id,created_at FROM acquisition_messages WHERE direction='inbound' ORDER BY created_at,id;
INSERT OR IGNORE INTO acquisition_analysis_progress(lead_id) SELECT id FROM acquisition_leads;
CREATE TRIGGER IF NOT EXISTS acq_initial_input AFTER INSERT ON acquisition_leads BEGIN
  INSERT INTO acquisition_analysis_progress(lead_id) VALUES(NEW.id);
  INSERT INTO acquisition_inputs(lead_id,source_url,excerpt,observed_at)
    VALUES(NEW.id,NEW.source_url,NEW.evidence,NEW.created_at);
END;
CREATE TRIGGER IF NOT EXISTS acq_reply_input AFTER INSERT ON acquisition_messages WHEN NEW.direction='inbound' BEGIN
  INSERT INTO acquisition_inputs(lead_id,message_id,observed_at) VALUES(NEW.lead_id,NEW.id,NEW.created_at);
END;
"#;
