CREATE TABLE storage_metadata (
    key TEXT NOT NULL PRIMARY KEY CHECK (length(key) > 0),
    value TEXT NOT NULL
);
