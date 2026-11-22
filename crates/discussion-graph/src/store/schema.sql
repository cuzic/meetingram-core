CREATE TABLE IF NOT EXISTS graph_utterances (
    session_id TEXT NOT NULL,
    uid TEXT NOT NULL,
    row_id INTEGER NOT NULL,
    track TEXT,
    speaker_label TEXT NOT NULL,
    start_ms INTEGER,
    end_ms INTEGER,
    extracted_rev INTEGER,
    PRIMARY KEY (session_id, uid),
    UNIQUE (session_id, row_id)
);

CREATE TABLE IF NOT EXISTS graph_nodes (
    session_id TEXT NOT NULL,
    nid TEXT NOT NULL,
    kind TEXT NOT NULL,
    text TEXT NOT NULL,
    status TEXT NOT NULL,
    merged_into TEXT,
    confidence REAL NOT NULL,
    review TEXT NOT NULL,
    created_rev INTEGER NOT NULL,
    updated_rev INTEGER NOT NULL,
    PRIMARY KEY (session_id, nid)
);

CREATE TABLE IF NOT EXISTS graph_edges (
    session_id TEXT NOT NULL,
    eid TEXT NOT NULL,
    src TEXT NOT NULL,
    dst TEXT NOT NULL,
    kind TEXT NOT NULL,
    confidence REAL NOT NULL,
    review TEXT NOT NULL,
    created_rev INTEGER NOT NULL,
    PRIMARY KEY (session_id, eid),
    UNIQUE (session_id, src, dst, kind)
);

CREATE TABLE IF NOT EXISTS graph_grounds (
    session_id TEXT NOT NULL,
    target TEXT NOT NULL,
    uid TEXT NOT NULL,
    act TEXT,
    PRIMARY KEY (session_id, target, uid)
);

CREATE TABLE IF NOT EXISTS graph_patches (
    session_id TEXT NOT NULL,
    rev INTEGER NOT NULL,
    source TEXT NOT NULL,
    provider_model TEXT,
    ops_json TEXT NOT NULL,
    rejected_ops INTEGER NOT NULL,
    created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
    PRIMARY KEY (session_id, rev)
);

CREATE TABLE IF NOT EXISTS graph_consolidations (
    session_id TEXT NOT NULL PRIMARY KEY,
    rev INTEGER NOT NULL
);
