-- 0001_initial: local schema for the 9Drive Desktop desktop app.

CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE accounts (
    id                  INTEGER PRIMARY KEY AUTOINCREMENT,
    provider            TEXT    NOT NULL,
    name                TEXT    NOT NULL,
    email               TEXT    NOT NULL,
    external_account_id TEXT,
    drive_folder_id     TEXT,
    created_at          INTEGER NOT NULL,
    last_used_at        INTEGER
);

CREATE TABLE account_quota (
    account_id           INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    limit_bytes          INTEGER NOT NULL,
    usage_bytes          INTEGER NOT NULL,
    usage_in_drive       INTEGER,
    usage_in_drive_trash INTEGER,
    synced_at            INTEGER NOT NULL
);

CREATE TABLE folders (
    id         INTEGER PRIMARY KEY AUTOINCREMENT,
    parent_id  INTEGER REFERENCES folders(id) ON DELETE CASCADE,
    name       TEXT    NOT NULL,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE files (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id    INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    drive_file_id TEXT    NOT NULL UNIQUE,
    folder_id     INTEGER REFERENCES folders(id) ON DELETE SET NULL,
    name          TEXT    NOT NULL,
    mime_type     TEXT,
    size_bytes    INTEGER,
    md5_checksum  TEXT,
    starred       INTEGER NOT NULL DEFAULT 0,
    archived      INTEGER NOT NULL DEFAULT 0,
    trashed_at    INTEGER,
    created_at    INTEGER NOT NULL,
    updated_at    INTEGER NOT NULL
);

CREATE TABLE sync_state (
    account_id       INTEGER PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
    start_page_token TEXT,
    last_sync_at     INTEGER
);

CREATE INDEX idx_files_account_id ON files(account_id);
CREATE INDEX idx_files_folder_id ON files(folder_id);
CREATE INDEX idx_files_name ON files(name);
CREATE INDEX idx_folders_parent_id ON folders(parent_id);