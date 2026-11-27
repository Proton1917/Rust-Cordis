import Std

/-!
Native callback destruction, admission accounting and retired library leases.
The models retain the checked integer boundaries and the field destruction order
in `cordis-host`. Atomic operations are represented by their linearized effects;
the Rust memory model, Tokio and dynamic loader remain implementation boundaries.
-/

namespace Cordis.NativeOwnership

def bit (b : Bool) : Nat := if b then 1 else 0

structure Callback where
  nativeAlive : Bool := true
  permitHeld : Bool := true
  polling : Bool := false
  cancelled : Bool := false
  destroyed : Nat := 0
  returned : Nat := 0
  deriving DecidableEq

inductive CallbackEvent where
  | poll | pending | ready | cancel | destroy | returnPermit
  deriving DecidableEq

def callbackStep (s : Callback) : CallbackEvent → Callback
  | .poll => if s.nativeAlive then { s with polling := true } else s
  | .pending | .ready => { s with polling := false }
  | .cancel => { s with cancelled := true }
  | .destroy =>
    if s.nativeAlive && !s.polling then
      { s with nativeAlive := false, destroyed := s.destroyed + 1 }
    else s
  | .returnPermit =>
    if !s.nativeAlive && s.permitHeld then
      { s with permitHeld := false, returned := s.returned + 1 }
    else s

def CallbackInvariant (s : Callback) : Prop :=
  (s.nativeAlive = true → s.permitHeld = true) ∧
  (s.polling = true → s.nativeAlive = true) ∧
  s.destroyed + bit s.nativeAlive = 1 ∧ s.returned + bit s.permitHeld = 1

theorem initial_callback_invariant : CallbackInvariant {} := by
  simp [CallbackInvariant, bit]

theorem callback_step_preserves (s : Callback) (event : CallbackEvent)
    (h : CallbackInvariant s) : CallbackInvariant (callbackStep s event) := by
  cases ha : s.nativeAlive <;> cases hh : s.permitHeld <;> cases hp : s.polling <;>
    cases event <;> simp_all [callbackStep, CallbackInvariant, bit] <;> omega

def callbackRun (s : Callback) : List CallbackEvent → Callback
  | [] => s
  | event :: rest => callbackRun (callbackStep s event) rest

