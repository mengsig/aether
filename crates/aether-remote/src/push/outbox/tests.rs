use aether_protocol::remote::AttentionKind;

use super::*;
use crate::push::notice::test_notice;

const MS: Duration = Duration::from_millis(1);

/// A notice on `topic`, told apart by `seq`.
fn notice(topic: &str, seq: u64) -> Arc<Notice> {
    let mut notice = test_notice("inst_test", aether_core::time::now());
    notice.topic = topic.to_owned();
    notice.payload.seq = seq;
    notice.payload.kind = AttentionKind::AgentFailed;
    Arc::new(notice)
}

fn retry(retry_after: Option<Duration>) -> Settled {
    Settled::Retry {
        retry_after,
        jitter: 1.0,
    }
}

#[test]
fn backoff_doubles_within_its_bounds() {
    assert_eq!(backoff(1, 1.0), BACKOFF_BASE);
    assert_eq!(backoff(1, 0.0), BACKOFF_BASE / 2);
    assert_eq!(backoff(2, 1.0), BACKOFF_BASE * 2);
    assert_eq!(backoff(4, 1.0), BACKOFF_BASE * 8);
    let mut previous = Duration::ZERO;
    for failures in 1..=64 {
        let (low, high) = (backoff(failures, 0.0), backoff(failures, 1.0));
        assert!(
            low * 2 == high && high <= BACKOFF_MAX,
            "{failures}: {low:?}..{high:?}"
        );
        assert!(high >= previous, "the step never shrinks");
        let between = backoff(failures, 0.37);
        assert!(low <= between && between <= high, "{failures}");
        previous = high;
    }
    assert_eq!(backoff(u32::MAX, 1.0), BACKOFF_MAX);
}

#[test]
fn a_notice_waits_out_the_settle_delay() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    assert_eq!(outbox.offer(device, notice("a", 1), start), Offered::Queued);
    assert_eq!(outbox.next_due(), Some(start + SETTLE));
    assert!(outbox.take_due(start + SETTLE - MS).is_none());

    let job = outbox.take_due(start + SETTLE).expect("due");
    assert_eq!((job.device, job.notice.payload.seq), (device, 1));
    assert!(
        outbox.take_due(start + SETTLE).is_none(),
        "it is being sent"
    );
    assert_eq!(outbox.next_due(), None);
}

#[test]
fn a_burst_on_one_topic_is_one_notice_and_the_newest_wins() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    for seq in 2..=5 {
        assert_eq!(
            outbox.offer(device, notice("a", seq), start + MS * 10),
            Offered::Replaced
        );
    }
    assert_eq!(outbox.len(), 1);
    assert_eq!(
        outbox.next_due(),
        Some(start + SETTLE),
        "replacing does not postpone"
    );
    let job = outbox.take_due(start + SETTLE).expect("due");
    assert_eq!(job.notice.payload.seq, 5);
}

#[test]
fn after_a_send_the_topic_stays_quiet_and_then_sends_the_latest() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    let job = outbox.take_due(start + SETTLE).expect("due");
    // Two more arrive while the first is in flight, one after it landed.
    outbox.offer(device, notice("a", 2), start + SETTLE);
    outbox.offer(device, notice("a", 3), start + SETTLE);
    let sent = start + SETTLE + MS * 50;
    outbox.settle(job, Settled::Delivered, sent);
    outbox.offer(device, notice("a", 4), sent + MS);

    assert_eq!(outbox.next_due(), Some(sent + QUIET));
    assert!(outbox.take_due(sent + QUIET - MS).is_none());
    let job = outbox
        .take_due(sent + QUIET)
        .expect("due after the quiet window");
    assert_eq!(job.notice.payload.seq, 4);
    outbox.settle(job, Settled::Delivered, sent + QUIET);

    // Another topic of the same device is not held back by the window.
    outbox.offer(device, notice("b", 9), sent + QUIET);
    let job = outbox
        .take_due(sent + QUIET + SETTLE)
        .expect("another topic");
    assert_eq!(job.notice.payload.seq, 9);
}

#[test]
fn a_quiet_window_is_forgotten_once_it_passed() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    let job = outbox.take_due(start + SETTLE).expect("due");
    outbox.settle(job, Settled::Delivered, start + SETTLE);
    assert_eq!(outbox.len(), 1, "the window is remembered");

    let later = start + SETTLE + QUIET;
    assert!(outbox.take_due(later).is_none());
    assert_eq!(outbox.len(), 0);
    assert!(
        outbox.devices.is_empty(),
        "nothing is kept for an idle device"
    );
    outbox.offer(device, notice("a", 2), later);
    assert_eq!(outbox.next_due(), Some(later + SETTLE));
}

