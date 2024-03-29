CREATE TABLE IF NOT EXISTS meta (
    key TEXT NOT NULL,
    value TEXT NOT NULL,

    PRIMARY KEY (key)
);
INSERT INTO meta (key, value) VALUES ('schema_version', '1');

CREATE TABLE IF NOT EXISTS installed_packages (
    name TEXT NOT NULL,
    version TEXT NOT NULL,

    PRIMARY KEY (name, version)
);

CREATE TABLE IF NOT EXISTS registries (
    uri TEXT NOT NULL,
    name TEXT,
    last_fetched DATETIME,

    PRIMARY KEY (uri)
);

CREATE TABLE IF NOT EXISTS known_packages (
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    description TEXT,
    homepage TEXT,
    license TEXT,
    source TEXT,
    build TEXT,
    registry TEXT NOT NULL,

    PRIMARY KEY (name, version),
    FOREIGN KEY (registry) REFERENCES registries (uri) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS known_packages_registry ON known_packages (registry);

CREATE TABLE IF NOT EXISTS workspaces (
    name TEXT NOT NULL,

    PRIMARY KEY (name)
);
INSERT INTO workspaces (name) VALUES ('global');

CREATE TABLE IF NOT EXISTS workspace_packages (
    name TEXT NOT NULL,
    version TEXT NOT NULL,
    requested_version TEXT NOT NULL,
    workspace TEXT NOT NULL,

    PRIMARY KEY (name, version, workspace),
    FOREIGN KEY (workspace) REFERENCES workspaces (name) ON DELETE CASCADE
    FOREIGN KEY (name, version) REFERENCES installed_packages (name, version) ON DELETE CASCADE
);