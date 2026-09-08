CREATE TABLE settings (
  key TEXT PRIMARY KEY,
  value_json TEXT NOT NULL,
  settings_version INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE recents (
  source_fingerprint TEXT PRIMARY KEY,
  path TEXT NOT NULL,
  display_name TEXT NOT NULL,
  consented_at INTEGER NOT NULL,
  last_opened_at INTEGER NOT NULL
) STRICT;

CREATE TABLE subtitle_bindings (
  source_fingerprint TEXT PRIMARY KEY,
  subtitle_source_version TEXT NOT NULL,
  binding_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE drafts (
  draft_id TEXT PRIMARY KEY,
  revision INTEGER NOT NULL,
  snapshot_json TEXT NOT NULL,
  created_at INTEGER NOT NULL
) STRICT;

CREATE TABLE jobs (
  job_id TEXT PRIMARY KEY,
  kind TEXT NOT NULL,
  stage TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  terminal_error_json TEXT,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE TABLE mining_history (
  mining_id TEXT PRIMARY KEY,
  note_id INTEGER NOT NULL,
  asset_hashes_json TEXT NOT NULL,
  confirmed_at INTEGER NOT NULL
) STRICT;

