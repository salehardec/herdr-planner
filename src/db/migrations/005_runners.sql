-- Schema version 5. Shipyards («верфи»): the last state of every runner. The source of
-- truth is settings/runners.json; this table is only what the planner remembers between
-- polls and restarts.
CREATE TABLE runners_cache (
    id         TEXT PRIMARY KEY,
    json       TEXT NOT NULL DEFAULT '{}',
    fetched_at INTEGER NOT NULL DEFAULT 0,
    ok         INTEGER NOT NULL DEFAULT 0,
    why        TEXT NOT NULL DEFAULT ''
);
