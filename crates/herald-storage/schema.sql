CREATE TABLE IF NOT EXISTS providers (
    name TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 0,
    health TEXT NOT NULL DEFAULT 'unknown',
    profile_ref TEXT,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS messages (
    provider TEXT NOT NULL,
    message_id TEXT NOT NULL,
    thread_id TEXT NOT NULL,
    sender TEXT NOT NULL,
    subject TEXT NOT NULL DEFAULT '',
    timestamp_ms INTEGER NOT NULL,
    unread INTEGER NOT NULL DEFAULT 0,
    last_seen_at INTEGER NOT NULL,
    PRIMARY KEY (provider, message_id)
);

CREATE INDEX IF NOT EXISTS idx_messages_thread
    ON messages (provider, thread_id, timestamp_ms);

CREATE TABLE IF NOT EXISTS actions (
    action_id TEXT PRIMARY KEY,
    provider TEXT NOT NULL,
    operation TEXT NOT NULL,
    target TEXT NOT NULL,
    content_hash TEXT,
    state TEXT NOT NULL,
    created_at INTEGER NOT NULL,
    completed_at INTEGER
);

CREATE INDEX IF NOT EXISTS idx_actions_state ON actions (state, created_at);

CREATE TABLE IF NOT EXISTS audit_events (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    action_id TEXT,
    at_ms INTEGER NOT NULL,
    status TEXT NOT NULL,
    detail TEXT NOT NULL DEFAULT ''
);

CREATE TABLE IF NOT EXISTS preferences (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
