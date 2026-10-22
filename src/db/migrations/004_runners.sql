-- Schema version 4. Shipyards («верфи»): the last state of every runner and the copies of the
-- shipyard list written to the far galaxies. The source of truth is settings/runners.json;
-- these two tables are only what the planner remembers between polls and restarts.
CREATE TABLE runners_cache (
    id         TEXT PRIMARY KEY,
    json       TEXT NOT NULL DEFAULT '{}',
    fetched_at INTEGER NOT NULL DEFAULT 0,
    ok         INTEGER NOT NULL DEFAULT 0,
    why        TEXT NOT NULL DEFAULT ''
);

CREATE TABLE runner_mirror (
    galaxy TEXT PRIMARY KEY,
    hash   TEXT NOT NULL DEFAULT '',
    at     INTEGER NOT NULL DEFAULT 0,
    ok     INTEGER NOT NULL DEFAULT 0,
    why    TEXT NOT NULL DEFAULT ''
);
