CREATE TABLE IF NOT EXISTS meta (
    key TEXT NOT NULL,
    value TEXT NOT NULL,

    PRIMARY KEY (key)
);
INSERT INTO meta (key, value) VALUES ('schema_version', '1');

CREATE TABLE IF NOT EXISTS installed_packages (
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    requested_version TEXT NOT NULL,
    workspace TEXT NOT NULL DEFAULT 'global',

    PRIMARY KEY (name, version, workspace)
    FOREIGN KEY (workspace) REFERENCES workspaces (name) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS installed_packages_workspace ON installed_packages (workspace);

CREATE TABLE IF NOT EXISTS registries (
    name TEXT NOT NULL,
    uri TEXT NOT NULL,
    last_fetched DATETIME,

    PRIMARY KEY (name)
);

CREATE TABLE IF NOT EXISTS known_packages (
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    description TEXT,
    homepage TEXT,
    license TEXT,
    source TEXT,
    build TEXT,
    artifacts TEXT,
    registry TEXT NOT NULL,

    PRIMARY KEY (name, version),
    FOREIGN KEY (registry) REFERENCES registries (name) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS known_packages_registry ON known_packages (registry);

CREATE TABLE IF NOT EXISTS workspaces (
    name TEXT NOT NULL,

    PRIMARY KEY (name)
);
INSERT INTO workspaces (name) VALUES ('global');