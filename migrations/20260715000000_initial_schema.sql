CREATE TABLE accounts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    email TEXT NOT NULL UNIQUE,
    display_name TEXT,
    username TEXT NOT NULL,
    password TEXT NOT NULL,
    imap_host TEXT NOT NULL,
    imap_port INTEGER NOT NULL CHECK (imap_port BETWEEN 1 AND 65535),
    smtp_host TEXT NOT NULL,
    smtp_port INTEGER NOT NULL CHECK (smtp_port BETWEEN 1 AND 65535),
    security_mode TEXT NOT NULL CHECK (security_mode IN ('tls', 'start_tls', 'none')),
    created_at INTEGER NOT NULL DEFAULT (unixepoch()),
    updated_at INTEGER NOT NULL DEFAULT (unixepoch())
);

CREATE TABLE folders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    name TEXT NOT NULL,
    delimiter TEXT NOT NULL DEFAULT '/',
    attributes TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(attributes)),
    uid_validity INTEGER,
    highest_uid INTEGER NOT NULL DEFAULT 0,
    message_count INTEGER NOT NULL DEFAULT 0,
    unread_count INTEGER NOT NULL DEFAULT 0,
    last_synced_at INTEGER,
    UNIQUE (account_id, name)
);

CREATE TABLE messages (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    uid INTEGER NOT NULL,
    message_id TEXT,
    sender TEXT NOT NULL DEFAULT '',
    recipients TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(recipients)),
    cc TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(cc)),
    bcc TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(bcc)),
    reply_to TEXT,
    subject TEXT NOT NULL DEFAULT '',
    sent_at TEXT,
    size INTEGER NOT NULL DEFAULT 0,
    flags TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(flags)),
    has_attachments INTEGER NOT NULL DEFAULT 0 CHECK (has_attachments IN (0, 1)),
    body_text TEXT,
    body_html TEXT,
    in_reply_to TEXT,
    references_json TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(references_json)),
    cached_at INTEGER NOT NULL DEFAULT (unixepoch()),
    UNIQUE (folder_id, uid)
);

CREATE TABLE attachments (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id INTEGER NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
    filename TEXT NOT NULL,
    mime_type TEXT NOT NULL,
    size INTEGER NOT NULL DEFAULT 0,
    content_id TEXT,
    part_id TEXT NOT NULL,
    local_path TEXT,
    UNIQUE (message_id, part_id)
);

CREATE INDEX idx_folders_account ON folders(account_id);
CREATE INDEX idx_messages_folder_uid ON messages(folder_id, uid DESC);
CREATE INDEX idx_messages_message_id ON messages(message_id);
CREATE INDEX idx_attachments_message ON attachments(message_id);