theorem callback_trace_preserves (s : Callback) (events : List CallbackEvent)
    (h : CallbackInvariant s) : CallbackInvariant (callbackRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (callback_step_preserves s event h)

theorem callback_capacity_outlives_native (events : List CallbackEvent)
    (alive : (callbackRun {} events).nativeAlive = true) :
    (callbackRun {} events).permitHeld = true :=
  (callback_trace_preserves {} events initial_callback_invariant).1 alive

theorem synchronous_poll_holds_capacity (events : List CallbackEvent)
    (polling : (callbackRun {} events).polling = true) :
    (callbackRun {} events).permitHeld = true := by
  have h := callback_trace_preserves {} events initial_callback_invariant
  exact h.1 (h.2.1 polling)

theorem callback_returns_at_most_once (events : List CallbackEvent) :
    (callbackRun {} events).returned ≤ 1 := by
  have h := (callback_trace_preserves {} events initial_callback_invariant).2.2.2
  omega

theorem cancellation_does_not_return_capacity (s : Callback) :
    (callbackStep s .cancel).permitHeld = s.permitHeld ∧
      (callbackStep s .cancel).returned = s.returned := by simp [callbackStep]

theorem return_requires_destroyed_future (s : Callback) (h : CallbackInvariant s)
    (returned : s.returned = 1) : s.nativeAlive = false := by
  cases ha : s.nativeAlive <;> cases hh : s.permitHeld <;>
    simp_all [CallbackInvariant, bit]

structure Capacity where
  maxActive : Nat
  maxBytes : Nat
  maxWord : Nat
  deriving DecidableEq

structure Usage where
  active : Nat := 0
  bytes : Nat := 0
  held : List Nat := []
  deriving DecidableEq

def UsageInvariant (c : Capacity) (s : Usage) : Prop :=
  s.active = s.held.length ∧ s.bytes = s.held.sum ∧
  s.active ≤ c.maxActive ∧ s.bytes ≤ c.maxBytes ∧ s.bytes ≤ c.maxWord

theorem sum_append (xs ys : List Nat) : (xs ++ ys).sum = xs.sum + ys.sum := by
  induction xs with
  | nil => simp
  | cons x xs ih => simp [ih, Nat.add_assoc]

/-- `held` is the ghost ledger of live Permit objects, each with its byte charge. -/
inductive UsageStep (c : Capacity) : Usage → Usage → Prop where
  | acquire (s : Usage) (bytes : Nat)
      (active : s.active < c.maxActive)
      (limit : s.bytes + bytes ≤ c.maxBytes)
      (word : s.bytes + bytes ≤ c.maxWord) :
      UsageStep c s
        { active := s.active + 1, bytes := s.bytes + bytes, held := s.held ++ [bytes] }
  | release (s : Usage) (before after : List Nat) (bytes : Nat)
      (owned : s.held = before ++ bytes :: after) :
      UsageStep c s
        { active := s.active - 1, bytes := s.bytes - bytes, held := before ++ after }

theorem initial_usage_invariant (c : Capacity) : UsageInvariant c {} := by
  simp [UsageInvariant]

theorem usage_step_preserves (c : Capacity) (s t : Usage)
    (step : UsageStep c s t) (h : UsageInvariant c s) : UsageInvariant c t := by
  cases step with
  | acquire bytes active limit word =>
    simp_all [UsageInvariant, sum_append]
    omega
  | release before after bytes owned =>
    simp_all [UsageInvariant, sum_append]
    omega

inductive UsageTrace (c : Capacity) : Usage → Usage → Prop where
  | refl (s) : UsageTrace c s s
  | tail {s t u} : UsageTrace c s t → UsageStep c t u → UsageTrace c s u

theorem usage_trace_preserves (c : Capacity) (s t : Usage)
    (trace : UsageTrace c s t) (h : UsageInvariant c s) : UsageInvariant c t := by
  induction trace with
  | refl => exact h
  | tail trace step ih => exact usage_step_preserves c _ _ step ih

/-- Arbitrary-order release cannot underflow either Rust accounting counter. -/
theorem release_has_capacity (c : Capacity) (s : Usage)
    (h : UsageInvariant c s) (before after : List Nat) (bytes : Nat)
    (owned : s.held = before ++ bytes :: after) : 0 < s.active ∧ bytes ≤ s.bytes := by
  simp [UsageInvariant, owned, sum_append] at h
  omega

theorem admitted_usage_is_bounded (c : Capacity) (s : Usage)
    (trace : UsageTrace c {} s) :
    s.active ≤ c.maxActive ∧ s.bytes ≤ c.maxBytes ∧ s.bytes ≤ c.maxWord :=
  (usage_trace_preserves c {} s trace (initial_usage_invariant c)).2.2

/-! The descriptor initially owns one lease. A successfully released descriptor
permits image unload only after retirement and the last counted lease release.
Cache-copy retirement is independent of the configured image unload policy. -/

structure Lease where
  count : Nat := 1
  retired : Bool := false
  unload : Bool
  descriptor : Option Bool := none
  libraryPresent : Bool := true
  cachePresent : Bool := true
  closes : Nat := 0
  deriving DecidableEq

inductive LeaseEvent where
  | acquire | release | retire | reap
  | descriptorResult (success : Bool)
  deriving DecidableEq

def leaseStep (maxWord : Nat) (s : Lease) : LeaseEvent → Lease
  | .acquire =>
    if ¬ (s.count = 0 ∧ s.retired = true) ∧ s.count < maxWord then
      { s with count := s.count + 1 }
    else s
  | .release => if 0 < s.count then { s with count := s.count - 1 } else s
  | .retire => { s with retired := true }
  | .descriptorResult success =>
    if s.descriptor = none then { s with descriptor := some success } else s
  | .reap =>
    if s.retired = true ∧ s.count = 0 then
      if s.unload = true ∧ s.descriptor = some true ∧ s.libraryPresent = true then
        { s with libraryPresent := false, cachePresent := false, closes := s.closes + 1 }
      else { s with cachePresent := false }
    else s

def LeaseInvariant (maxWord : Nat) (s : Lease) : Prop :=
  s.count ≤ maxWord ∧
  (s.libraryPresent = false →
    s.retired = true ∧ s.count = 0 ∧ s.unload = true ∧ s.descriptor = some true) ∧
  (s.cachePresent = false → s.retired = true ∧ s.count = 0) ∧
  s.closes + bit s.libraryPresent = 1

theorem initial_lease_invariant (maxWord : Nat) (positive : 0 < maxWord)
    (unload : Bool) : LeaseInvariant maxWord { unload := unload } := by
  simp [LeaseInvariant, bit]
  omega

theorem lease_step_preserves (maxWord : Nat) (s : Lease) (event : LeaseEvent)
    (h : LeaseInvariant maxWord s) : LeaseInvariant maxWord (leaseStep maxWord s event) := by
  cases event <;> simp only [leaseStep]
  all_goals repeat first | split
  all_goals simp_all [LeaseInvariant, bit]
  all_goals try omega
  all_goals cases hl : s.libraryPresent <;> cases hc : s.cachePresent <;> simp_all
  all_goals omega

def leaseRun (maxWord : Nat) (s : Lease) : List LeaseEvent → Lease
  | [] => s
  | event :: rest => leaseRun maxWord (leaseStep maxWord s event) rest

theorem lease_trace_preserves (maxWord : Nat) (s : Lease) (events : List LeaseEvent)
    (h : LeaseInvariant maxWord s) : LeaseInvariant maxWord (leaseRun maxWord s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (lease_step_preserves maxWord s event h)

/-- This implication follows from arbitrary traces, including acquire/reap races
once represented in their atomic order. No safety condition is supplied by a caller. -/
theorem image_closes_only_when_safe (maxWord : Nat) (positive : 0 < maxWord)
    (unload : Bool) (events : List LeaseEvent)
    (closed : (leaseRun maxWord { unload := unload } events).libraryPresent = false) :
    let s := leaseRun maxWord { unload := unload } events
    s.retired = true ∧ s.count = 0 ∧ s.unload = true ∧ s.descriptor = some true :=
  (lease_trace_preserves maxWord _ events
    (initial_lease_invariant maxWord positive unload)).2.1 closed

theorem image_closes_at_most_once (maxWord : Nat) (positive : 0 < maxWord)
    (unload : Bool) (events : List LeaseEvent) :
    (leaseRun maxWord { unload := unload } events).closes ≤ 1 := by
  have h := (lease_trace_preserves maxWord _ events
    (initial_lease_invariant maxWord positive unload)).2.2.2
  omega

theorem live_lease_prevents_reaping (maxWord : Nat) (s : Lease)
    (live : 0 < s.count) : leaseStep maxWord s .reap = s := by
  simp [leaseStep, Nat.ne_of_gt live]

theorem retired_zero_cannot_be_reacquired (maxWord : Nat) (s : Lease)
    (zero : s.count = 0) (retired : s.retired = true) :
    leaseStep maxWord s .acquire = s := by simp [leaseStep, zero, retired]

theorem checked_lease_acquisition (maxWord : Nat) (s : Lease)
    (full : s.count = maxWord) : leaseStep maxWord s .acquire = s := by
  simp [leaseStep, full]

theorem failed_descriptor_pins_image (maxWord : Nat) (s : Lease)
    (failed : s.descriptor = some false) :
    (leaseStep maxWord s .reap).libraryPresent = s.libraryPresent := by
  simp only [leaseStep]
  split <;> simp [failed]

theorem never_policy_pins_image (maxWord : Nat) (s : Lease) (policy : s.unload = false) :
    (leaseStep maxWord s .reap).libraryPresent = s.libraryPresent := by
  simp only [leaseStep]
  split <;> simp [policy]

theorem quiet_retirement_returns_cache (maxWord : Nat) (s : Lease)
    (zero : s.count = 0) (retired : s.retired = true) :
    (leaseStep maxWord s .reap).cachePresent = false := by
  simp only [leaseStep, zero, retired, and_self, if_true]
  split <;> rfl

end Cordis.NativeOwnership
