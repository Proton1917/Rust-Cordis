import Std

/-! Store directory initialization confirms each child before its ancestors.
`durable n` records successful directory-content barriers under the filesystem
contract. Path resolution, directory creation, and the meaning of a successful
OS barrier remain explicit inputs to this program model. -/
namespace Cordis.DirectoryInitialization

structure State where
  pending : List Nat
  confirmed : List Nat
  durable : Nat → Bool
  failed : Bool

def initial (required : List Nat) (durable : Nat → Bool) : State :=
  ⟨required, [], durable, false⟩

def nextIo (s : State) : Option Nat :=
  if s.failed then none else s.pending.head?

def ready (s : State) : Bool := !s.failed && s.pending.isEmpty

def returns (s : State) (success : Bool) : State :=
  if s.failed then s else
  match s.pending with
  | [] => s
  | name :: rest =>
    if success then
      { pending := rest, confirmed := s.confirmed ++ [name],
        durable := fun other => if other = name then true else s.durable other,
        failed := false }
    else { s with failed := true }

def run : State → List Bool → State
  | s, [] => s
  | s, result :: rest => run (returns s result) rest

def Invariant (required : List Nat) (s : State) : Prop :=
  required = s.confirmed ++ s.pending ∧
  ∀ name ∈ s.confirmed, s.durable name = true

theorem initial_invariant (required : List Nat) (durable : Nat → Bool) :
    Invariant required (initial required durable) := by
  simp [Invariant, initial]

theorem return_preserves_invariant (required : List Nat) (s : State)
    (success : Bool) (h : Invariant required s) :
    Invariant required (returns s success) := by
  cases hf : s.failed
  · cases hp : s.pending with
    | nil => simpa [returns, hf, hp] using h
    | cons name rest =>
      cases success
      · simpa [returns, hf, hp, Invariant] using h
      · constructor
        · simpa [returns, hf, hp, List.append_assoc] using h.1
        · intro other member
          simp only [returns, hf, Bool.false_eq_true, ↓reduceIte, hp,
            ] at member ⊢
          by_cases equal : other = name
          · simp [equal]
          · simp [equal]
            apply h.2 other
            simpa [List.mem_append, List.mem_singleton, equal] using member
  · simpa [returns, hf] using h

theorem arbitrary_returns_preserve_invariant (required : List Nat) (s : State)
    (results : List Bool) (h : Invariant required s) :
    Invariant required (run s results) := by
  induction results generalizing s with
  | nil => exact h
  | cons result rest ih =>
    exact ih (returns s result) (return_preserves_invariant required s result h)

theorem every_confirmed_name_has_successful_barrier (required : List Nat)
    (durable : Nat → Bool) (results : List Bool) :
    ∀ name ∈ (run (initial required durable) results).confirmed,
      (run (initial required durable) results).durable name = true :=
  (arbitrary_returns_preserve_invariant required _ results (initial_invariant _ _)).2

theorem confirmation_is_an_ordered_prefix (required : List Nat)
    (durable : Nat → Bool) (results : List Bool) :
    required = (run (initial required durable) results).confirmed ++
      (run (initial required durable) results).pending :=
  (arbitrary_returns_preserve_invariant required _ results (initial_invariant _ _)).1

theorem ready_confirms_every_required_directory (required : List Nat) (s : State)
    (h : Invariant required s) (done : ready s = true) :
    ∀ name ∈ required, s.durable name = true := by
  have empty : s.pending = [] := by
    cases hp : s.pending with
    | nil => rfl
    | cons name rest => simp [ready, hp] at done
  intro name member
  apply h.2 name
  simpa [h.1, empty] using member

theorem successful_initialization_confirms_entire_path (required : List Nat)
    (durable : Nat → Bool) (results : List Bool)
    (done : ready (run (initial required durable) results) = true) :
    ∀ name ∈ required, (run (initial required durable) results).durable name = true :=
  ready_confirms_every_required_directory required _
    (arbitrary_returns_preserve_invariant required _ results (initial_invariant _ _)) done

theorem failed_barrier_rejects_initialization (s : State) (name : Nat)
    (rest : List Nat) (active : s.failed = false) (pending : s.pending = name :: rest) :
    ready (returns s false) = false ∧ nextIo (returns s false) = none := by
  simp [returns, active, pending, ready, nextIo]

theorem failure_stops_remaining_barriers (s : State) (failed : s.failed = true)
    (results : List Bool) : run s results = s := by
  induction results with
  | nil => rfl
  | cons result rest ih => simpa [run, returns, failed] using ih

theorem successful_barriers_exhaust_pending (s : State) (active : s.failed = false) :
    (run s (List.replicate s.pending.length true)).pending = [] ∧
      (run s (List.replicate s.pending.length true)).failed = false := by
  generalize hp : s.pending = pending
  induction pending generalizing s with
  | nil => simpa [hp, run] using And.intro hp active
  | cons name rest ih =>
    have nextPending : (returns s true).pending = rest := by
      simp [returns, active, hp]
    have nextActive : (returns s true).failed = false := by
      simp [returns, active, hp]
    simpa [hp, List.replicate_succ, run] using ih (returns s true) nextActive nextPending

def retry (required : List Nat) (previous : State) : State := initial required previous.durable

theorem retry_repeats_every_barrier (required : List Nat) (previous : State) :
    (retry required previous).pending = required ∧
      (retry required previous).confirmed = [] ∧
      (retry required previous).failed = false := by
  simp [retry, initial]

theorem existing_durable_names_do_not_skip_confirmation (required : List Nat)
    (before after : Nat → Bool) :
    nextIo (initial required before) = nextIo (initial required after) := by
  rfl

theorem children_and_ancestors_all_confirmed (consumed data : Nat)
    (ancestors : List Nat) (durable : Nat → Bool) (results : List Bool)
    (done : ready (run (initial (consumed :: data :: ancestors) durable) results) = true) :
    let final := run (initial (consumed :: data :: ancestors) durable) results
    final.durable consumed = true ∧ final.durable data = true ∧
      ∀ ancestor ∈ ancestors, final.durable ancestor = true := by
  have all := successful_initialization_confirms_entire_path _ durable results done
  exact ⟨all consumed (by simp), all data (by simp), fun name member => all name (by simp [member])⟩

end Cordis.DirectoryInitialization
