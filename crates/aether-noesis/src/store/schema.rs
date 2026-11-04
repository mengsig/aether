//! The `noesis.db` schema. Migration numbers are pre-assigned by CONTRACTS.md
//! §1.4: N1 (this task), N2 learner tables (F4), N3 evidence bundles (F6).
//! Each later task appends exactly its own entry.

use rusqlite::{Connection, OptionalExtension};

use super::StoreError;

/// N1: sessions, the messages index, questions and problems, plans, the
/// graph ledgers, coverage and jobs. Sealed values live only here.
const N1: &str = r#"
CREATE TABLE sessions (
    id         TEXT PRIMARY KEY CHECK (id LIKE 'ns\_%' ESCAPE '\'),
    goal       TEXT NOT NULL CHECK (length(goal) BETWEEN 1 AND 2000),
    mode       TEXT NOT NULL CHECK (mode IN ('teach', 'practice')),
    phase      TEXT NOT NULL CHECK (phase IN ('probe', 'plan', 'teach', 'practice')),
    agent      TEXT NOT NULL CHECK (length(agent) > 0),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;

-- Where each teaching message sits in the session agent's Aether transcript.
CREATE TABLE session_messages (
    session     TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    seq         INTEGER NOT NULL CHECK (seq >= 0),
    message_ref TEXT NOT NULL,
    role        TEXT NOT NULL CHECK (role IN ('learner', 'tutor')),
    tokens      INTEGER NOT NULL DEFAULT 0 CHECK (tokens >= 0),
    at          TEXT NOT NULL,
    PRIMARY KEY (session, seq),
    UNIQUE (session, message_ref)
) STRICT;

CREATE TABLE plans (
    session    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    revision   INTEGER NOT NULL CHECK (revision >= 0),
    tree       TEXT NOT NULL CHECK (json_valid(tree)),
    created_at TEXT NOT NULL,
    PRIMARY KEY (session, revision)
) STRICT;

CREATE TABLE questions (
    id          TEXT PRIMARY KEY CHECK (id LIKE 'q\_%' ESCAPE '\'),
    session     TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('choice', 'recall', 'goal')),
    prompt      TEXT NOT NULL,
    options     TEXT CHECK (options IS NULL OR json_valid(options)),
    multi       INTEGER NOT NULL DEFAULT 0 CHECK (multi IN (0, 1)),
    collapsed   INTEGER NOT NULL DEFAULT 0 CHECK (collapsed IN (0, 1)),
    plan_node   TEXT,
    concepts    TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(concepts)),
    answer_key  TEXT NOT NULL CHECK (json_valid(answer_key)),
    explanation TEXT,
    posed_at    TEXT NOT NULL,
    answer      TEXT CHECK (answer IS NULL OR json_valid(answer)),
    correct     INTEGER CHECK (correct IS NULL OR correct IN (0, 1)),
    answered_at TEXT,
    CHECK ((answer IS NULL) = (answered_at IS NULL))
) STRICT;
CREATE INDEX questions_by_session ON questions(session, posed_at);
CREATE UNIQUE INDEX questions_one_open ON questions(session) WHERE answered_at IS NULL;

CREATE TABLE problems (
    id           TEXT PRIMARY KEY CHECK (length(id) > 0),
    session      TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    statement    TEXT NOT NULL,
    mode         TEXT NOT NULL CHECK (mode IN ('recall', 'discrimination', 'composition')),
    targets      TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(targets)),
    solution     TEXT NOT NULL,
    posed_at     TEXT NOT NULL,
    outcome      TEXT CHECK (outcome IS NULL OR outcome IN
                     ('solved', 'solved_with_hints', 'moved_on', 'surrendered', 'dismissed')),
    rung         INTEGER CHECK (rung IS NULL OR rung BETWEEN 0 AND 5),
    miss_class   TEXT CHECK (miss_class IS NULL OR miss_class IN
                     ('cue', 'discrimination', 'execution', 'careless', 'structural')),
    retest       INTEGER CHECK (retest IS NULL OR retest IN (0, 1)),
    concluded_at TEXT,
    CHECK ((outcome IS NULL) = (concluded_at IS NULL))
) STRICT;
CREATE INDEX problems_by_session ON problems(session, posed_at);
CREATE UNIQUE INDEX problems_one_active ON problems(session) WHERE concluded_at IS NULL;

