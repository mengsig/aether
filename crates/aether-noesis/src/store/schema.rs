//! The `noesis.db` schema. Migration numbers are pre-assigned by CONTRACTS.md
//! §1.4: N1 (F1), N2 learner tables (F4), N3 evidence bundles (F6), N4 a
//! session's history.
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

/// N2: the learner model (NOESIS.md §5). It shares only ids with the
/// proposition graph, as plain values: no key, trigger or view joins them.
const N2: &str = r#"
CREATE TABLE concepts (
    id         TEXT PRIMARY KEY CHECK (id LIKE 'k\_%' ESCAPE '\'),
    kind       TEXT NOT NULL CHECK (kind IN ('proposition', 'plan_node', 'topic')),
    ref        TEXT NOT NULL CHECK (length(ref) > 0),
    title      TEXT NOT NULL,
    created_at TEXT NOT NULL,
    UNIQUE (kind, ref)
) STRICT;

-- Append-only: history is never edited, and a correction is a new row.
CREATE TABLE evidence (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    concept         TEXT NOT NULL REFERENCES concepts(id),
    session         TEXT CHECK (session IS NULL OR length(session) > 0),
    at              TEXT NOT NULL,
    kind            TEXT NOT NULL CHECK (kind IN
                        ('probe_answer', 'predict_answer', 'check_answer', 'recall_judged',
                         'practice_conclude', 'exposure', 'misconception_observed',
                         'operator_correction')),
    outcome         TEXT NOT NULL CHECK (outcome IN
                        ('correct', 'incorrect', 'dont_know', 'exposed', 'observed', 'corrected')),
    confidence      INTEGER CHECK (confidence IS NULL OR confidence IN (0, 30, 50, 70, 90, 100)),
    dont_know       INTEGER NOT NULL DEFAULT 0 CHECK (dont_know IN (0, 1)),
    mode            TEXT CHECK (mode IS NULL OR mode IN ('recall', 'discrimination', 'composition')),
    hint_rung       INTEGER CHECK (hint_rung IS NULL OR hint_rung BETWEEN 0 AND 5),
    miss_class      TEXT CHECK (miss_class IS NULL OR miss_class IN
                        ('cue', 'discrimination', 'execution', 'careless', 'structural')),
    note            TEXT,
    corrected_state TEXT CHECK (corrected_state IS NULL OR corrected_state IN
                        ('unseen', 'exposed', 'demonstrated', 'fragile', 'misconception')),
    CHECK (dont_know = (outcome = 'dont_know')),
    CHECK (dont_know = 0 OR confidence IS NULL),
    CHECK ((kind = 'exposure') = (outcome = 'exposed')),
    CHECK ((kind = 'misconception_observed') = (outcome = 'observed')),
    CHECK ((kind = 'operator_correction') = (outcome = 'corrected')),
    CHECK ((kind = 'operator_correction') = (corrected_state IS NOT NULL))
) STRICT;
CREATE INDEX evidence_by_concept ON evidence(concept, id);

CREATE TRIGGER evidence_is_append_only
BEFORE UPDATE ON evidence
BEGIN
    SELECT RAISE(ABORT, 'learner evidence is append-only');
END;

-- Derived from `evidence` by the learner model's rebuild, and by nothing else.
CREATE TABLE concept_state (
    concept         TEXT PRIMARY KEY REFERENCES concepts(id),
    state           TEXT NOT NULL CHECK (state IN
                        ('unseen', 'exposed', 'demonstrated', 'fragile', 'misconception')),
    strength        REAL NOT NULL CHECK (strength BETWEEN 0 AND 1),
    last_success_at TEXT,
    review_due_at   TEXT,
    calibration     REAL CHECK (calibration IS NULL OR calibration BETWEEN 0 AND 1),
    misconceptions  TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(misconceptions)),
    as_of_evidence  INTEGER NOT NULL CHECK (as_of_evidence >= 0),
    derived_at      TEXT NOT NULL,
    CHECK ((last_success_at IS NULL) = (review_due_at IS NULL))
) STRICT;
CREATE INDEX concept_state_by_state ON concept_state(state);
CREATE INDEX concept_state_by_review ON concept_state(review_due_at)
    WHERE review_due_at IS NOT NULL;
"#;

/// N3: evidence bundles (CONTRACTS.md §6.3), one per session at most, kept
/// with it and deleted with it. Additive: N1 and N2 rows are untouched.
const N3: &str = r#"
CREATE TABLE evidence_bundles (
    session       TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
    title         TEXT NOT NULL CHECK (length(title) > 0),
    excerpt_bytes INTEGER NOT NULL CHECK (excerpt_bytes BETWEEN 0 AND 32768),
    created_at    TEXT NOT NULL
) STRICT;

CREATE TABLE evidence_items (
    session  TEXT NOT NULL REFERENCES evidence_bundles(session) ON DELETE CASCADE,
    ord      INTEGER NOT NULL CHECK (ord >= 0),
    ref_kind TEXT NOT NULL CHECK (ref_kind IN
                 ('pr', 'issue', 'item', 'run', 'stage', 'commit', 'file', 'agent')),
    ref_id   TEXT NOT NULL CHECK (length(ref_id) > 0),
    ref_url  TEXT,
    excerpt  TEXT NOT NULL,
    status   TEXT NOT NULL CHECK (status IN ('verified', 'reviewed', 'preliminary')),
    PRIMARY KEY (session, ord)
) STRICT;
"#;

/// N4: a session's history. Every agent that ran it, in order, with the
/// transcript seqs of the inputs Noesis wrote for it; the lesson step each
/// question was posed as; and why the session's last turn stalled. A session
/// from before N4 keeps only its current agent, and an older question's step
/// is inferred from what was stored with it.
const N4: &str = r#"
CREATE TABLE session_agents (
    session    TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    ord        INTEGER NOT NULL CHECK (ord >= 0),
    agent      TEXT NOT NULL CHECK (length(agent) > 0),
    role       TEXT NOT NULL CHECK (role IN ('prober', 'teacher', 'practitioner')),
    internal   TEXT NOT NULL DEFAULT '[]' CHECK (json_valid(internal)),
    started_at TEXT NOT NULL,
    PRIMARY KEY (session, ord),
    UNIQUE (session, agent)
) STRICT;

INSERT INTO session_agents(session, ord, agent, role, started_at)
SELECT id, 0, agent,
       CASE phase WHEN 'probe' THEN 'prober' WHEN 'practice' THEN 'practitioner' ELSE 'teacher' END,
       created_at
FROM sessions;

ALTER TABLE questions ADD COLUMN step TEXT CHECK (step IS NULL OR step IN
    ('measure', 'predict', 'check', 'goal', 'practice'));

UPDATE questions SET step = CASE
    WHEN kind = 'goal' THEN 'goal'
    WHEN collapsed = 1 THEN 'check'
    WHEN (SELECT mode FROM sessions WHERE id = questions.session) = 'practice' THEN 'practice'
    WHEN plan_node IS NOT NULL THEN 'predict'
    ELSE 'measure'
END;

ALTER TABLE sessions ADD COLUMN stalled TEXT CHECK (stalled IS NULL OR json_valid(stalled));
"#;

pub(crate) const MIGRATIONS: &[&str] = &[N1, N2, N3, N4];

/// The schema version this binary writes and reads.
pub const SCHEMA_VERSION: u32 = 4;

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
