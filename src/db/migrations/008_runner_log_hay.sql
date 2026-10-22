-- Schema version 8. The journal's searchable text (PLN-47): the lower-cased words of a row, built from the container's
-- name, the task, the reason and the values in `data` (never the JSON keys). SQLite's LIKE is case-insensitive for
-- ASCII only, so the search asks this column with `instr`; Cyrillic «верфь» then finds «Верфь», and `_` and `%` are
-- ordinary characters. Rows written before this column are filled once when the database is opened.
ALTER TABLE runner_log ADD COLUMN hay TEXT NOT NULL DEFAULT '';
