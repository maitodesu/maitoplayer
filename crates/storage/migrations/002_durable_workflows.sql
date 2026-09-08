ALTER TABLE jobs ADD COLUMN attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0);
ALTER TABLE jobs ADD COLUMN created_at INTEGER NOT NULL DEFAULT 0;

CREATE INDEX jobs_recovery_idx ON jobs(stage, updated_at);

CREATE TABLE publish_attempts (
  mining_id TEXT PRIMARY KEY,
  job_id TEXT NOT NULL UNIQUE,
  stage TEXT NOT NULL CHECK (
    stage IN ('validated', 'assets_ready', 'media_uploaded', 'note_created', 'confirmed', 'failed')
  ),
  note_id INTEGER,
  asset_hashes_json TEXT NOT NULL DEFAULT '[]',
  terminal_error_json TEXT,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  CHECK (
    (stage IN ('note_created', 'confirmed') AND note_id IS NOT NULL)
    OR (stage NOT IN ('note_created', 'confirmed') AND note_id IS NULL)
  )
) STRICT;

CREATE INDEX publish_attempts_recovery_idx ON publish_attempts(stage, updated_at);

CREATE TABLE asset_references (
  owner_kind TEXT NOT NULL,
  owner_id TEXT NOT NULL,
  asset_hash TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  PRIMARY KEY (owner_kind, owner_id, asset_hash)
) STRICT;

CREATE INDEX asset_references_hash_idx ON asset_references(asset_hash);
