import Admission

/-!
Immutable context derivation and its cached guard-presence bit. The cache records
view shape, never owner liveness or an epoch's current validity. Root and Fiber
layers terminate the owning fiber's guard segment while retaining the parent
chain for ordinary service lookup. Rust's FiberId boundary test and construction
of these layers remain an explicit representation-refinement obligation.
-/

namespace Cordis.Scopes

inductive Layer where
  | root
  | fiber
  | plain
  | epoch (captured : Epoch)
  deriving DecidableEq, Repr

structure Node where
  layer : Layer
  hasEpochGuard : Bool
  deriving DecidableEq, Repr

abbrev Scope := List Node

def cached : Scope → Bool
  | [] => false
  | node :: _ => node.hasEpochGuard

/-- Complete traversal of the owning fiber's segment, independent of the cache. -/
def guards : Scope → List Epoch
  | [] => []
  | node :: rest => match node.layer with
    | .root | .fiber => []
    | .plain => guards rest
    | .epoch captured => captured :: guards rest

def root : Scope := [⟨.root, false⟩]
def derive (scope : Scope) : Scope := ⟨.plain, cached scope⟩ :: scope
def bind (scope : Scope) (captured : Epoch) : Scope := ⟨.epoch captured, true⟩ :: scope
def child (scope : Scope) : Scope := ⟨.fiber, false⟩ :: scope

/-- Every finite history of the actual immutable view constructors is covered. -/
inductive WellFormed : Scope → Prop where
  | root : WellFormed root
  | derive {scope} : WellFormed scope → WellFormed (derive scope)
  | bind {scope} (captured) : WellFormed scope → WellFormed (bind scope captured)
  | child {scope} : WellFormed scope → WellFormed (child scope)

theorem cache_matches_guards (scope : Scope) (h : WellFormed scope) :
    cached scope = !(guards scope).isEmpty := by
  induction h with
  | root => rfl
  | derive _ ih => exact ih
  | bind _ _ => rfl
  | child _ => rfl

theorem absent_cache_has_no_guards (scope : Scope) (h : WellFormed scope)
    (absent : cached scope = false) : guards scope = [] := by
  have same := cache_matches_guards scope h
  cases hg : guards scope with
  | nil => rfl
  | cons head tail => simp [hg, absent] at same

structure Facts where
  alive : Bool
  target : Epoch
  running : Epoch

def guard (facts : Facts) (captured : Epoch) : Admission.Guard :=
  ⟨facts.alive, facts.target, facts.running, captured⟩

def slow (facts : Facts) (scope : Scope) : Bool :=
  facts.alive && (guards scope).all (fun captured => Admission.permits (guard facts captured))

theorem guard_factors_current (facts : Facts) (captured : Epoch) :
    Admission.permits (guard facts captured) =
      (current facts.alive (facts.target == facts.running) && facts.target == captured) := by
  simp [Admission.permits, guard, current_epoch_equiv, current_equiv]

theorem nonempty_guards_share_current (facts : Facts) (first : Epoch) (rest : List Epoch) :
    (first :: rest).all (fun captured => Admission.permits (guard facts captured)) =
      (current facts.alive (facts.target == facts.running) &&
        (first :: rest).all (fun captured => facts.target == captured)) := by
  cases hc : current facts.alive (facts.target == facts.running) <;>
    simp [guard_factors_current, hc]

/-- The false cache path reads only the owner's current liveness. -/
def fast (facts : Facts) (scope : Scope) : Bool :=
  if !facts.alive then false
  else if cached scope then
    current facts.alive (facts.target == facts.running) &&
      (guards scope).all (fun captured => facts.target == captured)
  else true

theorem cached_admission_equiv (facts : Facts) (scope : Scope) (h : WellFormed scope) :
    fast facts scope = slow facts scope := by
  cases ha : facts.alive with
  | false => simp [fast, slow, ha]
  | true =>
    cases hc : cached scope with
    | false =>
      have empty := absent_cache_has_no_guards scope h hc
      simp [fast, slow, ha, hc, empty]
    | true =>
      have same := cache_matches_guards scope h
      cases hg : guards scope with
      | nil => simp [hg, hc] at same
      | cons first rest =>
        have factor := nonempty_guards_share_current facts first rest
        simpa [fast, slow, ha, hc, hg] using factor.symm

theorem plain_derivation_preserves_guards (scope : Scope) :
    guards (derive scope) = guards scope := rfl

theorem plain_derivation_preserves_admission (facts : Facts) (scope : Scope) :
    fast facts (derive scope) = fast facts scope := rfl

theorem binding_keeps_ancestor_guards (scope : Scope) (captured : Epoch) :
    guards (bind scope captured) = captured :: guards scope := rfl

theorem binding_adds_a_conjunct (facts : Facts) (scope : Scope) (captured : Epoch)
    (h : WellFormed scope) :
    fast facts (bind scope captured) =
      (Admission.permits (guard facts captured) && fast facts scope) := by
  rw [cached_admission_equiv facts _ (.bind captured h), cached_admission_equiv facts scope h]
  simp [slow, guards, bind, Bool.and_assoc, Bool.and_left_comm]

theorem binding_cannot_restore_rejected_view (facts : Facts) (scope : Scope)
    (captured : Epoch) (h : WellFormed scope) (rejected : fast facts scope = false) :
    fast facts (bind scope captured) = false := by
  rw [binding_adds_a_conjunct facts scope captured h, rejected]
  simp

theorem child_starts_own_guard_segment (scope : Scope) : guards (child scope) = [] := rfl

theorem child_preserves_alive_contract (facts : Facts) (scope : Scope) :
    fast facts (child scope) = facts.alive := by
  cases ha : facts.alive <;> simp [fast, child, cached, ha]

theorem root_preserves_alive_contract (facts : Facts) : fast facts root = facts.alive := by
  cases ha : facts.alive <;> simp [fast, root, cached, ha]

def admissionState (facts : Facts) (scope : Scope) (owned : List Nat) : Admission.State :=
  ⟨facts.alive, (guards scope).map (guard facts), owned⟩

theorem traversal_matches_admission (facts : Facts) (scope : Scope) (owned : List Nat) :
    slow facts scope = Admission.admitted (admissionState facts scope owned) := by
  simp [slow, Admission.admitted, admissionState, List.all_map, Function.comp_def]

/-- Cached traversal and complete traversal produce identical ownership updates. -/
def register (facts : Facts) (scope : Scope) (owned : List Nat) (effect : Nat) :
    Admission.State × Bool :=
  let state := admissionState facts scope owned
  if fast facts scope then ({state with owned := owned ++ [effect]}, true) else (state, false)

theorem cached_registration_equiv (facts : Facts) (scope : Scope) (owned : List Nat)
    (effect : Nat) (h : WellFormed scope) :
    register facts scope owned effect =
      Admission.register (admissionState facts scope owned) effect := by
  unfold register Admission.register
  rw [cached_admission_equiv facts scope h, traversal_matches_admission facts scope owned]
  rfl

end Cordis.Scopes
