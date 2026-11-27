import Std

/-!
Terminal ownership in `sdks/typescript/cordis-sdk/src/wire.ts` and
`sdks/python/cordis_sdk/wire.py`.

The TypeScript map entry is removed by `claimPending` before settlement.
Python retains the entry until `request`'s `finally`, and `_response` tests
`future.done()` before settling it. `Style` preserves that representation
difference; both execute synchronous terminal sections on one event loop.
-/

namespace Cordis.SdkProtocol

def bit (b : Bool) : Nat := if b then 1 else 0

inductive Style where
  | typescript | python
  deriving DecidableEq

structure Waiter where
  present : Bool := true
  done : Bool := false
  settlements : Nat := 0
  payloads : Nat := 0
  accepted : Nat := 0
  lateClosed : Nat := 0
  deriving DecidableEq

def active (s : Waiter) : Bool := s.present && !s.done

def settle (style : Style) (s : Waiter) : Waiter :=
  if active s then
    { s with
      present := match style with | .typescript => false | .python => s.present
      done := true
      settlements := s.settlements + 1 }
  else s

inductive Event where
  | ok | error | timeout | disconnect | cleanup
  deriving DecidableEq

def step (style : Style) (s : Waiter) : Event → Waiter
  | .ok =>
      if active s then
        { settle style s with payloads := s.payloads + 1, accepted := s.accepted + 1 }
      else { s with payloads := s.payloads + 1, lateClosed := s.lateClosed + 1 }
  | .error | .timeout | .disconnect => settle style s
  | .cleanup => { s with present := false }

def Invariant (s : Waiter) : Prop :=
  s.settlements = bit s.done ∧ s.accepted ≤ s.settlements ∧
    s.payloads = s.accepted + s.lateClosed

theorem initial_invariant : Invariant {} := by simp [Invariant, bit]

theorem settle_preserves (style : Style) (s : Waiter) (h : Invariant s) :
    Invariant (settle style s) := by
  rcases s with ⟨present, done, settlements, payloads, accepted, lateClosed⟩
  cases style <;> cases present <;> cases done <;>
    simp_all [settle, active, Invariant, bit] <;> omega

theorem step_preserves (style : Style) (s : Waiter) (event : Event)
    (h : Invariant s) : Invariant (step style s event) := by
  cases event with
  | error => exact settle_preserves style s h
  | timeout => exact settle_preserves style s h
  | disconnect => exact settle_preserves style s h
  | cleanup => exact h
  | ok =>
    rcases s with ⟨present, done, settlements, payloads, accepted, lateClosed⟩
    cases style <;> cases present <;> cases done <;>
      simp_all [step, settle, active, Invariant, bit] <;> omega

def run (style : Style) (s : Waiter) : List Event → Waiter
  | [] => s
  | event :: rest => run style (step style s event) rest

theorem run_preserves (style : Style) (events : List Event) (s : Waiter)
    (h : Invariant s) : Invariant (run style s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (step_preserves style s event h)

theorem every_sdk_trace_has_single_settlement (style : Style) (events : List Event) :
    let s := run style {} events
    s.settlements ≤ 1 ∧ s.accepted ≤ 1 ∧ s.payloads = s.accepted + s.lateClosed := by
  have h := run_preserves style events {} initial_invariant
  dsimp
  unfold Invariant at h
  have hb : bit (run style {} events).done ≤ 1 := by
    cases (run style {} events).done <;> simp [bit]
  omega

theorem typescript_claim_removes_entry (s : Waiter) (h : active s = true) :
    (settle .typescript s).present = false := by simp [settle, h]

theorem python_settlement_retains_entry_until_finally (s : Waiter)
    (h : active s = true) :
    (settle .python s).present = s.present ∧ (settle .python s).done = true := by
  simp [settle, h]

theorem repeated_terminal_cannot_settle_again (style : Style) (s : Waiter) :
    settle style (settle style s) = settle style s := by
  cases hp : s.present <;> cases hd : s.done <;> cases style <;>
    simp [settle, active, hp, hd]

theorem late_payload_has_one_release_owner (style : Style) (s : Waiter)
    (h : active s = false) :
    (step style s .ok).lateClosed = s.lateClosed + 1 ∧
      (step style s .ok).accepted = s.accepted ∧
      (step style s .ok).settlements = s.settlements := by
  simp [step, h]

theorem cleanup_removes_pending_entry (style : Style) (s : Waiter) :
    (step style s .cleanup).present = false := by rfl

/-! ## TypeScript's payload between result settlement and send completion

`Promise.all([sent, completed])` transfers a received payload to the caller only
after sending has succeeded. The catch path closes an already received payload.
This is a separate owner from the late-response close path above.
-/

inductive PayloadOwner where
  | waiting | caller | closed
  deriving DecidableEq

structure Delivery where
  owner : PayloadOwner := .waiting
  returns : Nat := 0
  closes : Nat := 0
  deriving DecidableEq

inductive DeliveryEvent where
  | sendSucceeded | sendFailed
  deriving DecidableEq

def deliveryStep (s : Delivery) (event : DeliveryEvent) : Delivery :=
  match s.owner, event with
  | .waiting, .sendSucceeded => { s with owner := .caller, returns := s.returns + 1 }
  | .waiting, .sendFailed => { s with owner := .closed, closes := s.closes + 1 }
  | _, _ => s

def Delivery.Valid (s : Delivery) : Prop :=
  match s.owner with
  | .waiting => s.returns = 0 ∧ s.closes = 0
  | .caller => s.returns = 1 ∧ s.closes = 0
  | .closed => s.returns = 0 ∧ s.closes = 1

theorem delivery_initial : Delivery.Valid {} := by simp [Delivery.Valid]

theorem delivery_preserves (s : Delivery) (event : DeliveryEvent) (h : s.Valid) :
    (deliveryStep s event).Valid := by
  cases ho : s.owner <;> cases event <;> simp_all [deliveryStep, Delivery.Valid]

def deliveryRun (s : Delivery) : List DeliveryEvent → Delivery
  | [] => s
  | event :: rest => deliveryRun (deliveryStep s event) rest

theorem delivery_run_preserves (events : List DeliveryEvent) (s : Delivery)
    (h : s.Valid) : (deliveryRun s events).Valid := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (delivery_preserves s event h)

theorem accepted_payload_never_returns_and_closes (events : List DeliveryEvent) :
    let s := deliveryRun {} events
    s.returns + s.closes ≤ 1 := by
  have h := delivery_run_preserves events {} delivery_initial
  dsimp
  cases ho : (deliveryRun {} events).owner <;> simp_all [Delivery.Valid] <;> omega

end Cordis.SdkProtocol
