-- Example package migration (up). Runs against the package's own database.
CREATE TABLE IF NOT EXISTS file_tools_notes(
    id INTEGER PRIMARY KEY,
    note TEXT NOT NULL
);
