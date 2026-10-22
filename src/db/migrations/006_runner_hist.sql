-- Schema version 6. What the shipyards did over the last hours and days (PLN-46).
--
-- `runner_hist` is the load of a yard over time, thinned: one row per minute for the last hours, one row per ten
-- minutes for the last day. `step` is the width of the row in seconds; `at` is its start. The three values are
-- percentages of the machine (0 … 100); NULL means «not known» (the yard did not answer, or the machine has no such
-- thing) and the page breaks the line there.
CREATE TABLE runner_hist (
    runner TEXT NOT NULL,
    step   INTEGER NOT NULL,
    at     INTEGER NOT NULL,
    cpu    REAL,
    mem    REAL,
    disk   REAL,
    PRIMARY KEY (runner, step, at)
);

-- What the containers of each task ate on a yard, keyed by the `herdr.task` label of the containers. `cpu_sec` is
-- CPU time summed over the samples (one core × one second); `mem_peak` is the highest sum of the memory of the task's
-- running containers seen at one sample. `task` is the board task the label names, empty for a label this board does
-- not know (a removed task, another board's container). `first_ms` / `last_ms` are the first and the last sample the
-- task was seen at.
CREATE TABLE task_usage (
    runner      TEXT NOT NULL,
    tag         TEXT NOT NULL,
    task        TEXT NOT NULL DEFAULT '',
    cpu_sec     REAL NOT NULL DEFAULT 0,
    mem_peak    INTEGER NOT NULL DEFAULT 0,
    first_ms    INTEGER NOT NULL DEFAULT 0,
    last_ms     INTEGER NOT NULL DEFAULT 0,
    PRIMARY KEY (runner, tag)
);
