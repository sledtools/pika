// Pure decision logic for evolution publish callbacks.
//
// Extracted from the GroupEvolutionPublished handler so the state machine can be
// tested without AppCore, MDK, nostr client, or tokio runtime.

use crate::updates::GroupEvolutionOp;
use nostr_sdk::prelude::EventId;

/// Snapshot of evolution-related state needed for handler decisions.
pub(super) struct EvolutionHandlerInput {
    pub current_session_gen: u64,
    /// (event_id, toasted) from pending_evolutions for this group, if present.
    pub pending_event: Option<(EventId, bool)>,
}

/// Effects produced by the evolution publish handler (pure decision logic).
///
/// The caller (handle_internal) applies these effects to AppCore state.
/// MDK calls, network I/O, and UI side effects stay in the caller.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct EvolutionHandlerEffects {
    /// Callback is from a stale session; ignore entirely.
    pub ignore: bool,
    /// Remove this group from evolution_publish_in_flight.
    pub remove_in_flight: bool,
    /// Remove this group from pending_evolutions.
    pub remove_pending: bool,
    /// Remove this group from pending_self_updates.
    pub remove_self_update: bool,
    /// Attempt merge_pending_commit for this group.
    pub attempt_merge: bool,
    /// Send welcome rumors to added peers.
    pub send_welcomes: bool,
    /// Clear creating_chat busy flag.
    pub clear_busy: bool,
    /// Refresh all groups from storage.
    pub refresh_storage: bool,
    /// Call drain_pending_evolutions after applying other effects.
    pub schedule_drain: bool,
    /// Toast this message to the user.
    pub toast: Option<String>,
    /// Mark the pending_evolutions entry as toasted.
    pub mark_toasted: bool,
}

impl EvolutionHandlerEffects {
    fn empty() -> Self {
        Self {
            ignore: false,
            remove_in_flight: false,
            remove_pending: false,
            remove_self_update: false,
            attempt_merge: false,
            send_welcomes: false,
            clear_busy: false,
            refresh_storage: false,
            schedule_drain: false,
            toast: None,
            mark_toasted: false,
        }
    }
}

/// Pure decision function for the GroupEvolutionPublished callback.
///
/// Takes the current state snapshot and callback fields, returns the set of
/// effects to apply. No side effects, no borrows on AppCore.
pub(super) fn evaluate_evolution_callback(
    state: &EvolutionHandlerInput,
    callback_session_gen: u64,
    callback_event_id: EventId,
    op: &GroupEvolutionOp,
    ok: bool,
    error: Option<&str>,
    has_welcomes: bool,
) -> EvolutionHandlerEffects {
    let mut fx = EvolutionHandlerEffects::empty();

    if callback_session_gen != state.current_session_gen {
        fx.ignore = true;
        return fx;
    }

    let current_event_matches = state
        .pending_event
        .map(|(id, _)| id == callback_event_id)
        .unwrap_or(true);

    fx.remove_in_flight = true;

    if !ok {
        if matches!(op, GroupEvolutionOp::SelfUpdate) {
            if !current_event_matches {
                fx.schedule_drain = true;
            }
            return fx;
        }
        // Standard op failure: toast once per event, keep queued for retry.
        if let Some((id, toasted)) = state.pending_event {
            if id == callback_event_id && !toasted {
                fx.mark_toasted = true;
                fx.toast = Some(format!(
                    "Group update failed: {}",
                    error.unwrap_or("unknown")
                ));
            }
        }
        if current_event_matches {
            fx.clear_busy = true;
        } else {
            fx.schedule_drain = true;
        }
        return fx;
    }

    // Success path.
    if current_event_matches {
        fx.remove_pending = true;
    }
    // Only clear the self-update requirement when this callback's event is
    // current. A superseded callback skips merge, so the requirement must
    // persist for drain to re-create the self-update. Merge failure is
    // handled separately via pending_merge_reconcile, not here.
    if current_event_matches && matches!(op, GroupEvolutionOp::SelfUpdate) {
        fx.remove_self_update = true;
    }
    if current_event_matches {
        fx.attempt_merge = true;
    }
    if has_welcomes {
        fx.send_welcomes = true;
    }
    if current_event_matches {
        fx.clear_busy = true;
    }
    fx.refresh_storage = true;
    if !current_event_matches {
        fx.schedule_drain = true;
    }

    fx
}

