-- Schema version 2. Files attached to a todo item: a JSON list of attachments (id, name, mime, size).
ALTER TABLE todo ADD COLUMN files TEXT NOT NULL DEFAULT '[]';
