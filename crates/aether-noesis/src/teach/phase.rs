//! The session phase machine (T3). The service stores the phase and changes
//! it only through [`advance`]; [`fold`] rebuilds it from the emitted events
//! so the stored value can be checked against the record.

use super::records::{Mode, NoesisEvent, Phase};

/// What can move a session to another phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// A successful, non-refused `noesis.probe_done`.
    ProbeDone,
    /// A question tagged with a plan node was posed.
    TaggedQuestion,
}

pub fn initial(mode: Mode) -> Phase {
    match mode {
        Mode::Teach => Phase::Probe,
        Mode::Practice => Phase::Practice,
    }
}

/// The phase after `trigger`, or the same phase when it does not apply.
/// A plan alone never advances a session.
pub fn advance(phase: Phase, trigger: Trigger) -> Phase {
    match (phase, trigger) {
        (Phase::Probe, Trigger::ProbeDone) => Phase::Plan,
        (Phase::Plan, Trigger::TaggedQuestion) => Phase::Teach,
        (phase, _) => phase,
    }
}

/// Rebuild a session's phase from its events, in order. It reads the facts
/// (creation, the probe hand-off, tagged questions), not the stored phase.
pub fn fold(session: &str, events: &[NoesisEvent]) -> Option<Phase> {
    let mut phase = None;
    for event in events {
        phase = match (phase, event) {
            (None, NoesisEvent::SessionCreated { session: view }) if view.id == session => {
                Some(initial(view.mode))
            }
            (
                Some(current),
                NoesisEvent::PhaseChanged {
                    session: id,
                    from: Phase::Probe,
                    to: Phase::Plan,
                },
            ) if id == session => Some(advance(current, Trigger::ProbeDone)),
            (
                Some(current),
                NoesisEvent::QuestionPosed {
                    session: id,
                    question,
                },
            ) if id == session && question.plan_node.is_some() => {
                Some(advance(current, Trigger::TaggedQuestion))
            }
            (phase, _) => phase,
        };
    }
    phase
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_triggers_move_a_session_and_practice_never_moves() {
        assert_eq!(advance(Phase::Probe, Trigger::ProbeDone), Phase::Plan);
        assert_eq!(advance(Phase::Plan, Trigger::TaggedQuestion), Phase::Teach);
        assert_eq!(advance(Phase::Probe, Trigger::TaggedQuestion), Phase::Probe);
        assert_eq!(advance(Phase::Plan, Trigger::ProbeDone), Phase::Plan);
        assert_eq!(advance(Phase::Teach, Trigger::ProbeDone), Phase::Teach);
        for trigger in [Trigger::ProbeDone, Trigger::TaggedQuestion] {
            assert_eq!(advance(Phase::Practice, trigger), Phase::Practice);
        }
        assert_eq!(fold("ns_x", &[]), None);
    }
}
