-- Schema version 2. Suggestions for a universe's memory, made once for a task when it is done.
-- `lines` is a JSON list of {kind, text}; `state` is working (being made), pending (waits for the human),
-- done (kept), dismissed, none (nothing worth keeping) or failed (no model answered).
CREATE TABLE memory_suggest (
    task  TEXT PRIMARY KEY,
    repo  TEXT NOT NULL,
    tag   TEXT NOT NULL DEFAULT '',
    lines TEXT NOT NULL DEFAULT '[]',
    state TEXT NOT NULL DEFAULT 'working',
    at    INTEGER NOT NULL
);
