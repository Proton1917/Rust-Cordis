import Std

/-!
UP/1 lock-linearized operations, extracted from `cordis-up/src/rpc.rs` and
`rpc/buffer.rs`. An event is one completed critical section, not an arbitrary
Rust instruction. The first-write token is the right to invoke the optional
commit hook; invoking that hook happens after the lock is released.
-/

namespace Cordis.Protocol

def bit (b : Bool) : Nat := if b then 1 else 0

/-! ## First positive write and cancellation -/

structure RequestWrite where
  started : Bool := false
  cancelled : Bool := false
  token : Bool := true
  granted : Nat := 0
  rolledBack : Nat := 0
  deriving DecidableEq

inductive WriteEvent where
  | pending | positiveWrite | cancel
  deriving DecidableEq

def writeStep (s : RequestWrite) : WriteEvent → RequestWrite
  | .pending => s
  | .positiveWrite =>
      if s.cancelled || s.started then s
      else { s with started := true, token := false,
                    granted := s.granted + bit s.token }
  | .cancel =>
      if s.started then { s with cancelled := true }
      else { s with cancelled := true, token := false,
                    rolledBack := s.rolledBack + bit s.token }

def WriteInvariant (s : RequestWrite) : Prop :=
  s.granted + s.rolledBack + bit s.token = 1 ∧
  (s.started = true → s.granted = 1) ∧
  (s.cancelled = true → s.token = false) ∧
  (s.started = false → s.cancelled = false → s.token = true)

theorem write_initial : WriteInvariant {} := by simp [WriteInvariant, bit]

theorem write_preserves (s : RequestWrite) (e : WriteEvent)
    (h : WriteInvariant s) : WriteInvariant (writeStep s e) := by
  rcases s with ⟨started, cancelled, token, granted, rolledBack⟩
  cases e <;> cases started <;> cases cancelled <;> cases token <;>
    simp_all [writeStep, WriteInvariant, bit] <;> omega

def writeRun (s : RequestWrite) : List WriteEvent → RequestWrite
  | [] => s
  | e :: rest => writeRun (writeStep s e) rest

