import Lifecycle

/-!
Effect admission consumes the owner's current facts in the same operation that
records ownership. Scope traversal supplies the guards; it is a separate Rust
refinement obligation. The predicate here is the existing generated currentEpoch
decision, not a second definition of generation validity.
-/

namespace Cordis.Admission

structure Guard where
  alive : Bool
  target : Epoch
  running : Epoch
  captured : Epoch
  deriving DecidableEq

def permits (g : Guard) : Bool :=
  currentEpoch g.alive (g.target == g.running) (g.target == g.captured)

def Valid (g : Guard) : Prop :=
  g.alive = true ∧ g.target = g.running ∧ g.target = g.captured

theorem permits_iff (g : Guard) : permits g = true ↔ Valid g := by
  simp [permits, current_epoch_equiv, Valid, and_assoc]

theorem all_guards_iff (guards : List Guard) :
    guards.all permits = true ↔ ∀ guard ∈ guards, Valid guard := by
  simp [List.all_eq_true, permits_iff]

structure State where
  ownerAlive : Bool
  guards : List Guard
  owned : List Nat
  deriving DecidableEq

def admitted (s : State) : Bool := s.ownerAlive && s.guards.all permits

/-- One lock-linearized operation: failed admission leaves ownership unchanged. -/
def register (s : State) (effect : Nat) : State × Bool :=
  if admitted s then ({s with owned := s.owned ++ [effect]}, true) else (s, false)

theorem admitted_iff (s : State) :
    admitted s = true ↔ s.ownerAlive = true ∧ ∀ guard ∈ s.guards, Valid guard := by
  simp [admitted, all_guards_iff, permits_iff]

theorem successful_registration (s : State) (effect : Nat)
    (h : (register s effect).2 = true) :
    s.ownerAlive = true ∧ (∀ guard ∈ s.guards, Valid guard) ∧
      (register s effect).1.owned = s.owned ++ [effect] := by
  cases ha : admitted s with
  | false => simp [register, ha] at h
  | true =>
    obtain ⟨owner, guards⟩ := (admitted_iff s).mp ha
    exact ⟨owner, guards, by simp [register, ha]⟩

theorem rejected_registration_preserves_owner (s : State) (effect : Nat)
    (h : admitted s = false) : register s effect = (s, false) := by
  simp [register, h]

theorem stale_guard_denies_registration (s : State) (effect : Nat) (g : Guard)
    (hg : g ∈ s.guards) (stale : g.target ≠ g.captured) :
    register s effect = (s, false) := by
  have h : admitted s = false := by
    cases ha : admitted s with
    | false => rfl
    | true => exact False.elim (stale (((admitted_iff s).mp ha).2 g hg).2.2)
  exact rejected_registration_preserves_owner s effect h

theorem inactive_owner_denies_registration (s : State) (effect : Nat)
    (h : s.ownerAlive = false) : register s effect = (s, false) := by
  simp [register, admitted, h]

theorem ordinary_context_preserves_alive_contract (s : State) (h : s.guards = []) :
    admitted s = s.ownerAlive := by simp [admitted, h]

end Cordis.Admission
