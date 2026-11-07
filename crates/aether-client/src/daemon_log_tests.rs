use super::*;
use std::time::Duration;

fn now() -> DateTime<Utc> {
    DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
        .unwrap()
        .with_timezone(&Utc)
}

fn slow_line(minutes_ago: i64, section: &str, elapsed_ms: u64) -> String {
    let at = (now() - TimeDelta::minutes(minutes_ago)).format("%Y-%m-%dT%H:%M:%S%.6fZ");
    format!(
        "{at}  WARN aether_runtime::commands::status: slow status section \
         section=\"{section}\" elapsed_ms={elapsed_ms}\n"
    )
}

fn idle_host() -> Option<&'static str> {
    None
}

const OVERLOADED: &str = "this host is overloaded: load 69.0 on 12 cores";

fn said(queries: Option<&str>, host: Option<&str>) -> String {
    crate::timed_out_message("status", Duration::from_secs(60), queries, host)
}

#[test]
fn the_slowest_recent_section_is_named_with_its_time() {
    let tail = slow_line(3, "usage_total", 2_400)
        + &slow_line(2, "runs", 141_000)
        + &slow_line(1, "mcp", 5_000);
    let slowest = slowest_recent(&tail, now()).expect("slow sections present");
    assert_eq!(slowest.name, "runs");
    assert_eq!(slowest.elapsed_ms, 141_000);
    assert_eq!(
        describe(&slowest),
        "its own queries are slow: status section `runs` took 141.0 s"
    );
}

#[test]
fn slow_sections_on_an_idle_host_blame_only_the_queries() {
    let tail = slow_line(1, "runs", 141_000);
    let queries = slowest_recent(&tail, now()).map(|s| describe(&s));
    let message = said(queries.as_deref(), idle_host());
    assert!(
        message.contains(
            "though it is still connected; its own queries are slow: status section `runs` \
             took 141.0 s, the likely cause; its log"
        ),
        "{message}"
    );
    assert!(!message.contains("overloaded"), "{message}");
}

#[test]
fn an_overloaded_host_without_slow_sections_blames_only_the_host() {
    let tail = "2026-11-06T11:59:00.000000Z  INFO aether_daemon: started\n";
    assert_eq!(slowest_recent(tail, now()), None);
    let message = said(None, Some(OVERLOADED));
    assert!(
        message.contains(&format!("{OVERLOADED}, the likely cause; its log")),
        "{message}"
    );
    assert!(!message.contains("its own queries"), "{message}");
}

#[test]
fn both_signals_are_named_together() {
    let queries = describe(&SlowSection {
        name: "runs".into(),
        elapsed_ms: 141_000,
    });
    let message = said(Some(&queries), Some(OVERLOADED));
    assert!(
        message.contains(&format!(
            "{queries} and {OVERLOADED}, the likely causes; its log"
        )),
        "{message}"
    );
}

#[test]
fn with_no_signal_the_message_is_the_plain_one() {
    assert_eq!(
        said(None, None),
        "the daemon did not answer `status` within 60s, though it is still connected; its log \
         (`logs/aetherd.log` in its data directory) names each request slower than 2s and what \
         it waited on"
    );
}

#[test]
fn lines_older_than_the_window_are_ignored() {
    let tail = slow_line(11, "runs", 141_000) + &slow_line(9, "mcp", 3_000);
    let slowest = slowest_recent(&tail, now()).expect("the recent one");
    assert_eq!((slowest.name.as_str(), slowest.elapsed_ms), ("mcp", 3_000));
    assert_eq!(slowest_recent(&slow_line(30, "runs", 9_000), now()), None);
}

#[test]
fn malformed_lines_are_ignored() {
    let at = "2026-11-06T11:59:00.000000Z";
    let tail = [
        "not a log line at all".to_owned(),
        String::new(),
        format!("{at}  WARN x: slow status section section=\"runs\" elapsed_ms=fast"),
        format!("{at}  WARN x: slow status section section=\"runs\""),
        format!("{at}  WARN x: slow status section elapsed_ms=5000"),
        format!("{at}  WARN x: slow status section section=\"\" elapsed_ms=5000"),
        "yesterday  WARN x: slow status section section=\"runs\" elapsed_ms=5000".to_owned(),
        format!("{at}  WARN x: some other warning section=\"runs\" elapsed_ms=5000"),
    ]
    .join("\n");
    assert_eq!(slowest_recent(&tail, now()), None);
}

#[test]
fn a_line_cut_by_the_tail_start_is_dropped() {
    let whole = slow_line(1, "runs", 7_000);
    let cut = &whole[whole.find("elapsed_ms").unwrap()..];
    let text = format!("{cut}{}", slow_line(1, "mcp", 3_000));
    assert_eq!(drop_first_line(&text), slow_line(1, "mcp", 3_000));
    assert_eq!(drop_first_line("no newline at all"), "");
    let slowest = slowest_recent(&text, now()).expect("the whole line survives");
    assert_eq!(slowest.name, "mcp");
}

#[test]
fn a_big_log_is_read_only_from_its_tail() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("events.0.log");
    let old = slow_line(1, "runs", 90_000);
    let filler = "2026-11-06T11:00:00.000000Z  INFO x: padding padding padding\n";
    let mut text = old.clone();
    while (text.len() as u64) < TAIL_BYTES * 2 {
        text.push_str(filler);
    }
    text.push_str(&slow_line(1, "mcp", 4_000));
    std::fs::write(&path, &text).unwrap();
    let tail = read_tail(&path).expect("readable");
    assert!(tail.len() as u64 <= TAIL_BYTES);
    let slowest = slowest_recent(&tail, now()).expect("the last line");
    assert_eq!(slowest.name, "mcp", "the early line is outside the tail");
    assert!(
        tail.starts_with("2026-11-06T11:00"),
        "starts on a whole line"
    );
}

#[test]
fn a_missing_log_is_an_error_the_caller_can_tell_apart() {
    let dir = tempfile::tempdir().unwrap();
    let error = read_tail(&dir.path().join("absent.log")).expect_err("no such file");
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

/// The daemon's real subscriber setup, so the parser tracks its format.
#[test]
fn a_line_written_by_the_daemons_subscriber_parses() {
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Sink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let sink = Sink::default();
    let writer = sink.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let (section, elapsed_ms) = ("runs", 141_000u64);
        tracing::warn!(section, elapsed_ms, "slow status section");
    });
    let line = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
    let slowest = slowest_recent(&line, Utc::now()).unwrap_or_else(|| panic!("unparsed: {line}"));
    assert_eq!(
        (slowest.name.as_str(), slowest.elapsed_ms),
        ("runs", 141_000)
    );
}