#[test]
fn one_device_sends_one_at_a_time_and_devices_do_not_wait_on_each_other() {
    let mut outbox = Outbox::default();
    let (slow, fast) = (DeviceId::new(), DeviceId::new());
    let start = Instant::now();
    outbox.offer(slow, notice("a", 1), start);
    outbox.offer(slow, notice("b", 2), start);
    outbox.offer(fast, notice("a", 3), start);
    let due = start + SETTLE;

    let first = outbox.take_due(due).expect("one");
    let second = outbox.take_due(due).expect("two");
    assert_ne!(first.device, second.device);
    assert!(
        outbox.take_due(due).is_none(),
        "the busy device's other topic waits"
    );

    let of_slow = if first.device == slow { first } else { second };
    outbox.settle(of_slow, Settled::Delivered, due);
    let third = outbox.take_due(due).expect("the other topic");
    assert_eq!(third.device, slow);
}

#[test]
fn failures_back_off_per_device_and_success_resets() {
    let mut outbox = Outbox::default();
    let (down, up) = (DeviceId::new(), DeviceId::new());
    let mut now = Instant::now();
    outbox.offer(down, notice("a", 1), now);
    outbox.offer(down, notice("b", 2), now);
    outbox.offer(up, notice("a", 3), now + MS);
    now += SETTLE;
    for failures in 1..=4u32 {
        let job = outbox.take_due(now).expect("due");
        assert_eq!(job.device, down);
        outbox.settle(job, retry(None), now);
        if failures == 1 {
            let other = outbox
                .take_due(now + MS)
                .expect("another device is not held");
            assert_eq!(other.device, up);
            outbox.settle(other, Settled::Delivered, now + MS);
        }
        let wait = backoff(failures, 1.0);
        assert_eq!(outbox.next_due(), Some(now + wait), "failure {failures}");
        assert!(
            outbox.take_due(now + wait - MS).is_none(),
            "every topic of the device waits"
        );
        now += wait;
    }

    let job = outbox.take_due(now).expect("due");
    outbox.settle(job, Settled::Delivered, now);
    let job = outbox.take_due(now).expect("the other topic goes at once");
    outbox.settle(job, retry(None), now);
    assert_eq!(
        outbox.next_due(),
        Some(now + backoff(1, 1.0)),
        "the count started over"
    );
}

#[test]
fn retry_after_extends_the_hold_but_never_past_the_lifetime() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    let due = start + SETTLE;

    let job = outbox.take_due(due).expect("due");
    outbox.settle(job, retry(Some(Duration::from_secs(120))), due);
    assert_eq!(outbox.next_due(), Some(due + Duration::from_secs(120)));

    let again = due + Duration::from_secs(120);
    let job = outbox.take_due(again).expect("due");
    outbox.settle(job, retry(Some(Duration::from_secs(1))), again);
    assert_eq!(
        outbox.next_due(),
        Some(again + backoff(2, 1.0)),
        "a short hint does not undercut"
    );

    let third = again + backoff(2, 1.0);
    let job = outbox.take_due(third).expect("due");
    outbox.settle(job, retry(Some(Duration::from_secs(86_400))), third);
    assert_eq!(
        outbox.next_due(),
        Some(third + LIFETIME),
        "a day is cut to the lifetime"
    );
}

#[test]
fn a_notice_is_given_up_after_its_lifetime() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    let mut now = start + SETTLE;
    let mut attempts = 0;
    while let Some(job) = outbox.take_due(now) {
        attempts += 1;
        assert!(now < start + LIFETIME, "nothing is sent past the lifetime");
        outbox.settle(job, retry(None), now);
        now = outbox.next_due().expect("a retry is scheduled");
    }
    // 5 s doubling to the 15 minute cap fits ten retries into an hour.
    assert_eq!(attempts, 11);
    assert_eq!(outbox.len(), 0, "the expired notice is gone");
    assert_eq!(outbox.next_due(), None);
}

#[test]
fn a_newer_notice_survives_the_failure_of_the_one_it_replaced() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    let job = outbox.take_due(start + SETTLE).expect("due");
    outbox.offer(device, notice("a", 2), start + SETTLE);
    outbox.settle(job, retry(None), start + SETTLE);

    let job = outbox.take_due(start + SETTLE + BACKOFF_BASE).expect("due");
    assert_eq!(job.notice.payload.seq, 2);
    outbox.settle(job, Settled::Dropped, start + SETTLE + BACKOFF_BASE);
    assert_eq!(outbox.next_due(), None, "a dropped notice is not repeated");
}

