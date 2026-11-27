import Effects

/-!
Task custody and serial cleanup execution, at the lock/first-poll boundaries of
`Kernel::submit` and `Effect::submit_cleanup`. The natural-number fields count
the single task token; Rust represents the corresponding slots with `Option`.
Rejection that leaves a wrapper in the host is distinct from recovery of the
unpolled task. Scheduler progress and Rust's ownership implementation are the
refinement boundary, not hypotheses asserting the safety result.
-/

namespace Cordis.Execution

structure TaskState where
  caller : Nat := 1
  slot : Nat := 0
  worker : Nat := 0
  suspended : Nat := 0
  finished : Nat := 0
  firstPolls : Nat := 0
  cancelled : Bool := false
  deriving DecidableEq

inductive TaskEvent where
  | submit | firstPoll | pending | ready
  | rejectRecovered | rejectRetained | suspend | resume
  | cancel | observerDropped
  deriving DecidableEq

def taskStep (s : TaskState) : TaskEvent → TaskState
  | .submit =>
    if 0 < s.caller then { s with caller := s.caller - 1, slot := s.slot + 1 } else s
  | .firstPoll =>
    if 0 < s.slot then
      { s with slot := s.slot - 1, worker := s.worker + 1, firstPolls := s.firstPolls + 1 }
    else s
  | .ready =>
    if 0 < s.worker then { s with worker := s.worker - 1, finished := s.finished + 1 } else s
  | .rejectRecovered =>
    if 0 < s.slot then { s with slot := s.slot - 1, caller := s.caller + 1 } else s
  | .suspend =>
    if 0 < s.caller then
      { s with caller := s.caller - 1, suspended := s.suspended + 1 }
    else s
  | .resume =>
    if 0 < s.suspended then
      { s with suspended := s.suspended - 1, caller := s.caller + 1 }
    else s
  | .cancel => { s with cancelled := true }
  | .pending | .rejectRetained | .observerDropped => s

def taskRun (s : TaskState) : List TaskEvent → TaskState
  | [] => s
  | event :: rest => taskRun (taskStep s event) rest

def tokens (s : TaskState) : Nat :=
  s.caller + s.slot + s.worker + s.suspended + s.finished

def TaskInvariant (s : TaskState) : Prop :=
  tokens s = 1 ∧ s.firstPolls = s.worker + s.finished

theorem initial_task_invariant : TaskInvariant {} := by simp [TaskInvariant, tokens]

theorem task_step_preserves (s : TaskState) (event : TaskEvent)
    (h : TaskInvariant s) : TaskInvariant (taskStep s event) := by
  rcases h with ⟨ht, hp⟩
  cases event <;> simp only [taskStep]
  all_goals try split
  all_goals simp_all [TaskInvariant, tokens]
  all_goals omega

