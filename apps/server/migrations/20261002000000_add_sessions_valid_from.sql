-- Browser session revocation cutoff in Unix microseconds; NULL preserves existing sessions.
ALTER TABLE users ADD COLUMN sessions_valid_from BIGINT;