#[test]
fn a_dead_subscription_takes_everything_for_the_device_with_it() {
    let mut outbox = Outbox::default();
    let (dead, alive) = (DeviceId::new(), DeviceId::new());
    let start = Instant::now();
    for topic in ["a", "b", "c"] {
        outbox.offer(dead, notice(topic, 1), start);
    }
    outbox.offer(alive, notice("a", 2), start);
    let job = loop {
        let job = outbox.take_due(start + SETTLE).expect("due");
        if job.device == dead {
            break job;
        }
    };
    outbox.settle(job, Settled::Gone, start + SETTLE);
    assert_eq!(outbox.len(), 1);
    assert!(!outbox.devices.contains_key(&dead));
}

#[test]
fn a_notice_for_a_replaced_subscription_is_due_again_at_once_and_costs_nothing() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    outbox.offer(device, notice("b", 2), start);
    let sent = start + SETTLE;
    let job = outbox.take_due(sent).expect("due");
    let seq = job.notice.payload.seq;
    outbox.settle(job, Settled::Resend, sent);

    assert_eq!(outbox.len(), 2, "nothing queued for the device was dropped");
    assert_eq!(
        outbox.devices[&device].failures, 0,
        "no failure was counted"
    );
    assert_eq!(outbox.next_due(), Some(sent), "and there is no hold");
    let seqs: Vec<u64> = std::iter::from_fn(|| {
        let job = outbox.take_due(sent)?;
        let seq = job.notice.payload.seq;
        outbox.settle(job, Settled::Delivered, sent);
        Some(seq)
    })
    .collect();
    assert_eq!(seqs.len(), 2, "both leave, the one sent again included");
    assert!(seqs.contains(&seq));
}

#[test]
fn forgetting_or_clearing_mid_flight_is_safe() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let start = Instant::now();
    outbox.offer(device, notice("a", 1), start);
    outbox.offer(device, notice("b", 2), start);
    let job = outbox.take_due(start + SETTLE).expect("due");
    outbox.forget(device);
    assert_eq!(outbox.len(), 0);
    outbox.settle(job, retry(None), start + SETTLE);
    assert_eq!(
        outbox.len(),
        0,
        "a late result does not bring the device back"
    );

    outbox.offer(device, notice("a", 3), start + SETTLE);
    let job = outbox.take_due(start + SETTLE * 2).expect("due");
    outbox.clear();
    outbox.settle(job, Settled::Delivered, start + SETTLE * 2);
    assert_eq!((outbox.len(), outbox.devices.len()), (0, 0));
}

#[test]
fn a_decided_approval_is_retracted_while_it_waits() {
    let mut outbox = Outbox::default();
    let device = DeviceId::new();
    let approval = ApprovalId::new();
    let start = Instant::now();
    let mut asking = (*notice("ask", 1)).clone();
    asking.payload.approval = Some(approval);
    outbox.offer(device, Arc::new(asking), start);
    outbox.offer(device, notice("status", 2), start);

    outbox.retract(ApprovalId::new());
    outbox.retract(approval);
    let job = outbox
        .take_due(start + SETTLE)
        .expect("the status notice remains");
    assert_eq!(job.notice.payload.seq, 2);
    assert!(outbox.take_due(start + SETTLE).is_none());
}

#[test]
fn the_outbox_is_bounded_and_drops_the_oldest() {
    let mut outbox = Outbox::default();
    let start = Instant::now();
    let devices: Vec<DeviceId> = (0..CAPACITY / DEVICE_CAPACITY)
        .map(|_| DeviceId::new())
        .collect();
    let mut tick = 0u32;
    for device in &devices {
        for topic in 0..DEVICE_CAPACITY {
            tick += 1;
            let offered = outbox.offer(*device, notice(&format!("t{topic}"), 1), start + MS * tick);
            assert_eq!(offered, Offered::Queued);
        }
    }
    assert_eq!(outbox.len(), CAPACITY);

    // One more for a full device drops that device's oldest, nobody else's.
    let offered = outbox.offer(devices[1], notice("extra", 2), start + MS * 1000);
    assert_eq!(offered, Offered::Evicted);
    assert_eq!(outbox.len(), CAPACITY);
    assert!(!outbox.slots.contains_key(&(devices[1], "t0".to_owned())));
    assert!(outbox.slots.contains_key(&(devices[0], "t0".to_owned())));

    // A new device takes the globally oldest slot.
    let newcomer = DeviceId::new();
    assert_eq!(
        outbox.offer(newcomer, notice("a", 3), start + MS * 1001),
        Offered::Evicted
    );
    assert_eq!(outbox.len(), CAPACITY);
    assert!(!outbox.slots.contains_key(&(devices[0], "t0".to_owned())));

    for round in 0..10_000u32 {
        outbox.offer(
            DeviceId::new(),
            notice("flood", 4),
            start + MS * (2000 + round),
        );
        assert!(outbox.len() <= CAPACITY);
    }
}