/// Whether to reset the merge-reconcile counter when queuing a new evolution event.
///
/// Returns true when the new event differs from what's currently queued (or nothing
/// is queued), ensuring stale retry budgets are cleared.
pub(super) fn should_reset_reconcile(
    prev_event_id: Option<EventId>,
    new_event_id: EventId,
) -> bool {
    prev_event_id.map(|id| id != new_event_id).unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event_id(b: u8) -> EventId {
        EventId::from_byte_array([b; 32])
    }

    fn input(session_gen: u64, pending: Option<(EventId, bool)>) -> EvolutionHandlerInput {
        EvolutionHandlerInput {
            current_session_gen: session_gen,
            pending_event: pending,
        }
    }

    // ── Stale session ──────────────────────────────────────────────

    #[test]
    fn stale_session_gen_is_ignored() {
        let state = input(5, Some((event_id(1), false)));
        let fx = evaluate_evolution_callback(
            &state,
            4,
            event_id(1),
            &GroupEvolutionOp::Standard,
            true,
            None,
            false,
        );
        assert!(fx.ignore);
        assert!(!fx.remove_in_flight);
        assert!(!fx.remove_pending);
        assert!(!fx.attempt_merge);
    }

    // ── Success + current event matches ────────────────────────────

    #[test]
    fn success_current_standard_merges_and_cleans_up() {
        let eid = event_id(1);
        let state = input(1, Some((eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            eid,
            &GroupEvolutionOp::Standard,
            true,
            None,
            false,
        );
        assert!(!fx.ignore);
        assert!(fx.remove_in_flight);
        assert!(fx.remove_pending);
        assert!(fx.attempt_merge);
        assert!(fx.clear_busy);
        assert!(fx.refresh_storage);
        assert!(!fx.remove_self_update);
        assert!(!fx.schedule_drain);
        assert!(!fx.send_welcomes);
    }

    #[test]
    fn success_current_self_update_clears_requirement() {
        let eid = event_id(1);
        let state = input(1, Some((eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            eid,
            &GroupEvolutionOp::SelfUpdate,
            true,
            None,
            false,
        );
        assert!(fx.remove_self_update);
        assert!(fx.remove_pending);
        assert!(fx.attempt_merge);
    }

    #[test]
    fn success_with_welcomes_sends_them() {
        let eid = event_id(1);
        let state = input(1, Some((eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            eid,
            &GroupEvolutionOp::Standard,
            true,
            None,
            true,
        );
        assert!(fx.send_welcomes);
    }

    // ── Success + superseded ───────────────────────────────────────

    #[test]
    fn success_superseded_does_not_merge_or_clear() {
        let old_eid = event_id(1);
        let new_eid = event_id(2);
        // Pending has new_eid, callback carries old_eid.
        let state = input(1, Some((new_eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            old_eid,
            &GroupEvolutionOp::Standard,
            true,
            None,
            false,
        );
        assert!(!fx.remove_pending);
        assert!(!fx.attempt_merge);
        assert!(!fx.clear_busy);
        assert!(fx.refresh_storage);
        assert!(fx.schedule_drain);
    }

    /// Regression test for finding 2: a superseded self-update callback must NOT
    /// clear the self-update requirement, because we intentionally skip merge for
    /// superseded events (MDK's pending commit may belong to the newer op).
    #[test]
    fn success_superseded_self_update_does_not_clear_requirement() {
        let old_eid = event_id(1);
        let new_eid = event_id(2);
        let state = input(1, Some((new_eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            old_eid,
            &GroupEvolutionOp::SelfUpdate,
            true,
            None,
            false,
        );
        assert!(!fx.remove_self_update);
        assert!(!fx.attempt_merge);
        assert!(fx.schedule_drain);
    }

    // ── Failure + Standard ─────────────────────────────────────────

    #[test]
    fn failure_standard_toasts_once() {
        let eid = event_id(1);
        let state = input(1, Some((eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            eid,
            &GroupEvolutionOp::Standard,
            false,
            Some("relay rejected"),
            false,
        );
        assert!(fx.mark_toasted);
        assert_eq!(
            fx.toast,
            Some("Group update failed: relay rejected".to_string())
        );
        assert!(fx.clear_busy);
        assert!(!fx.schedule_drain);
    }

    #[test]
    fn failure_standard_already_toasted_no_re_toast() {
        let eid = event_id(1);
        let state = input(1, Some((eid, true))); // already toasted
        let fx = evaluate_evolution_callback(
            &state,
            1,
            eid,
            &GroupEvolutionOp::Standard,
            false,
            Some("relay rejected"),
            false,
        );
        assert!(!fx.mark_toasted);
        assert!(fx.toast.is_none());
        assert!(fx.clear_busy);
    }

    #[test]
    fn failure_standard_superseded_drains_without_toast() {
        let old_eid = event_id(1);
        let new_eid = event_id(2);
        let state = input(1, Some((new_eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            old_eid,
            &GroupEvolutionOp::Standard,
            false,
            Some("err"),
            false,
        );
        assert!(!fx.clear_busy);
        assert!(fx.schedule_drain);
        // Should not toast — the superseded event's error isn't relevant.
        assert!(fx.toast.is_none());
    }

    // ── Failure + SelfUpdate ───────────────────────────────────────

    #[test]
    fn failure_self_update_silent_retry() {
        let eid = event_id(1);
        let state = input(1, Some((eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            eid,
            &GroupEvolutionOp::SelfUpdate,
            false,
            Some("timeout"),
            false,
        );
        assert!(fx.toast.is_none());
        assert!(!fx.clear_busy);
        assert!(!fx.schedule_drain); // current matches, no drain needed
        assert!(!fx.remove_self_update);
    }

    #[test]
    fn failure_self_update_superseded_drains() {
        let old_eid = event_id(1);
        let new_eid = event_id(2);
        let state = input(1, Some((new_eid, false)));
        let fx = evaluate_evolution_callback(
            &state,
            1,
            old_eid,
            &GroupEvolutionOp::SelfUpdate,
            false,
            Some("err"),
            false,
        );
        assert!(fx.schedule_drain);
    }

    // ── No pending entry (already cleaned up) ──────────────────────

    #[test]
    fn success_no_pending_entry_defaults_to_current() {
        let eid = event_id(1);
        let state = input(1, None); // pending already cleaned up
        let fx = evaluate_evolution_callback(
            &state,
            1,
            eid,
            &GroupEvolutionOp::Standard,
            true,
            None,
            false,
        );
        // current_event_matches defaults to true when no pending entry.
        assert!(fx.remove_pending); // no-op at call site, but correct intent
        assert!(fx.attempt_merge);
        assert!(fx.clear_busy);
    }

    // ── Reconcile counter reset ────────────────────────────────────

    /// Regression test for finding 1: when pending_evolutions was already cleaned
    /// up (prev processed successfully but merge failed), prev_event_id is None.
    /// Stale reconcile counters must still be cleared.
    #[test]
    fn reconcile_reset_no_previous_event() {
        assert!(should_reset_reconcile(None, event_id(1)));
    }

    #[test]
    fn reconcile_reset_different_event() {
        assert!(should_reset_reconcile(Some(event_id(1)), event_id(2)));
    }

    #[test]
    fn reconcile_no_reset_same_event() {
        let eid = event_id(1);
        assert!(!should_reset_reconcile(Some(eid), eid));
    }

    // ── Applier safety invariants ──────────────────────────────────
    //
    // The applier in handle_internal applies effects sequentially.
    // These tests verify the reducer never produces effect combinations
    // that would be unsafe under the applier's ordering:
    //   mark_toasted → toast → remove_pending → remove_self_update
    //   → attempt_merge → send_welcomes → clear_busy → refresh
    //   → schedule_drain

    /// mark_toasted writes to the pending_evolutions entry that
    /// remove_pending deletes. The reducer must never set both.
    #[test]
    fn invariant_mark_toasted_and_remove_pending_are_exclusive() {
        let cases: Vec<(EvolutionHandlerInput, bool, Option<&str>, &GroupEvolutionOp)> = vec![
            // All failure paths where mark_toasted could fire.
            (
                input(1, Some((event_id(1), false))),
                false,
                Some("err"),
                &GroupEvolutionOp::Standard,
            ),
            (
                input(1, Some((event_id(1), true))),
                false,
                Some("err"),
                &GroupEvolutionOp::Standard,
            ),
            (
                input(1, Some((event_id(2), false))),
                false,
                Some("err"),
                &GroupEvolutionOp::Standard,
            ),
            (
                input(1, Some((event_id(1), false))),
                false,
                Some("err"),
                &GroupEvolutionOp::SelfUpdate,
            ),
            (
                input(1, None),
                false,
                Some("err"),
                &GroupEvolutionOp::Standard,
            ),
            // All success paths where remove_pending could fire.
            (
                input(1, Some((event_id(1), false))),
                true,
                None,
                &GroupEvolutionOp::Standard,
            ),
            (
                input(1, Some((event_id(2), false))),
                true,
                None,
                &GroupEvolutionOp::Standard,
            ),
            (
                input(1, Some((event_id(1), false))),
                true,
                None,
                &GroupEvolutionOp::SelfUpdate,
            ),
            (input(1, None), true, None, &GroupEvolutionOp::Standard),
        ];
        for (state, ok, error, op) in cases {
            let fx = evaluate_evolution_callback(&state, 1, event_id(1), op, ok, error, false);
            assert!(
                !(fx.mark_toasted && fx.remove_pending),
                "mark_toasted and remove_pending must not both be true: \
                 ok={ok}, pending={:?}, op={op:?}",
                state.pending_event,
            );
        }
    }

    /// On any failure path, the reducer must never request merge or
    /// storage refresh (the commit isn't confirmed).
    #[test]
    fn invariant_failure_never_merges_or_refreshes() {
        let states = vec![
            input(1, Some((event_id(1), false))),
            input(1, Some((event_id(1), true))),
            input(1, Some((event_id(2), false))),
            input(1, None),
        ];
        let ops = [GroupEvolutionOp::Standard, GroupEvolutionOp::SelfUpdate];
        for state in &states {
            for op in &ops {
                let fx = evaluate_evolution_callback(
                    state,
                    1,
                    event_id(1),
                    op,
                    false,
                    Some("err"),
                    false,
                );
                assert!(
                    !fx.attempt_merge,
                    "failure must not merge: pending={:?}, op={op:?}",
                    state.pending_event,
                );
                assert!(
                    !fx.refresh_storage,
                    "failure must not refresh: pending={:?}, op={op:?}",
                    state.pending_event,
                );
            }
        }
    }
}
