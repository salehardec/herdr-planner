-- Schema version 1. Applied once; PRAGMA user_version records the level.

-- Dela (the God's own list). `ord` is the position; `to_` is '', 'quasar' or a task id.
CREATE TABLE todo (
    id   TEXT PRIMARY KEY,
    text TEXT NOT NULL,
    at   INTEGER NOT NULL,
    ord  INTEGER NOT NULL,
    to_  TEXT NOT NULL DEFAULT ''
);
CREATE INDEX todo_ord ON todo(ord);

-- Journal of board events; `n` is the entry's sequence number.
CREATE TABLE journal (
    n    INTEGER PRIMARY KEY,
    t    INTEGER NOT NULL,
    id   TEXT NOT NULL,
    s    TEXT NOT NULL,
    text TEXT NOT NULL
);

-- Status changes per task; a row is written only when the status differs from the last row.
CREATE TABLE status_history (
    id     INTEGER PRIMARY KEY AUTOINCREMENT,
    task   TEXT NOT NULL,
    status TEXT NOT NULL,
    at     INTEGER NOT NULL
);
CREATE INDEX status_history_task ON status_history(task, id);

-- The former chat.json: one row (id = 1).
CREATE TABLE chat_state (
    id         INTEGER PRIMARY KEY CHECK (id = 1),
    last_sent  TEXT NOT NULL DEFAULT '',
    sent_at    INTEGER NOT NULL DEFAULT 0,
    last_reply TEXT NOT NULL DEFAULT '',
    history    TEXT NOT NULL DEFAULT '[]'
);

-- Index of the Coordinator's transcript turns. `n` is the position over all
-- transcripts (the numbering `history::page` uses); `h` fingerprints the content.
CREATE TABLE chat_turns (
    n     INTEGER PRIMARY KEY,
    at    INTEGER NOT NULL,
    from_ TEXT NOT NULL,
    q     TEXT NOT NULL,
    a     TEXT NOT NULL,
    h     TEXT NOT NULL DEFAULT ''
);
CREATE VIRTUAL TABLE chat_fts USING fts5(
    q, a,
    content = 'chat_turns',
    content_rowid = 'n',
    tokenize = 'trigram'
);
CREATE TRIGGER chat_turns_ai AFTER INSERT ON chat_turns BEGIN
    INSERT INTO chat_fts(rowid, q, a) VALUES (new.n, new.q, new.a);
END;
CREATE TRIGGER chat_turns_ad AFTER DELETE ON chat_turns BEGIN
    INSERT INTO chat_fts(chat_fts, rowid, q, a) VALUES ('delete', old.n, old.q, old.a);
END;
CREATE TRIGGER chat_turns_au AFTER UPDATE ON chat_turns BEGIN
    INSERT INTO chat_fts(chat_fts, rowid, q, a) VALUES ('delete', old.n, old.q, old.a);
    INSERT INTO chat_fts(rowid, q, a) VALUES (new.n, new.q, new.a);
END;

-- Searchable documents (planets, tasks, galaxies, ...). `rid` is a stable integer
-- rowid for the external-content FTS table; `key` is the logical primary key.
CREATE TABLE search_docs (
    rid    INTEGER PRIMARY KEY,
    key    TEXT NOT NULL UNIQUE,
    kind   TEXT NOT NULL,
    id     TEXT NOT NULL,
    galaxy TEXT NOT NULL DEFAULT '',
    path   TEXT NOT NULL DEFAULT '',
    title  TEXT NOT NULL DEFAULT '',
    text   TEXT NOT NULL DEFAULT '',
    status TEXT NOT NULL DEFAULT '',
    at     INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX search_docs_galaxy ON search_docs(galaxy);
CREATE VIRTUAL TABLE search_fts USING fts5(
    title, text,
    content = 'search_docs',
    content_rowid = 'rid',
    tokenize = 'trigram'
);
CREATE TRIGGER search_docs_ai AFTER INSERT ON search_docs BEGIN
    INSERT INTO search_fts(rowid, title, text) VALUES (new.rid, new.title, new.text);
END;
CREATE TRIGGER search_docs_ad AFTER DELETE ON search_docs BEGIN
    INSERT INTO search_fts(search_fts, rowid, title, text) VALUES ('delete', old.rid, old.title, old.text);
END;
CREATE TRIGGER search_docs_au AFTER UPDATE ON search_docs BEGIN
    INSERT INTO search_fts(search_fts, rowid, title, text) VALUES ('delete', old.rid, old.title, old.text);
    INSERT INTO search_fts(rowid, title, text) VALUES (new.rid, new.title, new.text);
END;

-- Translations of answers, keyed by a hash of the source text.
CREATE TABLE translations (
    hash TEXT PRIMARY KEY,
    text TEXT NOT NULL,
    at   INTEGER NOT NULL
);

-- Far galaxies (their boards) and the state of the link to each.
CREATE TABLE galaxies (
    label      TEXT PRIMARY KEY,
    json       TEXT NOT NULL,
    fetched_at INTEGER NOT NULL,
    ok         INTEGER NOT NULL,
    why        TEXT NOT NULL DEFAULT ''
);

-- Main-branch lag per repository of a galaxy.
CREATE TABLE gitlag (
    galaxy TEXT NOT NULL,
    repo   TEXT NOT NULL,
    json   TEXT NOT NULL,
    at     INTEGER NOT NULL,
    PRIMARY KEY (galaxy, repo)
);

CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
