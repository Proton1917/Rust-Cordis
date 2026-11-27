import Protocol
import SdkProtocol

/-!
A complete outgoing request owns its result between response settlement and
return to its caller. Sending, response arrival and coroutine resumption are
separate events. This distinction models Python's cancelled-after-settlement
window and TypeScript's Promise.all send failure window.
-/
namespace Cordis.RequestSemantics

abbrev Style := SdkProtocol.Style

structure State where
  waiter : SdkProtocol.Waiter := { present := false }
  admitted : Bool := false
  sent : Bool := false
  exited : Bool := false
  held : Nat := 0
  returned : Nat := 0
  abandoned : Nat := 0
  deriving DecidableEq

inductive Event where
  | acceptRequest | sendSucceeded | response | remoteError
  | cancel | timeout | disconnect | sendFailed | returnPayload | finishError
  deriving DecidableEq

def closeWaiter (style : Style) (waiter : SdkProtocol.Waiter)
    (event : SdkProtocol.Event) : SdkProtocol.Waiter :=
  SdkProtocol.step style (SdkProtocol.step style waiter event) .cleanup

def abort (style : Style) (s : State) (event : SdkProtocol.Event) : State :=
  { s with waiter := closeWaiter style s.waiter event, exited := true, held := 0, abandoned := s.abandoned + s.held }

def step (style : Style) (s : State) : Event → State
  | .acceptRequest =>
      if s.admitted || s.exited then s
      else { s with admitted := true, waiter := { s.waiter with present := true } }
  | .sendSucceeded => { s with sent := true }
  | .response =>
      { s with waiter := SdkProtocol.step style s.waiter .ok, held := s.held + if SdkProtocol.active s.waiter then 1 else 0 }
  | .remoteError => { s with waiter := SdkProtocol.step style s.waiter .error }
  | .cancel => abort style s .timeout
  | .timeout => abort style s .timeout
  | .disconnect => abort style s .disconnect
  | .sendFailed => abort style s .error
  | .returnPayload =>
      if s.sent && !s.exited && s.held > 0 then
        { s with waiter := { s.waiter with present := false }, exited := true, held := 0, returned := s.returned + s.held }
      else s
  | .finishError =>
      if s.waiter.done && s.held == 0 then
        { s with waiter := { s.waiter with present := false }, exited := true }
      else s

def Invariant (s : State) : Prop :=
  SdkProtocol.Invariant s.waiter ∧
  s.waiter.accepted = s.held + s.returned + s.abandoned ∧
  (s.exited = true → s.held = 0 ∧ s.waiter.present = false)

theorem initial : Invariant {} := by
  simp [Invariant, SdkProtocol.Invariant, SdkProtocol.bit]

theorem close_waiter_invariant (style : Style) (waiter : SdkProtocol.Waiter)
    (h : SdkProtocol.Invariant waiter) (event : SdkProtocol.Event) :
    SdkProtocol.Invariant (closeWaiter style waiter event) :=
  SdkProtocol.step_preserves style _ .cleanup (SdkProtocol.step_preserves style waiter event h)

theorem terminal_keeps_accepted (style : Style) (waiter : SdkProtocol.Waiter)
    (event : SdkProtocol.Event) (h : event ≠ .ok) :
    (SdkProtocol.step style waiter event).accepted = waiter.accepted := by
  cases event <;> simp_all [SdkProtocol.step, SdkProtocol.settle]
  all_goals split <;> rfl

theorem abort_preserves (style : Style) (s : State) (h : Invariant s)
    (event : SdkProtocol.Event) (terminal : event ≠ .ok) :
    Invariant (abort style s event) := by
  refine ⟨close_waiter_invariant style s.waiter h.1 event, ?_, ?_⟩
  · have ha := terminal_keeps_accepted style s.waiter event terminal
    change (SdkProtocol.step style s.waiter event).accepted = 0 + s.returned + (s.abandoned + s.held)
    rw [ha]
    have hl := h.2.1
    omega
  · simp [abort, closeWaiter, SdkProtocol.step]

theorem response_preserves (style : Style) (s : State) (h : Invariant s) :
    Invariant (step style s .response) := by
  refine ⟨SdkProtocol.step_preserves style s.waiter .ok h.1, ?_, ?_⟩
  · have hl := h.2.1
    by_cases active : SdkProtocol.active s.waiter = true
    · simp [step, SdkProtocol.step, SdkProtocol.settle, active]
      omega
    · simp [step, SdkProtocol.step, SdkProtocol.settle, active, hl]
  · intro exited
    have hexit := h.2.2 exited
    simp [step, SdkProtocol.active, hexit.1, hexit.2,
      SdkProtocol.step, SdkProtocol.settle]

