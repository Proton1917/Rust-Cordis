import NativeOwnership

/-!
Small-step lease operations. A clone borrows one existing counted owner until its
CAS succeeds, fails at the integer limit, or is abandoned. This is the ABI clone
contract, represented as a resource transfer rather than a safety hypothesis.

An atomic load may return ANY earlier value in the count modification history.
The model admits stale observations and assumes no cross-object total order for
the retired flag and count. A successful RMW compares against the current count;
failed CAS returns the anchor to its retry phase. Reaping separates the retired
read, count read and image-lock take.
-/
set_option maxRecDepth 2048

namespace Cordis.Concurrency

structure State where
  count : Nat := 1
  held : Nat := 1
  reading : Nat := 0
  attempts : List Nat := []
  history : List Nat := [1]
  retired : Bool := false
  seenRetired : Nat := 0
  ready : Nat := 0
  descriptor : Option Bool := none
  unload : Bool := true
  image : Bool := true
  closes : Nat := 0
  deriving DecidableEq

def balance (s : State) : Nat := s.held + s.reading + s.attempts.length

def Invariant (limit : Nat) (s : State) : Prop :=
  s.count = balance s ∧ s.count ≤ limit ∧
  (0 ∈ s.history → s.count = 0) ∧
  (0 < s.seenRetired → s.retired = true) ∧
  (0 < s.ready → s.retired = true ∧ s.count = 0) ∧
  (s.image = false → s.retired = true ∧ s.count = 0 ∧
    s.unload = true ∧ s.descriptor = some true) ∧
  s.closes + NativeOwnership.bit s.image = 1

/-- Premises are operational guards or possession of a concrete token. -/
inductive Step (limit : Nat) : State → State → Prop where
  | borrow (s) (owned : 0 < s.held) :
      Step limit s { s with held := s.held - 1, reading := s.reading + 1 }
  | load (s) (expected) (owned : 0 < s.reading) (observed : expected ∈ s.history) :
      Step limit s { s with reading := s.reading - 1, attempts := expected :: s.attempts }
  | exchange (s) (before after) (expected)
      (owned : s.attempts = before ++ expected :: after)
      (expectedEq : expected = s.count) (space : s.count < limit) :
      Step limit s { s with
        count := s.count + 1
        held := s.held + 2
        attempts := before ++ after
        history := (s.count + 1) :: s.history }
  | retry (s) (before after) (expected)
      (owned : s.attempts = before ++ expected :: after) (changed : expected ≠ s.count) :
      Step limit s { s with attempts := before ++ after, reading := s.reading + 1 }
  | overflow (s) (before after) (expected)
      (owned : s.attempts = before ++ expected :: after)
      (expectedEq : expected = s.count) (full : s.count = limit) :
      Step limit s { s with attempts := before ++ after, held := s.held + 1 }
  | abandon (s) (owned : 0 < s.reading) :
      Step limit s { s with held := s.held + 1, reading := s.reading - 1 }
  | release (s) (owned : 0 < s.held) :
      Step limit s { s with
        count := s.count - 1
        held := s.held - 1
        history := (s.count - 1) :: s.history }
  | retire (s) : Step limit s { s with retired := true }
  | retiredTrue (s) (observed : s.retired = true) :
      Step limit s { s with seenRetired := s.seenRetired + 1 }
  | retiredFalse (s) : Step limit s s
  | countZero (s) (reading : 0 < s.seenRetired) (observed : 0 ∈ s.history) :
      Step limit s { s with seenRetired := s.seenRetired - 1, ready := s.ready + 1 }
  | countNonzero (s) (value) (reading : 0 < s.seenRetired)
      (observed : value ∈ s.history) (nonzero : value ≠ 0) :
      Step limit s { s with seenRetired := s.seenRetired - 1 }
  | descriptor (s) (result) (unique : s.descriptor = none) :
      Step limit s { s with descriptor := some result }
  | takeImage (s) (ready : 0 < s.ready) (policy : s.unload = true)
      (released : s.descriptor = some true) (present : s.image = true) :
      Step limit s { s with ready := s.ready - 1, image := false, closes := s.closes + 1 }
  | skipImage (s) (ready : 0 < s.ready) :
      Step limit s { s with ready := s.ready - 1 }

theorem initial_invariant (limit : Nat) (positive : 0 < limit) : Invariant limit {} := by
  simp [Invariant, balance, NativeOwnership.bit]
  omega

theorem step_preserves (limit : Nat) (s t : State)
    (step : Step limit s t) (h : Invariant limit s) : Invariant limit t := by
  rcases h with ⟨balanceEq, bound, zero, retired, ready, closed, closes⟩
  cases step <;> simp_all [Invariant, balance, NativeOwnership.bit]
  all_goals try omega
  refine ⟨by omega, by omega, ?_⟩
  intro hz
  rcases hz with hz | hz
  · omega
  · have hz := zero hz; omega

