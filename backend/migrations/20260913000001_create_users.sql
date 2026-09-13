-- users + auth_identities — docs/pulse-database-schema.md #2.1, #2.1b
--
-- UUID PKs use gen_random_uuid(), built into Postgres 13+ (no pgcrypto /
-- uuid-ossp extension needed).

CREATE TABLE users (
    id            UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email         TEXT        NOT NULL,
    -- NULL when the user only signs in via OAuth.
    password_hash TEXT,
    role          TEXT        NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user')),
    is_active     BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Emails are unique case-insensitively ("A@x.com" and "a@x.com" are the same
-- account) — matters for OAuth linking by email (#2.1b), where providers may
-- return a different casing than what the user registered with.
CREATE UNIQUE INDEX users_email_lower_key ON users (lower(email));

CREATE TABLE auth_identities (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id          UUID        NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    provider         TEXT        NOT NULL CHECK (provider IN ('email', 'google', 'github')),
    -- Provider-side id (Google `sub`, GitHub user id); NULL for provider = 'email'.
    provider_user_id TEXT,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),

    CONSTRAINT auth_identities_provider_user_id_presence CHECK (
        (provider = 'email') = (provider_user_id IS NULL)
    ),
    -- Per schema doc: one Google/GitHub account can't map to two users.
    CONSTRAINT auth_identities_provider_user_key UNIQUE (provider, provider_user_id),
    -- Addition: at most one identity per provider per user (also covers
    -- 'email', where provider_user_id is NULL and so escapes the key above).
    -- Doubles as the index for user_id lookups / FK cascades.
    CONSTRAINT auth_identities_user_provider_key UNIQUE (user_id, provider)
);
