-- Schema version 7. The journal of a shipyard (PLN-47): who did what and when — a container stopped, restarted or
-- removed, a cleaning (manual, disk, automatic), the agents' rule block connected or taken out, a yard removed, and the
-- periods when it did not answer.
--
-- `kind` is the word the page filters by (`stop`, `restart`, `delete`, `clean`, `disk`, `auto`, `rules`, `down`,
-- `remove`); `who` is the actor (`human` or `auto`), never a guess from a name. A container action keeps the task it
-- served (`task`, `tag`) and the container's own name. `until` is set for a downtime row only: the moment the yard
-- answered again (0 — it is silent now). `data` is the JSON of numbers and names for the kind the page reads; it is
-- never a sentence, so both languages can say it.
--
-- Rows live [`KEEP_DAYS`] days; an open downtime row is never pruned (it is the current silence, not history).
CREATE TABLE runner_log (
    n      INTEGER PRIMARY KEY AUTOINCREMENT,
    runner TEXT NOT NULL,
    at     INTEGER NOT NULL,
    kind   TEXT NOT NULL,
    who    TEXT NOT NULL DEFAULT '',
    task   TEXT NOT NULL DEFAULT '',
    tag    TEXT NOT NULL DEFAULT '',
    name   TEXT NOT NULL DEFAULT '',
    ok     INTEGER NOT NULL DEFAULT 1,
    why    TEXT NOT NULL DEFAULT '',
    until  INTEGER NOT NULL DEFAULT 0,
    data   TEXT NOT NULL DEFAULT '{}'
);

CREATE INDEX runner_log_runner_n ON runner_log (runner, n);
CREATE INDEX runner_log_runner_kind ON runner_log (runner, kind);
