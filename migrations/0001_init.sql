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

    registry TEXT NOT NULL,
    PRIMARY KEY (name, version),
    FOREIGN KEY (registry) REFERENCES registries (name)
);