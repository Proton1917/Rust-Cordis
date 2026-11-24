import Lifecycle

/-!
The effect protocol separates cancellation, execution completion, and cleanup
ownership. `defer` remains legal while the execution is running, including after
cancellation. The caller executes every returned cleanup operation, serially for
one effect. Failures do not discard the remaining operations.
-/

namespace Cordis.Effects

structure State (α : Type) where
  armed : Bool := true
  running : Bool := true
  cleaning : Bool := false
  pending : List α := []
  starts : Nat := 0
  deriving Repr

inductive Command (α : Type) where
  | defer (operation : α)
  | dispose
  | finish (failed : Bool)
  deriving Repr

/-- Only the cleanup claimant takes the owned stack. -/
def claim (guard : Bool → Bool → Bool) (s : State α) : State α × List α :=
  if guard s.running s.cleaning then
    ({s with cleaning := true, pending := [], starts := s.starts + 1}, s.pending.reverse)
  else (s, [])

def step (guard : Bool → Bool → Bool) (s : State α) : Command α → State α × List α
  | .defer operation =>
    (if s.running then {s with pending := s.pending ++ [operation]} else s, [])
  | .dispose => claim guard {s with armed := false}
  | .finish failed =>
    let next := {s with running := false, armed := s.armed && !failed}
    if next.armed then (next, []) else claim guard next

def referenceGuard (running cleaning : Bool) := !running && !cleaning

theorem guard_equiv : cleanup = referenceGuard :=
  funext fun running => funext fun cleaning => cleanup_equiv running cleaning

theorem step_equiv (s : State α) (command : Command α) :
    step cleanup s command = step referenceGuard s command := by rw [guard_equiv]

def run (guard : Bool → Bool → Bool) : State α → List (Command α) → State α × List α
  | s, [] => (s, [])
  | s, command :: rest =>
    let (next, released) := step guard s command
    let (last, releases) := run guard next rest
    (last, released ++ releases)

theorem trace_equiv (s : State α) (commands : List (Command α)) :
    run cleanup s commands = run referenceGuard s commands := by
  induction commands generalizing s with
  | nil => rfl
  | cons command rest ih => simp only [run, step_equiv, ih]

def OwnsOnce (s : State α) : Prop := s.starts = if s.cleaning then 1 else 0

theorem initial_owns_once : OwnsOnce ({} : State α) := rfl

theorem claim_preserves_ownership (s : State α) (h : OwnsOnce s) :
    OwnsOnce (claim cleanup s).1 := by
  cases hr : s.running <;> cases hc : s.cleaning <;>
    simp [claim, cleanup_equiv, OwnsOnce, hr, hc] at * <;> assumption

theorem step_preserves_ownership (s : State α) (command : Command α) (h : OwnsOnce s) :
    OwnsOnce (step cleanup s command).1 := by
  cases command with
  | defer operation =>
    cases hr : s.running <;> simpa [step, hr, OwnsOnce] using h
  | dispose => exact claim_preserves_ownership _ h
  | finish failed =>
    simp only [step]
    split
    · exact h
    · exact claim_preserves_ownership _ h

theorem run_preserves_ownership (s : State α) (commands : List (Command α)) (h : OwnsOnce s) :
    OwnsOnce (run cleanup s commands).1 := by
  induction commands generalizing s with
  | nil => exact h
  | cons command rest ih =>
    simp only [run]
    exact ih _ (step_preserves_ownership _ _ h)

/-- Repeated and concurrent disposal requests linearize to at most one claimant. -/
theorem cleanup_starts_at_most_once (commands : List (Command α)) :
    (run cleanup ({} : State α) commands).1.starts ≤ 1 := by
  have h := run_preserves_ownership ({} : State α) commands initial_owns_once
  unfold OwnsOnce at h
  rw [h]
  split <;> decide

/-- Cancellation of an in-flight execution cannot take its cleanup stack. -/
theorem cancel_waits_for_execution (s : State α) (h : s.running = true) :
    step cleanup s .dispose = ({s with armed := false}, []) := by
  simp [step, claim, cleanup_equiv, h]

/-- A final yield after cancellation is retained by its original owner. -/
theorem final_defer_retained (s : State α) (operation : α) (h : s.running = true) :
    (step cleanup {s with armed := false} (.defer operation)).1.pending =
      s.pending ++ [operation] := by simp [step, h]

theorem claimed_steps_are_lifo (s : State α) (hr : s.running = false)
    (hc : s.cleaning = false) :
    (claim cleanup s).2 = s.pending.reverse := by simp [claim, cleanup_equiv, hr, hc]

def drain (completed : List α) : List α → List α
  | [] => completed
  | operation :: rest => drain (completed ++ [operation]) rest

theorem drain_preserves_order (completed pending : List α) :
    drain completed pending = completed ++ pending := by
  induction pending generalizing completed with
  | nil => simp [drain]
  | cons operation rest ih => simp [drain, ih, List.append_assoc]

/-- No bound on the number of yields or cleanup operations. -/
theorem complete_rollback (s : State α) (hr : s.running = false) (hc : s.cleaning = false) :
    drain [] (claim cleanup s).2 = s.pending.reverse := by
  rw [claimed_steps_are_lifo s hr hc, drain_preserves_order]
  rfl

end Cordis.Effects