theorem step_preserves (style : Style) (s : State) (event : Event)
    (h : Invariant s) : Invariant (step style s event) := by
  cases event with
  | acceptRequest =>
    simp only [step]
    split
    · exact h
    · refine ⟨h.1, h.2.1, ?_⟩
      rename_i guard
      simp at guard
      simp [guard.2]
  | sendSucceeded => exact h
  | response => exact response_preserves style s h
  | remoteError =>
    refine ⟨SdkProtocol.step_preserves style s.waiter .error h.1, ?_, ?_⟩
    · simpa [step, terminal_keeps_accepted style s.waiter .error (by decide)] using h.2.1
    · intro he
      have hp := (h.2.2 he).2
      simpa [step, SdkProtocol.step, SdkProtocol.settle, SdkProtocol.active, hp] using h.2.2 he
  | cancel => exact abort_preserves style s h .timeout (by decide)
  | timeout => exact abort_preserves style s h .timeout (by decide)
  | disconnect => exact abort_preserves style s h .disconnect (by decide)
  | sendFailed => exact abort_preserves style s h .error (by decide)
  | returnPayload =>
    simp only [step]
    split
    · refine ⟨h.1, ?_, ?_⟩
      · have hl := h.2.1
        simp only
        omega
      · simp
    · exact h
  | finishError =>
    simp only [step]
    split
    · rename_i guard
      have zero : s.held = 0 := by
        simpa using (show s.waiter.done = true ∧ s.held = 0 from by simpa using guard).2
      exact ⟨h.1, h.2.1, by simp [zero]⟩
    · exact h

def run (style : Style) (s : State) : List Event → State
  | [] => s
  | event :: rest => run style (step style s event) rest

