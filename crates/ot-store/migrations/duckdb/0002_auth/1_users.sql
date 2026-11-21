-- Users, roles, sessions and API tokens. Written in portable SQL so the same
-- schema can later be created on PostgreSQL.
CREATE SEQUENCE IF NOT EXISTS user_ids START 1;
CREATE TABLE IF NOT EXISTS users (
    id BIGINT PRIMARY KEY DEFAULT nextval('user_ids'),
    username VARCHAR NOT NULL UNIQUE,
    full_name VARCHAR NOT NULL,
    password_hash VARCHAR NOT NULL,
    enabled BOOLEAN NOT NULL DEFAULT TRUE,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS user_roles (
    user_id BIGINT NOT NULL REFERENCES users(id),
    role VARCHAR NOT NULL CHECK (role IN
        ('system_manager', 'books_manager', 'books_user', 'books_cashier', 'viewer')),
    PRIMARY KEY (user_id, role)
);

CREATE TABLE IF NOT EXISTS sessions (
    token_hash VARCHAR PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id),
    csrf_token VARCHAR NOT NULL,
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    expires_unix BIGINT NOT NULL
);

CREATE SEQUENCE IF NOT EXISTS api_token_ids START 1;
CREATE TABLE IF NOT EXISTS api_tokens (
    id BIGINT PRIMARY KEY DEFAULT nextval('api_token_ids'),
    user_id BIGINT NOT NULL REFERENCES users(id),
    name VARCHAR NOT NULL,
    token_hash VARCHAR NOT NULL UNIQUE,
    scope VARCHAR NOT NULL CHECK (scope IN ('read', 'draft', 'submit', 'admin')),
    created_at TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP,
    revoked_at TIMESTAMP
);