CREATE TABLE sources (
    id          TEXT PRIMARY KEY CHECK (id LIKE 's\_%' ESCAPE '\'),
    name        TEXT NOT NULL CHECK (length(name) BETWEEN 1 AND 200),
    artifact    TEXT NOT NULL,
    imported_at TEXT NOT NULL
) STRICT;

CREATE TABLE propositions (
    id         TEXT PRIMARY KEY CHECK (id LIKE 'p\_%' ESCAPE '\'),
    name       TEXT NOT NULL,
    statement  TEXT NOT NULL,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE proposition_aliases (
    proposition TEXT NOT NULL REFERENCES propositions(id) ON DELETE CASCADE,
    alias       TEXT NOT NULL,
    PRIMARY KEY (proposition, alias)
) STRICT;

-- `source` is a session or a source id; the graph outlives both, so no key.
CREATE TABLE proposition_provenance (
    proposition TEXT NOT NULL REFERENCES propositions(id) ON DELETE CASCADE,
    source      TEXT NOT NULL,
    message_ref TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (proposition, source, message_ref)
) STRICT;
CREATE INDEX proposition_provenance_by_source ON proposition_provenance(source);

CREATE TABLE chains (
    id         TEXT PRIMARY KEY CHECK (id LIKE 'c\_%' ESCAPE '\'),
    conclusion TEXT NOT NULL REFERENCES propositions(id) ON DELETE CASCADE,
    created_at TEXT NOT NULL
) STRICT;

CREATE TABLE chain_premises (
    chain   TEXT NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    premise TEXT NOT NULL REFERENCES propositions(id) ON DELETE CASCADE,
    PRIMARY KEY (chain, premise)
) STRICT;
CREATE INDEX chain_premises_by_premise ON chain_premises(premise);

CREATE TABLE chain_provenance (
    chain       TEXT NOT NULL REFERENCES chains(id) ON DELETE CASCADE,
    source      TEXT NOT NULL,
    message_ref TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (chain, source, message_ref)
) STRICT;
CREATE INDEX chain_provenance_by_source ON chain_provenance(source);

-- A chain without one of its premises is no longer a valid inference.
CREATE TRIGGER propositions_drop_premised_chains
BEFORE DELETE ON propositions
BEGIN
    DELETE FROM chains WHERE id IN (SELECT chain FROM chain_premises WHERE premise = OLD.id);
END;

CREATE TABLE clusters (
    id      TEXT PRIMARY KEY CHECK (length(id) > 0),
    session TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE
) STRICT;

CREATE TABLE cluster_members (
    cluster     TEXT NOT NULL REFERENCES clusters(id) ON DELETE CASCADE,
    proposition TEXT NOT NULL REFERENCES propositions(id) ON DELETE CASCADE,
    PRIMARY KEY (cluster, proposition)
) STRICT;

CREATE TABLE coverage (
    source      TEXT NOT NULL,
    message_ref TEXT NOT NULL,
    fingerprint TEXT NOT NULL CHECK (length(fingerprint) = 64),
    tokens      INTEGER NOT NULL CHECK (tokens >= 0),
    at          TEXT NOT NULL,
    PRIMARY KEY (source, message_ref)
) STRICT;

CREATE TABLE jobs (
    id         TEXT PRIMARY KEY CHECK (id LIKE 'j\_%' ESCAPE '\'),
    kind       TEXT NOT NULL CHECK (kind IN
                   ('extract_session', 'extract_source', 'extract_engineering')),
    target     TEXT NOT NULL,
    state      TEXT NOT NULL CHECK (state IN
                   ('queued', 'running', 'merging', 'succeeded', 'failed', 'cancelled')),
    stats      TEXT CHECK (stats IS NULL OR json_valid(stats)),
    error      TEXT,
    usage      TEXT NOT NULL DEFAULT '{}' CHECK (json_valid(usage)),
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
) STRICT;
CREATE INDEX jobs_by_state ON jobs(state, created_at);
CREATE INDEX jobs_by_target ON jobs(target);
"#;

pub(crate) const MIGRATIONS: &[&str] = &[N1];

/// The schema version this binary writes and reads.
pub const SCHEMA_VERSION: u32 = 1;

const _: () = assert!(MIGRATIONS.len() == SCHEMA_VERSION as usize);

pub(crate) fn current_version(conn: &Connection) -> Result<Option<u32>, StoreError> {
    let stamp: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .optional()?;
    let Some(raw) = stamp else {
        return Ok(None);
    };
    let version = raw
        .parse()
        .map_err(|_| StoreError::Corrupt(format!("schema_version is not a number: `{raw}`")))?;
    Ok(Some(version))
}

/// Create or upgrade the schema in one transaction, so an interrupted upgrade
/// leaves the version it started from. Refuses a store from a newer binary.
pub(crate) fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL) STRICT;",
    )?;
    let found = current_version(conn)?.unwrap_or(0);
    if found > SCHEMA_VERSION {
        return Err(StoreError::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found == SCHEMA_VERSION {
        return Ok(());
    }
    let tx = conn.unchecked_transaction()?;
    for statements in &MIGRATIONS[found as usize..] {
        tx.execute_batch(statements)?;
    }
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('schema_version', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [SCHEMA_VERSION.to_string()],
    )?;
    tx.commit()?;
    debug_assert_eq!(current_version(conn)?, Some(SCHEMA_VERSION));
    Ok(())
}
