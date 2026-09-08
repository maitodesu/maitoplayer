ALTER TABLE publish_attempts ADD COLUMN media_uploaded_at INTEGER;
ALTER TABLE publish_attempts ADD COLUMN note_created_at INTEGER;
ALTER TABLE publish_attempts ADD COLUMN confirmed_at INTEGER;

CREATE TABLE publish_claims (
  mining_id TEXT PRIMARY KEY,
  owner_token TEXT NOT NULL,
  lease_expires_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
) STRICT;

CREATE INDEX publish_claims_expiry_idx ON publish_claims(lease_expires_at);