theorem run_preserves (style : Style) (events : List Event) (s : State)
    (h : Invariant s) : Invariant (run style s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (step_preserves style s event h)

theorem all_request_traces_conserve_payloads (style : Style) (events : List Event) :
    let s := run style {} events
    s.waiter.payloads = s.held + s.returned + s.abandoned + s.waiter.lateClosed := by
  have h := run_preserves style events {} initial
  dsimp
  have hp := h.1.2.2
  rw [h.2.1] at hp
  exact hp

theorem result_returned_at_most_once (style : Style) (events : List Event) :
    (run style {} events).returned ≤ 1 := by
  have h := run_preserves style events {} initial
  have hb : SdkProtocol.bit (run style {} events).waiter.done ≤ 1 := by
    cases (run style {} events).waiter.done <;> simp [SdkProtocol.bit]
  have hs := h.1.1
  have ha := h.1.2.1
  have hl := h.2.1
  omega

theorem exited_request_has_no_orphan_payload (style : Style) (events : List Event)
    (exited : (run style {} events).exited = true) :
    let s := run style {} events
    s.held = 0 ∧ s.waiter.present = false ∧
      s.waiter.payloads = s.returned + s.abandoned + s.waiter.lateClosed := by
  have h := (run_preserves style events {} initial).2.2 exited
  have hp := all_request_traces_conserve_payloads style events
  dsimp at hp ⊢
  exact ⟨h.1, h.2, by simpa [h.1] using hp⟩

theorem response_then_cancel_closes_received_python :
    let s := run .python {} [.acceptRequest, .sendSucceeded, .response, .cancel]
    s.returned = 0 ∧ s.abandoned = 1 ∧ s.held = 0 := by decide

theorem response_then_failed_send_closes_received_typescript :
    let s := run .typescript {} [.acceptRequest, .response, .sendFailed]
    s.returned = 0 ∧ s.abandoned = 1 ∧ s.held = 0 := by decide

theorem successful_request_transfers_exactly_one (style : Style) :
    let s := run style {} [.acceptRequest, .sendSucceeded, .response, .returnPayload]
    s.returned = 1 ∧ s.abandoned = 0 ∧ s.held = 0 := by
  cases style <;> decide

/-! Shared deadline semantics: durations are measured in one monotone clock.
The transmitted integer budget is rounded separately, at the write boundary. -/
def deadline (localDeadline : Nat) (parent : Option Nat) : Nat :=
  match parent with | none => localDeadline | some inherited => min localDeadline inherited

theorem child_deadline_respects_local (localDeadline : Nat) (parent : Option Nat) :
    deadline localDeadline parent ≤ localDeadline := by cases parent <;> simp [deadline, Nat.min_le_left]

theorem child_deadline_respects_parent (localDeadline parent : Nat) :
    deadline localDeadline (some parent) ≤ parent := by simp [deadline, Nat.min_le_right]

def nestedDeadline (root : Nat) : List Nat → Nat
  | [] => root
  | localDeadline :: rest => nestedDeadline (deadline localDeadline (some root)) rest

theorem any_nesting_keeps_root_deadline (root : Nat) (locals : List Nat) :
    nestedDeadline root locals ≤ root := by
  induction locals generalizing root with
  | nil => exact Nat.le_refl _
  | cons localDeadline locals ih =>
    exact Nat.le_trans (ih _) (child_deadline_respects_parent localDeadline root)

theorem earlier_parent_is_monotone (localDeadline first second : Nat) (h : first ≤ second) :
    deadline localDeadline (some first) ≤ deadline localDeadline (some second) := by
  simp [deadline]
  omega

def remainingBudget (now endTime : Nat) : Nat := max 1 (endTime - now)

theorem later_write_never_increases_budget (first second endTime : Nat)
    (time : first ≤ second) : remainingBudget second endTime ≤ remainingBudget first endTime := by
  simp [remainingBudget]
  omega

theorem inherited_deadline_health_owner (localDeadline parent : Nat) (earlier : parent < localDeadline) :
    deadline localDeadline (some parent) ≠ localDeadline := by simp [deadline]; omega

/-! One request event updates both transport ownership and response ownership.
Queue admission, the first positive write, write completion, response settlement
and return remain distinct. A completed write has passed a positive write.
-/
structure Session where
  request : State := {}
  write : Protocol.RequestWrite := {}
  queue : Protocol.FIFO Nat := {}
  endTime : Nat

inductive SessionEvent where
  | enqueue (id : Nat) | dequeue | firstWrite (now : Nat) | writeComplete
  | response | remoteError | cancel | timeout | disconnect | sendFailed
  | returnPayload | finishError

def sessionAbort (style : Style) (s : Session) (event : Event) : Session :=
  { s with request := step style s.request event, write := Protocol.writeStep s.write .cancel }

def sessionStep (style : Style) (s : Session) : SessionEvent → Session
  | .enqueue id => { s with request := step style s.request .acceptRequest, queue := Protocol.enqueue s.queue id }
  | .dequeue => { s with queue := Protocol.dequeue s.queue }
  | .firstWrite now =>
      if now < s.endTime then { s with write := Protocol.writeStep s.write .positiveWrite }
      else sessionAbort style s .timeout
  | .writeComplete =>
      if s.write.started then { s with request := step style s.request .sendSucceeded } else s
  | .response => { s with request := step style s.request .response }
  | .remoteError => { s with request := step style s.request .remoteError }
  | .cancel => sessionAbort style s .cancel
  | .timeout => sessionAbort style s .timeout
  | .disconnect => sessionAbort style s .disconnect
  | .sendFailed => sessionAbort style s .sendFailed
  | .returnPayload => { s with request := step style s.request .returnPayload }
  | .finishError => { s with request := step style s.request .finishError }

def SessionInvariant (s : Session) : Prop :=
  Invariant s.request ∧ Protocol.WriteInvariant s.write ∧ s.queue.Valid ∧
    (s.request.sent = true → s.write.started = true)

theorem session_initial (endTime : Nat) : SessionInvariant { endTime := endTime } :=
  ⟨initial, Protocol.write_initial, rfl, by simp⟩

theorem started_is_monotone (write : Protocol.RequestWrite) (event : Protocol.WriteEvent)
    (started : write.started = true) : (Protocol.writeStep write event).started = true := by
  cases event <;> simp [Protocol.writeStep, started]

theorem abort_session_preserves (style : Style) (s : Session) (h : SessionInvariant s)
    (event : Event) (terminal : event = .cancel ∨ event = .timeout ∨ event = .disconnect ∨ event = .sendFailed) :
    SessionInvariant (sessionAbort style s event) := by
  refine ⟨step_preserves style _ event h.1, Protocol.write_preserves _ .cancel h.2.1, h.2.2.1, ?_⟩
  rcases terminal with he | he | he | he <;> subst event <;>
    exact fun sent => started_is_monotone s.write .cancel (h.2.2.2 sent)

theorem session_preserves (style : Style) (s : Session) (event : SessionEvent)
    (h : SessionInvariant s) : SessionInvariant (sessionStep style s event) := by
  cases event with
  | enqueue id =>
    refine ⟨step_preserves style _ .acceptRequest h.1, h.2.1,
      Protocol.enqueue_preserves_fifo _ id h.2.2.1, ?_⟩
    simp only [sessionStep, step]
    split <;> exact h.2.2.2
  | dequeue => exact ⟨h.1, h.2.1, Protocol.dequeue_preserves_fifo _ h.2.2.1, h.2.2.2⟩
  | firstWrite now =>
    simp only [sessionStep]
    split
    · exact ⟨h.1, Protocol.write_preserves _ .positiveWrite h.2.1, h.2.2.1,
        fun sent => started_is_monotone s.write .positiveWrite (h.2.2.2 sent)⟩
    · exact abort_session_preserves style s h .timeout (by simp)
  | writeComplete =>
    simp only [sessionStep]
    split
    · rename_i started
      exact ⟨step_preserves style _ .sendSucceeded h.1, h.2.1, h.2.2.1, fun _ => started⟩
    · exact h
  | response => exact ⟨step_preserves style _ .response h.1, h.2⟩
  | remoteError => exact ⟨step_preserves style _ .remoteError h.1, h.2⟩
  | cancel => exact abort_session_preserves style s h .cancel (by simp)
  | timeout => exact abort_session_preserves style s h .timeout (by simp)
  | disconnect => exact abort_session_preserves style s h .disconnect (by simp)
  | sendFailed => exact abort_session_preserves style s h .sendFailed (by simp)
  | returnPayload =>
    refine ⟨step_preserves style _ .returnPayload h.1, h.2.1, h.2.2.1, ?_⟩
    simp only [sessionStep, step]
    split <;> exact h.2.2.2
  | finishError =>
    refine ⟨step_preserves style _ .finishError h.1, h.2.1, h.2.2.1, ?_⟩
    simp only [sessionStep, step]
    split <;> exact h.2.2.2

def sessionRun (style : Style) (s : Session) : List SessionEvent → Session
  | [] => s
  | event :: rest => sessionRun style (sessionStep style s event) rest

theorem session_run_preserves (style : Style) (events : List SessionEvent) (s : Session)
    (h : SessionInvariant s) : SessionInvariant (sessionRun style s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (session_preserves style s event h)

theorem complete_request_trace_conserves_all_owners (style : Style) (endTime : Nat)
    (events : List SessionEvent) :
    let s := sessionRun style { endTime := endTime } events
    s.write.granted + s.write.rolledBack + Protocol.bit s.write.token = 1 ∧
    s.queue.accepted = s.queue.consumed ++ s.queue.waiting ∧
    s.request.waiter.payloads = s.request.held + s.request.returned +
      s.request.abandoned + s.request.waiter.lateClosed := by
  have h := session_run_preserves style events _ (session_initial endTime)
  dsimp
  refine ⟨h.2.1.1, h.2.2.1, ?_⟩
  have hp := h.1.1.2.2
  rw [h.1.2.1] at hp
  exact hp

theorem expired_before_first_write_rolls_back (style : Style) (endTime now id : Nat)
    (expired : endTime ≤ now) :
    let s := sessionRun style { endTime := endTime } [.enqueue id, .firstWrite now]
    s.write.granted = 0 ∧ s.write.rolledBack = 1 ∧ s.request.exited = true := by
  simp [sessionRun, sessionStep, sessionAbort, step, abort, Protocol.writeStep,
    Protocol.bit, show ¬now < endTime by omega]

/-- Parent cancellation follows the active await/signal edges supplied by the runtime. -/
def cancelDescendants (style : Style) (children : List State) : List State :=
  children.map (fun child => abort style child .timeout)

theorem parent_cancel_returns_every_held_descendant (style : Style) (children : List State)
    (child : State) (member : child ∈ cancelDescendants style children) :
    child.exited = true ∧ child.held = 0 ∧ child.waiter.present = false := by
  simp only [cancelDescendants, List.mem_map] at member
  obtain ⟨before, _, rfl⟩ := member
  simp [abort, closeWaiter, SdkProtocol.step]

end Cordis.RequestSemantics