theorem write_run_preserves (events : List WriteEvent) (s : RequestWrite)
    (h : WriteInvariant s) : WriteInvariant (writeRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons e rest ih => exact ih _ (write_preserves s e h)

theorem write_trace_unique_owner (events : List WriteEvent) :
    let s := writeRun {} events
    s.granted ≤ 1 ∧ s.rolledBack ≤ 1 ∧
      s.granted + s.rolledBack + bit s.token = 1 := by
  have h := write_run_preserves events {} write_initial
  dsimp
  unfold WriteInvariant at h
  omega

theorem cancelled_before_start_never_writes (s : RequestWrite)
    (h : s.cancelled = true) : writeStep s .positiveWrite = s := by
  simp [writeStep, h]

theorem cancel_after_start_keeps_commit_owner (s : RequestWrite)
    (h : s.started = true) :
    (writeStep s .cancel).granted = s.granted ∧
      (writeStep s .cancel).rolledBack = s.rolledBack := by
  simp [writeStep, h]

theorem first_positive_write_takes_token (s : RequestWrite)
    (hn : s.started = false) (hc : s.cancelled = false) :
    (writeStep s .positiveWrite).started = true ∧
      (writeStep s .positiveWrite).token = false ∧
      (writeStep s .positiveWrite).granted = s.granted + bit s.token := by
  simp [writeStep, hn, hc]

/-! ## Consuming a pending reply before accounting for connection health -/

inductive Terminal where
  | healthy | timeout | cancel | disconnect
  deriving DecidableEq

structure Pending where
  present : Bool := true
  ownsDeadline : Bool
  claims : Nat := 0
  health : Nat := 0
  healthUpdates : Nat := 0
  deriving DecidableEq

def healthAfter (owns : Bool) (health : Nat) : Terminal → Nat
  | .healthy => 0
  | .timeout => if owns then health + 1 else health
  | .cancel | .disconnect => health

def accountsHealth (owns : Bool) : Terminal → Bool
  | .healthy => true
  | .timeout => owns
  | .cancel | .disconnect => false

def claim (s : Pending) (event : Terminal) : Pending :=
  if s.present then
    { s with
      present := false
      claims := s.claims + 1
      health := healthAfter s.ownsDeadline s.health event
      healthUpdates := s.healthUpdates + bit (accountsHealth s.ownsDeadline event) }
  else s

def PendingInvariant (s : Pending) : Prop :=
  s.claims + bit s.present = 1 ∧ s.healthUpdates ≤ s.claims

theorem pending_initial (owns : Bool) (health : Nat) :
    PendingInvariant { ownsDeadline := owns, health := health } := by
  simp [PendingInvariant, bit]

theorem claim_preserves (s : Pending) (e : Terminal)
    (h : PendingInvariant s) : PendingInvariant (claim s e) := by
  rcases s with ⟨present, owns, claims, health, updates⟩
  cases present <;> cases owns <;> cases e <;>
    simp_all [PendingInvariant, claim, bit, accountsHealth] <;> omega

def terminalRun (s : Pending) : List Terminal → Pending
  | [] => s
  | e :: rest => terminalRun (claim s e) rest

theorem terminal_run_preserves (events : List Terminal) (s : Pending)
    (h : PendingInvariant s) : PendingInvariant (terminalRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons e rest ih => exact ih _ (claim_preserves s e h)

theorem terminal_trace_single_claim (owns : Bool) (health : Nat)
    (events : List Terminal) :
    let s := terminalRun { ownsDeadline := owns, health := health } events
    s.claims ≤ 1 ∧ s.healthUpdates ≤ 1 := by
  have h := terminal_run_preserves events _ (pending_initial owns health)
  dsimp
  unfold PendingInvariant at h
  omega

theorem duplicate_terminal_is_noop (s : Pending) (first second : Terminal) :
    claim (claim s first) second = claim s first := by
  cases h : s.present <;> simp [claim, h]

theorem inherited_timeout_keeps_health (s : Pending) (h : s.ownsDeadline = false) :
    (claim s .timeout).health = s.health := by
  cases hp : s.present <;> simp [claim, hp, healthAfter, h]

theorem healthy_claim_resets_health (s : Pending) (h : s.present = true) :
    (claim s .healthy).health = 0 := by simp [claim, h, healthAfter]

/-! Incoming cancellation consumes its one-shot sender. The serving task keeps
the response right through the select of completion, deadline, or cancellation. -/

structure Inbound where
  cancelSlot : Bool
  replyRight : Bool := true
  cancelSignals : Nat := 0
  responseAttempts : Nat := 0
  deriving DecidableEq

inductive InboundEvent where
  | cancel | finish
  deriving DecidableEq

def inboundStep (s : Inbound) : InboundEvent → Inbound
  | .cancel =>
      if s.cancelSlot then { s with cancelSlot := false, cancelSignals := s.cancelSignals + 1 }
      else s
  | .finish =>
      if s.replyRight then
        { s with cancelSlot := false, replyRight := false, responseAttempts := s.responseAttempts + 1 }
      else s

def InboundInvariant (s : Inbound) : Prop :=
  s.cancelSignals + bit s.cancelSlot ≤ 1 ∧ s.responseAttempts + bit s.replyRight = 1

theorem inbound_initial (cancellable : Bool) :
    InboundInvariant { cancelSlot := cancellable } := by
  cases cancellable <;> simp [InboundInvariant, bit]

theorem inbound_preserves (s : Inbound) (event : InboundEvent)
    (h : InboundInvariant s) : InboundInvariant (inboundStep s event) := by
  rcases s with ⟨cancelSlot, replyRight, signals, attempts⟩
  cases event <;> cases cancelSlot <;> cases replyRight <;>
    simp_all [inboundStep, InboundInvariant, bit] <;> omega

def inboundRun (s : Inbound) : List InboundEvent → Inbound
  | [] => s
  | event :: rest => inboundRun (inboundStep s event) rest

theorem inbound_run_preserves (events : List InboundEvent) (s : Inbound)
    (h : InboundInvariant s) : InboundInvariant (inboundRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (inbound_preserves s event h)

theorem inbound_trace_has_single_reply_owner (events : List InboundEvent) (cancellable : Bool) :
    let s := inboundRun { cancelSlot := cancellable } events
    s.cancelSignals ≤ 1 ∧ s.responseAttempts ≤ 1 := by
  have h := inbound_run_preserves events _ (inbound_initial cancellable)
  dsimp
  unfold InboundInvariant at h
  omega

theorem cancellation_preserves_serving_tasks_reply_right (s : Inbound) :
    (inboundStep s .cancel).replyRight = s.replyRight ∧
      (inboundStep s .cancel).responseAttempts = s.responseAttempts := by
  cases hc : s.cancelSlot <;> simp [inboundStep, hc]

/-! ## Allocated-byte leases and the privileged control reserve -/

structure Usage where
  data : Nat
  control : Nat
  deriving DecidableEq

def Usage.total (u : Usage) : Nat := u.data + u.control

structure Budget where
  maximum : Nat
  reserve : Nat
  deriving DecidableEq

def Bounded (b : Budget) (u : Usage) : Prop :=
  u.data ≤ b.maximum ∧ u.control ≤ b.reserve

/-- `needed` is the saturating difference between requested and leased capacity.
The normal pool is used first, exactly as `Lease::grow` computes it. -/
def allocation (b : Budget) (u : Usage) (needed : Nat) : Usage :=
  let data := min needed (b.maximum - u.data)
  ⟨data, needed - data⟩

def canGrow (b : Budget) (u : Usage) (needed : Nat) (privileged : Bool) : Bool :=
  let a := allocation b u needed
  decide (a.control = 0 ∨ (privileged = true ∧ a.control ≤ b.reserve - u.control))

def grow (b : Budget) (u : Usage) (needed : Nat) (privileged : Bool) : Usage :=
  if canGrow b u needed privileged then
    let a := allocation b u needed
    ⟨u.data + a.data, u.control + a.control⟩
  else u

theorem allocation_conserves (b : Budget) (u : Usage) (needed : Nat) :
    (allocation b u needed).total = needed := by
  simp [allocation, Usage.total, Nat.min_def]
  split <;> omega

theorem grow_preserves_bound (b : Budget) (u : Usage) (needed : Nat)
    (privileged : Bool) (h : Bounded b u) :
    Bounded b (grow b u needed privileged) := by
  unfold grow
  split
  next accepted =>
    have guard : (allocation b u needed).control = 0 ∨
        (privileged = true ∧ (allocation b u needed).control ≤ b.reserve - u.control) := by
      simpa [canGrow] using accepted
    simp only [Bounded] at h ⊢
    simp only [allocation, Nat.min_def] at guard ⊢
    split at guard <;> split <;> omega
  next => exact h

theorem grow_total_bound (b : Budget) (u : Usage) (needed : Nat)
    (privileged : Bool) (h : Bounded b u) :
    (grow b u needed privileged).total ≤ b.maximum + b.reserve := by
  have hg := grow_preserves_bound b u needed privileged h
  unfold Bounded Usage.total at *
  omega

theorem successful_growth_charges_exact_capacity (b : Budget) (u : Usage)
    (needed : Nat) (privileged : Bool)
    (h : canGrow b u needed privileged = true) :
    (grow b u needed privileged).total = u.total + needed := by
  have ha := allocation_conserves b u needed
  simp only [grow, h, Bool.true_eq, ↓reduceIte, Usage.total] at *
  omega

theorem normal_frames_leave_reserve_unchanged (b : Budget) (u : Usage)
    (needed : Nat) : (grow b u needed false).control = u.control := by
  unfold grow
  split
  next h =>
    have hc : (allocation b u needed).control = 0 := by simpa [canGrow] using h
    simp [hc]
  next => rfl

theorem rejected_growth_preserves_usage (b : Budget) (u : Usage) (needed : Nat)
    (privileged : Bool) (h : canGrow b u needed privileged = false) :
    grow b u needed privileged = u := by simp [grow, h]

def release (u lease : Usage) : Usage :=
  ⟨u.data - lease.data, u.control - lease.control⟩

theorem release_conserves_usage (u lease : Usage)
    (hd : lease.data ≤ u.data) (hc : lease.control ≤ u.control) :
    (release u lease).total + lease.total = u.total := by
  simp only [release, Usage.total]
  omega

theorem release_preserves_bound (b : Budget) (u lease : Usage)
    (h : Bounded b u) : Bounded b (release u lease) := by
  simp only [Bounded, release] at *
  omega

inductive BudgetEvent where
  | acquire (needed : Nat) (privileged : Bool)
  | giveBack (lease : Usage)
  deriving DecidableEq

def budgetStep (b : Budget) (u : Usage) : BudgetEvent → Usage
  | .acquire n p => grow b u n p
  | .giveBack lease => release u lease

def budgetRun (b : Budget) (u : Usage) : List BudgetEvent → Usage
  | [] => u
  | e :: rest => budgetRun b (budgetStep b u e) rest

theorem budget_trace_bounded (b : Budget) (events : List BudgetEvent)
    (u : Usage) (h : Bounded b u) : Bounded b (budgetRun b u events) := by
  induction events generalizing u with
  | nil => exact h
  | cons e rest ih =>
    apply ih
    cases e with
    | acquire n p => exact grow_preserves_bound b u n p h
    | giveBack lease => exact release_preserves_bound b u lease h

/-! A list cell represents one live linear lease. Removing that cell returns
its credits. This owner model derives the subtraction preconditions above. -/

def addUsage (left right : Usage) : Usage :=
  ⟨left.data + right.data, left.control + right.control⟩

def ownedUsage : List Usage → Usage
  | [] => ⟨0, 0⟩
  | lease :: rest => addUsage lease (ownedUsage rest)

theorem owned_usage_append (front rear : List Usage) :
    ownedUsage (front ++ rear) = addUsage (ownedUsage front) (ownedUsage rear) := by
  induction front with
  | nil => simp [ownedUsage, addUsage]
  | cons lease rest ih => simp [ownedUsage, ih, addUsage, Nat.add_assoc]

theorem growing_owner_accounts_all_credits (front rear : List Usage) (owner delta : Usage) :
    ownedUsage (front ++ addUsage owner delta :: rear) =
      addUsage (ownedUsage (front ++ owner :: rear)) delta := by
  simp [owned_usage_append, ownedUsage, addUsage, Nat.add_assoc, Nat.add_comm, Nat.add_left_comm]

theorem dropping_owner_returns_exact_credits (front rear : List Usage) (owner : Usage) :
    release (ownedUsage (front ++ owner :: rear)) owner = ownedUsage (front ++ rear) := by
  simp [owned_usage_append, ownedUsage, addUsage, release]
  constructor <;> omega

theorem live_owner_is_charged (front rear : List Usage) (owner : Usage) :
    owner.data ≤ (ownedUsage (front ++ owner :: rear)).data ∧
      owner.control ≤ (ownedUsage (front ++ owner :: rear)).control := by
  simp [owned_usage_append, ownedUsage, addUsage]
  omega

inductive OwnedStep (b : Budget) : List Usage → List Usage → Prop where
  | newLease (leases : List Usage) : OwnedStep b leases (⟨0, 0⟩ :: leases)
  | growLease (front rear : List Usage) (owner : Usage) (needed : Nat) (privileged : Bool)
      (admitted : canGrow b (ownedUsage (front ++ owner :: rear)) needed privileged = true) :
      OwnedStep b (front ++ owner :: rear)
        (front ++ addUsage owner (allocation b (ownedUsage (front ++ owner :: rear)) needed) :: rear)
  | dropLease (front rear : List Usage) (owner : Usage) :
      OwnedStep b (front ++ owner :: rear) (front ++ rear)

theorem owned_step_preserves_bound (b : Budget) (leases next : List Usage)
    (h : Bounded b (ownedUsage leases)) (step : OwnedStep b leases next) :
    Bounded b (ownedUsage next) := by
  cases step with
  | newLease leases => simpa [ownedUsage, addUsage] using h
  | growLease front rear owner needed privileged admitted =>
    rw [growing_owner_accounts_all_credits]
    have grown := grow_preserves_bound b (ownedUsage (front ++ owner :: rear)) needed privileged h
    simpa [grow, admitted, addUsage] using grown
  | dropLease front rear owner =>
    rw [← dropping_owner_returns_exact_credits front rear owner]
    exact release_preserves_bound b _ owner h

inductive OwnedReachable (b : Budget) : List Usage → Prop where
  | initial : OwnedReachable b []
  | next {leases next : List Usage} :
      OwnedReachable b leases → OwnedStep b leases next → OwnedReachable b next

theorem all_owned_traces_are_bounded (b : Budget) (leases : List Usage)
    (h : OwnedReachable b leases) : Bounded b (ownedUsage leases) := by
  induction h with
  | initial => simp [Bounded, ownedUsage]
  | next previous step ih => exact owned_step_preserves_bound b _ _ ih step

/-! ## The one writer's FIFO

Byte/slot admission precedes enqueue. Control messages use the same queue;
their reserved admission capacity gives no permission to overtake a data frame.
The transition records acquisition by the writer. The implementation's next
receive occurs after the previously acquired frame completes or skips.
-/

structure FIFO (α : Type) where
  accepted : List α := []
  consumed : List α := []
  waiting : List α := []

def FIFO.Valid (s : FIFO α) : Prop := s.accepted = s.consumed ++ s.waiting

def enqueue (s : FIFO α) (value : α) : FIFO α :=
  { s with accepted := s.accepted ++ [value], waiting := s.waiting ++ [value] }

def dequeue (s : FIFO α) : FIFO α :=
  match s.waiting with
  | [] => s
  | head :: tail => { s with consumed := s.consumed ++ [head], waiting := tail }

theorem enqueue_preserves_fifo (s : FIFO α) (value : α) (h : s.Valid) :
    (enqueue s value).Valid := by
  simpa [FIFO.Valid, enqueue, h, List.append_assoc] using congrArg (· ++ [value]) h

theorem dequeue_preserves_fifo (s : FIFO α) (h : s.Valid) : (dequeue s).Valid := by
  cases hw : s.waiting with
  | nil => simpa [dequeue, hw] using h
  | cons head tail => simpa [FIFO.Valid, dequeue, hw, List.append_assoc] using h

inductive QueueEvent (α : Type) where
  | enqueue (value : α)
  | dequeue

def queueStep (s : FIFO α) : QueueEvent α → FIFO α
  | .enqueue value => enqueue s value
  | .dequeue => dequeue s

def queueRun (s : FIFO α) : List (QueueEvent α) → FIFO α
  | [] => s
  | e :: rest => queueRun (queueStep s e) rest

theorem queue_trace_is_fifo (events : List (QueueEvent α)) (s : FIFO α)
    (h : s.Valid) : (queueRun s events).Valid := by
  induction events generalizing s with
  | nil => exact h
  | cons e rest ih =>
    apply ih
    cases e with
    | enqueue value => exact enqueue_preserves_fifo s value h
    | dequeue => exact dequeue_preserves_fifo s h

end Cordis.Protocol