theorem task_trace_preserves (s : TaskState) (events : List TaskEvent)
    (h : TaskInvariant s) : TaskInvariant (taskRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (task_step_preserves s event h)

/-- Any number of retries, observer drops and wakeups retain exactly one token. -/
theorem task_conserved (events : List TaskEvent) : tokens (taskRun {} events) = 1 :=
  (task_trace_preserves {} events initial_task_invariant).1

/-- Repeated polling cannot take the original task from its shared slot twice. -/
theorem first_poll_at_most_once (events : List TaskEvent) :
    (taskRun {} events).firstPolls ≤ 1 := by
  have h := task_trace_preserves {} events initial_task_invariant
  simp only [TaskInvariant, tokens] at h
  omega

theorem exclusive_custody (events : List TaskEvent) :
    let s := taskRun {} events
    s.caller + s.slot + s.worker + s.suspended + s.finished = 1 :=
  task_conserved events

theorem running_task_cannot_be_recovered (s : TaskState) (h : TaskInvariant s)
    (running : 0 < s.worker) : taskStep s .rejectRecovered = s := by
  have ht := h.1
  simp only [tokens] at ht
  have hs : s.slot = 0 := by omega
  simp [taskStep, hs]

theorem host_retention_preserves_custody (s : TaskState) :
    taskStep s .rejectRetained = s := rfl

theorem pending_and_observers_preserve_custody (s : TaskState) :
    taskStep (taskStep s .pending) .observerDropped = s := rfl

theorem cancellation_retains_task (s : TaskState) :
    tokens (taskStep s .cancel) = tokens s := rfl

/-- The two rejected submissions in `submit_cleanup` end in the owner's slot. -/
theorem double_rejection_is_recoverable :
    taskRun {} [.submit, .rejectRecovered, .submit, .rejectRecovered, .suspend] =
      { caller := 0, suspended := 1 : TaskState } := by decide

theorem resume_transfers_same_custody :
    taskRun {} [.submit, .rejectRecovered, .submit, .rejectRecovered, .suspend,
      .resume, .submit, .firstPoll, .ready] =
      { caller := 0, finished := 1, firstPolls := 1 : TaskState } := by decide

/-! Serial cleanup keeps a work partition even across pending polls and failure.
`pending` is the stack already reversed by the unique claimant in Effects.lean.
The callback result records an optional error code; every result advances the
same pending list. An observer never owns this continuation. -/

structure CleanupState (α : Type) where
  pending : List α
  current : Option α := none
  completed : List α := []
  firstError : Option Nat := none
  terminal : Bool := false

inductive CleanupEvent where
  | start | pending | observerDropped | dispatchRejected | dispatchResumed
  | ready (error : Option Nat)
  | finish
  deriving DecidableEq

def remember (prior next : Option Nat) : Option Nat :=
  match prior with
  | some error => some error
  | none => next

def cleanupStep (s : CleanupState α) : CleanupEvent → CleanupState α
  | .start =>
    match s.current, s.pending with
    | none, operation :: rest => { s with current := some operation, pending := rest }
    | _, _ => s
  | .ready error =>
    match s.current with
    | none => s
    | some operation =>
      { s with current := none, completed := s.completed ++ [operation]
               firstError := remember s.firstError error }
  | .finish =>
    match s.current, s.pending with
    | none, [] => { s with terminal := true }
    | _, _ => s
  | .pending | .observerDropped | .dispatchRejected | .dispatchResumed => s

def cleanupRun (s : CleanupState α) : List CleanupEvent → CleanupState α
  | [] => s
  | event :: rest => cleanupRun (cleanupStep s event) rest

def work (s : CleanupState α) : List α :=
  s.completed ++ s.current.toList ++ s.pending

def CleanupInvariant (original : List α) (s : CleanupState α) : Prop :=
  work s = original ∧ (s.terminal = true → s.current = none ∧ s.pending = [])

theorem initial_cleanup_invariant (operations : List α) :
    CleanupInvariant operations { pending := operations } := by
  simp [CleanupInvariant, work]

theorem cleanup_step_preserves_work (s : CleanupState α) (event : CleanupEvent) :
    work (cleanupStep s event) = work s := by
  cases event <;> cases hc : s.current <;> cases hp : s.pending <;>
    simp [cleanupStep, work, hc, hp, List.append_assoc]

theorem cleanup_step_preserves (original : List α) (s : CleanupState α)
    (event : CleanupEvent) (h : CleanupInvariant original s) :
    CleanupInvariant original (cleanupStep s event) := by
  refine ⟨(cleanup_step_preserves_work s event).trans h.1, ?_⟩
  have ht := h.2
  cases event <;> cases hc : s.current <;> cases hp : s.pending <;>
    simp_all [cleanupStep]

theorem cleanup_trace_preserves (original : List α) (s : CleanupState α)
    (events : List CleanupEvent) (h : CleanupInvariant original s) :
    CleanupInvariant original (cleanupRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (cleanup_step_preserves original s event h)

/-- Completion of an arbitrary legal trace contains the whole LIFO sequence. -/
theorem completed_cleanup_is_lifo (operations : List α) (events : List CleanupEvent)
    (done : (cleanupRun { pending := operations.reverse } events).terminal = true) :
    (cleanupRun { pending := operations.reverse } events).completed = operations.reverse := by
  have h := cleanup_trace_preserves operations.reverse _ events
    (initial_cleanup_invariant operations.reverse)
  have finished := h.2 done
  simpa [work, finished.1, finished.2] using h.1

/-- Compose the existing unique cleanup claimant with the asynchronous runner. -/
theorem claimed_cleanup_completes_registered_stack (s : Effects.State α)
    (stopped : s.running = false) (unclaimed : s.cleaning = false)
    (events : List CleanupEvent)
    (done : (cleanupRun { pending := (Effects.claim cleanup s).2 } events).terminal = true) :
    (cleanupRun { pending := (Effects.claim cleanup s).2 } events).completed = s.pending.reverse := by
  have handedOff := Effects.claimed_steps_are_lifo s stopped unclaimed
  rw [handedOff] at done ⊢
  exact completed_cleanup_is_lifo s.pending events done

theorem first_error_preserved (s : CleanupState α) (error : Nat)
    (h : s.firstError = some error) (event : CleanupEvent) :
    (cleanupStep s event).firstError = some error := by
  cases event <;> cases hc : s.current <;> cases hp : s.pending <;>
    simp_all [cleanupStep, remember]

theorem first_error_trace_preserved (s : CleanupState α) (error : Nat)
    (h : s.firstError = some error) (events : List CleanupEvent) :
    (cleanupRun s events).firstError = some error := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (first_error_preserved s error h event)

theorem failure_keeps_remaining_work (s : CleanupState α) (operation : α)
    (error : Nat) (h : s.current = some operation) :
    (cleanupStep s (.ready (some error))).pending = s.pending ∧
      (cleanupStep s (.ready (some error))).completed = s.completed ++ [operation] := by
  simp [cleanupStep, h]

def remaining (s : CleanupState α) : Nat := s.current.toList.length + s.pending.length

/-- Each returned callback decreases the remaining work, regardless of failure. -/
theorem callback_result_progress (s : CleanupState α) (operation : α)
    (error : Option Nat) (h : s.current = some operation) :
    remaining (cleanupStep s (.ready error)) + 1 = remaining s := by
  simp [remaining, cleanupStep, h, Nat.add_comm]

end Cordis.Execution
