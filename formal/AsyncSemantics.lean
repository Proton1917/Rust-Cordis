import Execution
import NativeOwnership

/-!
Future microsteps and conditional cleanup liveness. Poll entry consumes a queued
notification; wake during poll is remembered until Pending. Cancellation requests
destruction, and destruction waits for a synchronous poll to return. A distinct
permit-return step models Rust declaration-order destruction in Admitted.

The cleanup liveness contract names productive host dispatch and callback return
events. It permits arbitrary intervening pending polls and observer cancellation;
it does not supply an eventual terminal state as a hypothesis.
-/
namespace Cordis.AsyncSemantics

inductive Phase where
  | queued | polling | sleeping | ready | destroyed | released
  deriving DecidableEq

structure FutureState where
  phase : Phase := .queued
  notified : Bool := false
  cancelled : Bool := false
  polls : Nat := 0
  destroys : Nat := 0
  returns : Nat := 0
  deriving DecidableEq

inductive FutureEvent where
  | wake | poll | pending | ready | cancel | destroy | returnPermit | observerDrop
  deriving DecidableEq

def alive (s : FutureState) : Bool := s.phase != .destroyed && s.phase != .released

def permit (s : FutureState) : Bool := s.phase != .released

def futureStep (s : FutureState) : FutureEvent → FutureState
  | .wake =>
    match s.phase with
    | .sleeping => { s with phase := .queued, notified := true }
    | .queued | .polling => { s with notified := true }
    | _ => s
  | .poll =>
    if s.phase = .queued then
      { s with phase := .polling, notified := false, polls := s.polls + 1 }
    else s
  | .pending =>
    if s.phase = .polling then
      { s with phase := if s.notified then .queued else .sleeping }
    else s
  | .ready =>
    if s.phase = .polling then { s with phase := .ready } else s
  | .cancel => { s with cancelled := true }
  | .destroy =>
    if alive s = true ∧ s.phase ≠ .polling ∧ (s.cancelled = true ∨ s.phase = .ready) then
      { s with phase := .destroyed, destroys := s.destroys + 1 }
    else s
  | .returnPermit =>
    if s.phase = .destroyed then
      { s with phase := .released, returns := s.returns + 1 }
    else s
  | .observerDrop => s

def FutureInvariant (s : FutureState) : Prop :=
  s.destroys + NativeOwnership.bit (alive s) = 1 ∧
  s.returns + NativeOwnership.bit (permit s) = 1 ∧
  (alive s = true → permit s = true)

theorem initial_future_invariant : FutureInvariant {} := by
  simp [FutureInvariant, alive, permit, NativeOwnership.bit]

theorem future_step_preserves (s : FutureState) (event : FutureEvent)
    (h : FutureInvariant s) : FutureInvariant (futureStep s event) := by
  cases phase : s.phase <;> cases notified : s.notified <;>
    cases cancelled : s.cancelled <;> cases event <;>
    simp_all [futureStep, FutureInvariant, alive, permit, NativeOwnership.bit] <;> omega

def futureRun (s : FutureState) : List FutureEvent → FutureState
  | [] => s
  | event :: rest => futureRun (futureStep s event) rest

