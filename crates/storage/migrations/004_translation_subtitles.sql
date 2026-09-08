CREATE TABLE translation_subtitle_bindings (
  source_fingerprint TEXT PRIMARY KEY,
  subtitle_source_version TEXT NOT NULL,
  binding_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;
