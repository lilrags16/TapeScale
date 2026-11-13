-- Durable request identity for destructive qualification queue submissions.
-- Existing rows remain valid with both columns NULL. Public queue requests now
-- require a stable key and can receive the original run, including after that
-- run becomes terminal; internal fixtures may continue to create NULL-key rows.

ALTER TABLE qualification_runs
    ADD COLUMN idempotency_key text,
    ADD COLUMN request_fingerprint text,
    ADD CONSTRAINT qualification_runs_idempotency_pair CHECK (
        (idempotency_key IS NULL) = (request_fingerprint IS NULL)
    ),
    ADD CONSTRAINT qualification_runs_idempotency_key_valid CHECK (
        idempotency_key IS NULL OR (
            idempotency_key <> ''
            AND octet_length(idempotency_key) <= 256
            AND idempotency_key !~ '[[:cntrl:]]'
        )
    ),
    ADD CONSTRAINT qualification_runs_request_fingerprint_valid CHECK (
        request_fingerprint IS NULL
        OR request_fingerprint ~ '^[0-9a-f]{64}$'
    );

CREATE UNIQUE INDEX qualification_runs_idempotency_key_unique
    ON qualification_runs(idempotency_key)
    WHERE idempotency_key IS NOT NULL;