inductive Trace (limit : Nat) : State → State → Prop where
  | refl (s) : Trace limit s s
  | tail {s t u} : Trace limit s t → Step limit t u → Trace limit s u

theorem trace_preserves (limit : Nat) (s t : State)
    (trace : Trace limit s t) (h : Invariant limit s) : Invariant limit t := by
  induction trace with
  | refl => exact h
  | tail trace step ih => exact step_preserves limit _ _ step ih

theorem zero_is_absorbing (limit : Nat) (s t : State)
    (step : Step limit s t) (h : Invariant limit s) (zero : s.count = 0) : t.count = 0 := by
  have balanceEq := h.1
  have absent : s.attempts = [] := by
    have len : s.attempts.length = 0 := by simp only [balance] at balanceEq; omega
    exact List.length_eq_zero_iff.mp len
  cases step with
  | exchange before after expected owned expectedEq space =>
    have len := congrArg List.length owned
    simp [absent] at len
    omega
  | release owned => simp [zero]
  | borrow owned => exact zero
  | load expected owned observed => exact zero
  | retry before after expected owned changed => exact zero
  | overflow before after expected owned expectedEq full => exact zero
  | abandon owned => exact zero
  | retire => exact zero
  | retiredTrue observed => exact zero
  | retiredFalse => exact zero
  | countZero reading observed => exact zero
  | countNonzero value reading observed nonzero => exact zero
  | descriptor result unique => exact zero
  | takeImage ready policy released present => exact zero
  | skipImage ready => exact zero

theorem borrowed_anchor_blocks_zero (limit : Nat) (s : State)
    (h : Invariant limit s) (borrowed : 0 < s.reading + s.attempts.length) : 0 < s.count := by
  have balanceEq := h.1
  simp [balance] at balanceEq
  omega

theorem stale_zero_is_current_zero (limit : Nat) (s : State)
    (h : Invariant limit s) (stale : 0 ∈ s.history) : s.count = 0 := h.2.2.1 stale

theorem cas_clone_has_live_anchor (limit : Nat) (s : State)
    (h : Invariant limit s) (before after : List Nat) (expected : Nat)
    (owned : s.attempts = before ++ expected :: after) : 0 < s.count := by
  have balanceEq := h.1
  simp [balance, owned] at balanceEq
  omega

theorem image_take_has_no_inflight_clone (limit : Nat) (s : State)
    (h : Invariant limit s) (ready : 0 < s.ready) :
    s.count = 0 ∧ s.held = 0 ∧ s.reading = 0 ∧ s.attempts = [] := by
  have count := (h.2.2.2.2.1 ready).2
  have balanceEq := h.1
  simp [balance, count] at balanceEq
  exact ⟨count, by omega, by omega, List.length_eq_zero_iff.mp (by omega)⟩

theorem arbitrary_interleavings_keep_image_safe (limit : Nat) (positive : 0 < limit)
    (s : State) (trace : Trace limit {} s) (closed : s.image = false) :
    s.retired = true ∧ s.count = 0 ∧ s.held = 0 ∧ s.reading = 0 ∧ s.attempts = [] := by
  have h := trace_preserves limit {} s trace (initial_invariant limit positive)
  have state := h.2.2.2.2.2.1 closed
  have balanceEq := h.1
  simp [balance, state.2.1] at balanceEq
  exact ⟨state.1, state.2.1, by omega, by omega, List.length_eq_zero_iff.mp (by omega)⟩

theorem image_take_occurs_at_most_once (limit : Nat) (positive : 0 < limit)
    (s : State) (trace : Trace limit {} s) : s.closes ≤ 1 := by
  have h := trace_preserves limit {} s trace (initial_invariant limit positive)
  have count := h.2.2.2.2.2.2
  omega

/-- A successful RMW is the clone linearization point, following all stale reads. -/
theorem exchange_refines_abstract_acquire (limit : Nat) (s : State)
    (h : Invariant limit s) (before after : List Nat) (expected : Nat)
    (owned : s.attempts = before ++ expected :: after) (space : s.count < limit) :
    (NativeOwnership.leaseStep limit
      { count := s.count, retired := s.retired, unload := s.unload,
        descriptor := s.descriptor, libraryPresent := s.image, closes := s.closes }
      .acquire).count = s.count + 1 := by
  have positive := cas_clone_has_live_anchor limit s h before after expected owned
  simp [NativeOwnership.leaseStep, Nat.ne_of_gt positive, space]

theorem release_refines_abstract_release (limit : Nat) (s : State)
    (h : Invariant limit s) (owned : 0 < s.held) :
    (NativeOwnership.leaseStep limit
      { count := s.count, retired := s.retired, unload := s.unload,
        descriptor := s.descriptor, libraryPresent := s.image, closes := s.closes }
      .release).count = s.count - 1 := by
  have balanceEq := h.1
  have positive : 0 < s.count := by simp [balance] at balanceEq; omega
  simp [NativeOwnership.leaseStep, positive]

end Cordis.Concurrency