theorem future_trace_preserves (s : FutureState) (events : List FutureEvent)
    (h : FutureInvariant s) : FutureInvariant (futureRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (future_step_preserves s event h)

theorem destruction_and_permit_return_are_unique (events : List FutureEvent) :
    (futureRun {} events).destroys ≤ 1 ∧ (futureRun {} events).returns ≤ 1 := by
  have h := future_trace_preserves {} events initial_future_invariant
  exact ⟨by have := h.1; omega, by have := h.2.1; omega⟩

theorem active_poll_excludes_destruction (s : FutureState) (polling : s.phase = .polling) :
    futureStep s .destroy = s := by simp [futureStep, polling]

theorem cancellation_keeps_permit (s : FutureState) :
    permit (futureStep s .cancel) = permit s := rfl

theorem wake_during_poll_survives_pending (s : FutureState)
    (polling : s.phase = .polling) :
    (futureStep (futureStep s .wake) .pending).phase = .queued := by
  simp [futureStep, polling]

theorem pending_without_notification_sleeps (s : FutureState)
    (polling : s.phase = .polling) (quiet : s.notified = false) :
    (futureStep s .pending).phase = .sleeping := by simp [futureStep, polling, quiet]

theorem return_follows_destruction (s : FutureState) (h : FutureInvariant s)
    (returned : s.returns = 1) : alive s = false := by
  cases phase : s.phase <;> simp_all [FutureInvariant, alive, permit, NativeOwnership.bit]

theorem observer_does_not_cancel_work (s : FutureState) : futureStep s .observerDrop = s := rfl

def nativeProjection (s : FutureState) : NativeOwnership.Callback :=
  { nativeAlive := alive s, permitHeld := permit s, polling := s.phase == .polling,
    cancelled := s.cancelled, destroyed := s.destroys, returned := s.returns }

theorem projection_satisfies_native_ownership (s : FutureState) (h : FutureInvariant s) :
    NativeOwnership.CallbackInvariant (nativeProjection s) := by
  cases phase : s.phase <;>
    simp_all [NativeOwnership.CallbackInvariant, nativeProjection, FutureInvariant,
      alive, permit, NativeOwnership.bit]

/-! A weighted rank counts unstarted callbacks twice, active callbacks once, and
terminal publication once. Starting, returning, and finishing each lower it. -/

def cleanupRank (s : Execution.CleanupState α) : Nat :=
  2 * s.pending.length + s.current.toList.length + if s.terminal then 0 else 1

def Productive (s : Execution.CleanupState α) : Execution.CleanupEvent → Prop
  | .start => s.current = none ∧ s.pending ≠ []
  | .ready _ => s.current ≠ none
  | .finish => s.current = none ∧ s.pending = [] ∧ s.terminal = false
  | _ => False

theorem cleanup_rank_never_increases (s : Execution.CleanupState α)
    (event : Execution.CleanupEvent) :
    cleanupRank (Execution.cleanupStep s event) ≤ cleanupRank s := by
  cases event <;> cases current : s.current <;> cases pending : s.pending <;>
    cases terminal : s.terminal <;>
    simp [Execution.cleanupStep, cleanupRank, current, pending, terminal] <;> omega

theorem productive_cleanup_step_decreases (s : Execution.CleanupState α)
    (event : Execution.CleanupEvent) (productive : Productive s event) :
    cleanupRank (Execution.cleanupStep s event) < cleanupRank s := by
  cases event <;> cases current : s.current <;> cases pending : s.pending <;>
    cases terminal : s.terminal <;>
    simp_all [Productive, Execution.cleanupStep, cleanupRank] <;> omega

/-- Host dispatch eventually starts/finishes enabled work, and each active
callback eventually returns. Intervening polling/observation events are unbounded. -/
def FairCleanup (states : Nat → Execution.CleanupState α)
    (events : Nat → Execution.CleanupEvent) : Prop :=
  ∀ i, (states i).terminal = false →
    ∃ j, i ≤ j ∧ Productive (states j) (events j)

theorem cleanup_rank_monotone (states : Nat → Execution.CleanupState α)
    (events : Nat → Execution.CleanupEvent)
    (steps : ∀ n, states (n + 1) = Execution.cleanupStep (states n) (events n))
    (i j : Nat) (ordered : i ≤ j) : cleanupRank (states j) ≤ cleanupRank (states i) := by
  obtain ⟨d, rfl⟩ := Nat.exists_eq_add_of_le ordered
  clear ordered
  induction d with
  | zero => simp
  | succ d ih =>
    calc
      cleanupRank (states (i + (d + 1))) ≤ cleanupRank (states (i + d)) := by
        rw [← Nat.add_assoc, steps]
        exact cleanup_rank_never_increases (states (i + d)) (events (i + d))
      _ ≤ cleanupRank (states i) := ih

theorem fair_cleanup_eventually_finishes (states : Nat → Execution.CleanupState α)
    (events : Nat → Execution.CleanupEvent)
    (steps : ∀ n, states (n + 1) = Execution.cleanupStep (states n) (events n))
    (fair : FairCleanup states events) (i : Nat) :
    ∃ j, i ≤ j ∧ (states j).terminal = true := by
  have finish : ∀ rank i, cleanupRank (states i) = rank →
      ∃ j, i ≤ j ∧ (states j).terminal = true := by
    intro rank
    induction rank using Nat.strongRecOn with
    | ind rank ih =>
      intro i rankEq
      cases terminal : (states i).terminal with
      | true => exact ⟨i, Nat.le_refl i, terminal⟩
      | false =>
        obtain ⟨j, ordered, productive⟩ := fair i terminal
        have decreases := productive_cleanup_step_decreases (states j) (events j) productive
        have bounded := cleanup_rank_monotone states events steps i j ordered
        have smaller : cleanupRank (states (j + 1)) < rank := by rw [steps]; omega
        obtain ⟨k, after, done⟩ := ih _ smaller (j + 1) rfl
        exact ⟨k, by omega, done⟩
  exact finish _ i rfl

theorem infinite_cleanup_states_keep_work (operations : List α)
    (states : Nat → Execution.CleanupState α) (events : Nat → Execution.CleanupEvent)
    (initial : states 0 = { pending := operations.reverse })
    (steps : ∀ n, states (n + 1) = Execution.cleanupStep (states n) (events n)) :
    ∀ n, Execution.CleanupInvariant operations.reverse (states n) := by
  intro n
  induction n with
  | zero => rw [initial]; exact Execution.initial_cleanup_invariant _
  | succ n ih => rw [steps]; exact Execution.cleanup_step_preserves _ _ _ ih

/-- Links fairness, actual cleanup work partition and the earlier LIFO theorem. -/
theorem fair_cleanup_returns_all_registered_work (operations : List α)
    (states : Nat → Execution.CleanupState α) (events : Nat → Execution.CleanupEvent)
    (initial : states 0 = { pending := operations.reverse })
    (steps : ∀ n, states (n + 1) = Execution.cleanupStep (states n) (events n))
    (fair : FairCleanup states events) :
    ∃ j, (states j).terminal = true ∧ (states j).completed = operations.reverse := by
  obtain ⟨j, _, done⟩ := fair_cleanup_eventually_finishes states events steps fair 0
  have h := infinite_cleanup_states_keep_work operations states events initial steps j
  have terminal := h.2 done
  refine ⟨j, done, ?_⟩
  simpa [Execution.work, terminal.1, terminal.2] using h.1

end Cordis.AsyncSemantics
