use super::*;
use serde_json::json;

fn usage() -> ProviderUsage {
    ProviderUsage::from_engine_payload(
        &json!({
            "primary": {"usedPercent": 38, "windowDurationMins": 300, "resetsAt": 2000000000},
            "secondary": {"usedPercent": 73, "windowDurationMins": 10080}
        }),
        Timestamp::from_timestamp(1999999100, 0).unwrap(),
    )
    .unwrap()
}

#[test]
fn headroom_ages_at_read_time_and_respects_configured_threshold() {
    let usage = usage();
    let at = usage.observed_at + chrono::Duration::seconds(900);
    let headroom = ProviderHeadroom::from_usage(Some(&usage), at, Duration::from_secs(900));
    assert_eq!(headroom.observed_at, Some(usage.observed_at));
    assert_eq!(headroom.age_seconds, Some(900));
    assert!(!headroom.stale);
    assert_eq!(headroom.windows[0].remaining_percent, 62);
    assert_eq!(headroom.windows[0].resets_in_seconds, Some(0));
    assert_eq!(headroom.windows[1].remaining_percent, 27);
    assert_eq!(headroom.windows[1].resets_in_seconds, None);
    assert!(ProviderHeadroom::from_usage(Some(&usage), at, Duration::from_secs(899)).stale);
    assert!(
        ProviderHeadroom::from_usage(
            Some(&usage),
            at + chrono::Duration::milliseconds(1),
            Duration::from_secs(900)
        )
        .stale
    );
}

#[test]
fn unknown_observations_and_clock_skew_do_not_invent_credit() {
    let mut usage = usage();
    let earlier = usage.observed_at - chrono::Duration::seconds(10);
    let unknown = ProviderHeadroom::from_usage(None, earlier, Duration::from_secs(900));
    assert!(unknown.stale && unknown.windows.is_empty());
    assert_eq!(unknown.observed_at, None);
    assert_eq!(unknown.age_seconds, None);
    let headroom = ProviderHeadroom::from_usage(Some(&usage), earlier, Duration::from_secs(900));
    assert_eq!(headroom.age_seconds, Some(0));
    assert_eq!(headroom.windows[0].resets_in_seconds, Some(910));
    usage.windows[0].quota.used_percent = 101;
    let later = earlier + chrono::Duration::days(10);
    let headroom = ProviderHeadroom::from_usage(Some(&usage), later, Duration::from_secs(900));
    assert_eq!(headroom.windows[0].remaining_percent, 0);
    assert_eq!(headroom.windows[0].resets_in_seconds, Some(0));
    assert!(headroom.stale);
    usage.windows.clear();
    assert!(ProviderHeadroom::from_usage(Some(&usage), earlier, Duration::from_secs(900)).stale);
}

#[test]
fn reset_fields_are_optional_and_invalid_values_stay_unknown() {
    for reset in [
        json!(null),
        json!("tomorrow"),
        json!(1.5),
        json!(u64::MAX),
        json!(i64::MAX),
    ] {
        let payload = json!({"primary":{"usedPercent":38,"resetsAt":reset}});
        let usage = ProviderUsage::from_engine_payload(&payload, crate::time::now()).unwrap();
        assert_eq!(usage.windows[0].quota.resets_at, None);
        assert_eq!(usage.windows[0].quota.used_percent, 38);
    }
    let legacy: QuotaWindow = serde_json::from_value(json!({"used_percent":38})).unwrap();
    assert_eq!(legacy.resets_at, None);
    assert_eq!(legacy.window_minutes, None);
}

#[test]
fn human_headroom_includes_utc_reset_time_and_date() {
    let mut quota = usage().windows[0].quota;
    quota.resets_at = Some("2030-01-02T15:30:00Z".parse().unwrap());
    assert_eq!(
        quota.headroom_label("primary"),
        "5h: 62% left, resets 15:30 UTC (2030-01-02)"
    );
    quota.resets_at = None;
    assert_eq!(
        quota.headroom_label("primary"),
        "5h: 62% left, reset unknown"
    );
}
