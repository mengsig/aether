use super::*;

fn sqlite(code: std::ffi::c_int) -> RuntimeError {
    RuntimeError::Store(aether_store::StoreError::Sqlite(
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None),
    ))
}

#[test]
fn a_full_disk_is_waited_out_however_long_it_takes() {
    let full = sqlite(rusqlite::ffi::SQLITE_FULL);
    assert_eq!(next_step(&full, 0), Next::RetryUnwritable);
    assert_eq!(
        next_step(&full, CONTENDED_ATTEMPTS + 10),
        Next::RetryUnwritable
    );
}

#[test]
fn a_locked_database_is_retried_a_bounded_number_of_times() {
    for code in [rusqlite::ffi::SQLITE_BUSY, rusqlite::ffi::SQLITE_LOCKED] {
        let locked = sqlite(code);
        assert_eq!(next_step(&locked, 0), Next::RetryContended);
        assert_eq!(
            next_step(&locked, CONTENDED_ATTEMPTS - 1),
            Next::RetryContended
        );
        assert_eq!(next_step(&locked, CONTENDED_ATTEMPTS), Next::GiveUp);
    }
}

#[test]
fn any_other_error_gives_the_ending_up_at_once() {
    let constraint = sqlite(rusqlite::ffi::SQLITE_CONSTRAINT_UNIQUE);
    assert_eq!(next_step(&constraint, 0), Next::GiveUp);
    let invalid = RuntimeError::Invalid("bad state".into());
    assert_eq!(next_step(&invalid, 0), Next::GiveUp);
}

#[test]
fn a_failure_that_cannot_be_recorded_falls_back_to_recovery_once() {
    let failed = Ending::Fail("the engine crashed".into());
    assert_eq!(fallback(&failed), Some(Ending::Recover));
    assert_eq!(fallback(&Ending::Recover), None, "recovery has no fallback");
}
