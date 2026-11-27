import Std
import Effects

/-!
The paper's effect algebra, and the operational interpretation used by
`Effect::defer`/`clean_steps`.  A callback inverse only needs to restore the
state at which that callback ran.  Global inverses are a stronger, explicit
domain used by `IsoEffect`; they are not silently inferred from Definition 6.
-/
namespace Cordis.RevertibleEffects

universe u

/-- A globally reversible context transformation (paper §3.1.1). -/
structure Iso (α : Type u) where
  forward : α → α
  backward : α → α
  backward_forward : ∀ x, backward (forward x) = x
  forward_backward : ∀ x, forward (backward x) = x

namespace Iso

def identity : Iso α := ⟨id, id, fun _ => rfl, fun _ => rfl⟩
def reverse (f : Iso α) : Iso α :=
  ⟨f.backward, f.forward, f.forward_backward, f.backward_forward⟩
/-- Composition is read right to left, as in the paper. -/
def compose (f g : Iso α) : Iso α :=
  ⟨fun x => f.forward (g.forward x), fun x => g.backward (f.backward x),
   fun x => by dsimp; rw [f.backward_forward, g.backward_forward],
   fun x => by dsimp; rw [g.forward_backward, f.forward_backward]⟩

@[ext] theorem ext (f g : Iso α) (hf : f.forward = g.forward)
    (hb : f.backward = g.backward) : f = g := by
  cases f; cases g; simp_all

theorem compose_associative (f g h : Iso α) :
    compose (compose f g) h = compose f (compose g h) := rfl

theorem compose_identity (f : Iso α) : compose f identity = f := by cases f; rfl
theorem identity_compose (f : Iso α) : compose identity f = f := by cases f; rfl

theorem compose_reverse (f : Iso α) : compose f (reverse f) = identity := by
  apply ext <;> funext x
  · exact f.forward_backward x
  · exact f.forward_backward x

theorem reverse_compose (f : Iso α) : compose (reverse f) f = identity := by
  apply ext <;> funext x
  · exact f.backward_forward x
  · exact f.backward_forward x

end Iso

/-- Definition 1, on the full symmetric-group domain. -/
abbrev EffectContext (α : Type u) := α × Iso α

/-- Definition 2. -/
def track (f : Iso α) (s : EffectContext α) : EffectContext α :=
  (f.forward s.1, Iso.compose s.2 (Iso.reverse f))

/-- Theorem 3: the first projection commutes with tracking. -/
theorem track_projection (f : Iso α) (s : EffectContext α) :
    (track f s).1 = f.forward s.1 := rfl

/-- Theorem 4: tracking is a homomorphism of transformations. -/
theorem track_composition (f g : Iso α) (s : EffectContext α) :
    track (Iso.compose f g) s = track f (track g s) := rfl

theorem track_identity (s : EffectContext α) : track Iso.identity s = s := by
  rcases s with ⟨x, r⟩
  simp [track, Iso.reverse, Iso.identity, Iso.compose]

theorem track_reverse (f : Iso α) (s : EffectContext α) :
    track (Iso.reverse f) (track f s) = s := by
  rw [← track_composition, Iso.reverse_compose, track_identity]

theorem reverse_track (f : Iso α) (s : EffectContext α) :
    track f (track (Iso.reverse f) s) = s := by
  rw [← track_composition, Iso.compose_reverse, track_identity]

def trackIso (f : Iso α) : Iso (EffectContext α) :=
  ⟨track f, track (Iso.reverse f), track_reverse f, reverse_track f⟩

theorem track_injective (f g : Iso α) (same : track f = track g) : f = g := by
  have hf : f.forward = g.forward := by
    funext x
    exact congrArg Prod.fst (congrFun same (x, Iso.identity))
  apply Iso.ext f g hf
  funext x
  have h := congrFun hf (g.backward x)
  rw [g.forward_backward] at h
  calc
    f.backward x = f.backward (f.forward (g.backward x)) := congrArg f.backward h.symm
    _ = g.backward x := f.backward_forward _

/-- Definition 5. -/
def recover (s : EffectContext α) : EffectContext α :=
  (s.2.forward s.1, Iso.identity)

theorem recover_track (f : Iso α) (s : EffectContext α) :
    recover (track f s) = recover s := by
  simp [recover, track, Iso.compose, Iso.reverse, f.backward_forward]

theorem recover_idempotent (s : EffectContext α) :
    recover (recover s) = recover s := rfl

def trackRun (s : EffectContext α) : List (Iso α) → EffectContext α
  | [] => s
  | f :: rest => trackRun (track f s) rest

theorem recover_track_run (s : EffectContext α) (effects : List (Iso α)) :
    recover (trackRun s effects) = recover s := by
  induction effects generalizing s with
  | nil => rfl
  | cons f rest ih => rw [trackRun, ih, recover_track]

theorem tracking_recovers_initial_state (initial : α) (effects : List (Iso α)) :
    recover (trackRun (initial, Iso.identity) effects) = (initial, Iso.identity) := by
  rw [recover_track_run]; rfl

/-- Definition 6, including state-dependent one-shot recovery functions. -/
abbrev Effect (α : Type u) := α → α × (α → α)
def Strict (e : Effect α) : Prop := ∀ x, (e x).2 (e x).1 = x

def identityEffect : Effect α := fun x => (x, id)

/-- Definition 9: `f` runs after `g`; inverses run in reverse order. -/
def compose (f g : Effect α) : Effect α := fun x =>
  let first := g x
  let second := f first.1
  (second.1, fun y => first.2 (second.2 y))

/-- Theorem 10 holds on the full one-shot left-inverse domain. -/
theorem strict_composition (f g : Effect α) (hf : Strict f) (hg : Strict g) :
    Strict (compose f g) := by
  intro x
  simp only [compose]
  rw [hf, hg]

theorem effect_composition_associative (f g h : Effect α) :
    compose (compose f g) h = compose f (compose g h) := rfl

theorem effect_composition_identity (e : Effect α) : compose e identityEffect = e := by
  funext x; rfl

theorem effect_identity_composition (e : Effect α) : compose identityEffect e = e := by
  funext x; rfl

/-- A counterexample to inferring a bijective inverse from Definition 6 alone. -/
def overwriteFalse : Effect Bool := fun before => (false, fun _ => before)

theorem overwrite_false_is_strict : Strict overwriteFalse := fun _ => rfl

theorem strict_inverse_need_not_be_injective :
    ¬ (∀ a b : Bool, (overwriteFalse true).2 a = (overwriteFalse true).2 b → a = b) := by
  intro injective
  have impossible := injective true false rfl
  cases impossible

/-- General callback tracking stores an accumulated recovery function. -/
abbrev Journal (α : Type u) := α × (α → α)

def record (e : Effect α) (s : Journal α) : Journal α :=
  let result := e s.1
  (result.1, fun x => s.2 (result.2 x))

def recoverJournal (s : Journal α) : Journal α := (s.2 s.1, id)

theorem recorded_effect_preserves_recovery (e : Effect α) (h : Strict e) (s : Journal α) :
    recoverJournal (record e s) = recoverJournal s := by
  simp only [recoverJournal, record]
  rw [h]

theorem journal_recovery_idempotent (s : Journal α) :
    recoverJournal (recoverJournal s) = recoverJournal s := rfl

/-- Capture the previous journal, as a LIFO pop does.  No global inverse of the
callback's recovery function is required.  Arbitrary selective removal instead
requires the commutation/ownership conditions proved below. -/
def lift (e : Effect α) : Effect (Journal α) := fun s =>
  let result := e s.1
  ((result.1, fun x => s.2 (result.2 x)), fun after => (result.2 after.1, s.2))

theorem operational_lift_is_strict (e : Effect α) (h : Strict e) : Strict (lift e) := by
  intro s
  simp only [lift]
  rw [h]

theorem operational_lift_projection (e : Effect α) (s : Journal α) :
    (lift e s).1 = record e s := rfl

/-- The operational, one-shot interpretation of Theorem 11. -/
theorem operational_lift_composition (f g : Effect α) :
    compose (lift f) (lift g) = lift (compose f g) := rfl

/-- The exact `track(g)` formula in Definition 7 has a well-typed domain when
returned recovery functions are invertible context transformations. -/
abbrev IsoEffect (α : Type u) := α → α × Iso α

def IsoStrict (e : IsoEffect α) : Prop := ∀ x, (e x).2.forward (e x).1 = x

def isoCompose (f g : IsoEffect α) : IsoEffect α := fun x =>
  let first := g x
  let second := f first.1
  (second.1, Iso.compose first.2 second.2)

def isoLift (e : IsoEffect α) : IsoEffect (EffectContext α) := fun s =>
  let result := e s.1
  ((result.1, Iso.compose s.2 result.2), trackIso result.2)

/-- Theorem 8 with the explicit global-inverse domain of `track`. -/
theorem iso_lift_is_strict (e : IsoEffect α) (h : IsoStrict e) :
    IsoStrict (isoLift e) := by
  intro s
  simp only [isoLift, trackIso, track]
  apply Prod.ext
  · exact h s.1
  · rw [Iso.compose_associative, Iso.compose_reverse, Iso.compose_identity]

theorem iso_composition_is_strict (f g : IsoEffect α)
    (hf : IsoStrict f) (hg : IsoStrict g) : IsoStrict (isoCompose f g) := by
  intro x
  simp only [isoCompose, Iso.compose]
  rw [hf, hg]

/-- Theorem 11 on the explicit `track` domain. -/
theorem iso_lift_composition (f g : IsoEffect α) :
    isoCompose (isoLift f) (isoLift g) = isoLift (isoCompose f g) := by
  funext s
  apply Prod.ext
  · rfl
  · apply Iso.ext <;> rfl

/-- Effect iterators are observed at a finite sequence of completed yield
boundaries.  Continuations and in-flight execution are modeled by Effects and
AsyncSemantics; every completed prefix has this algebraic interpretation. -/
def recordRun (s : Journal α) : List (Effect α) → Journal α
  | [] => s
  | e :: rest => recordRun (record e s) rest

theorem strict_trace_preserves_recovery (s : Journal α) (effects : List (Effect α))
    (strict : ∀ e ∈ effects, Strict e) :
    recoverJournal (recordRun s effects) = recoverJournal s := by
  induction effects generalizing s with
  | nil => rfl
  | cons e rest ih =>
    rw [recordRun, ih _ (fun f hf => strict f (by simp [hf]))]
    exact recorded_effect_preserves_recovery e (strict e (by simp)) s

theorem completed_prefix_recovers (initial : α) (completed : List (Effect α))
    (strict : ∀ e ∈ completed, Strict e) :
    recoverJournal (recordRun (initial, id) completed) = (initial, id) := by
  rw [strict_trace_preserves_recovery _ _ strict]; rfl

theorem record_run_append (s : Journal α) (first second : List (Effect α)) :
    recordRun s (first ++ second) = recordRun (recordRun s first) second := by
  induction first generalizing s with
  | nil => rfl
  | cons e rest ih => exact ih (record e s)

/-- An executed effect trace records the actual inverse returned at each step. -/
def execute (s : α) : List (Effect α) → α × List (α → α)
  | [] => (s, [])
  | e :: rest =>
    let current := e s
    let future := execute current.1 rest
    (future.1, future.2 ++ [current.2])

def undo : α → List (α → α) → α
  | s, [] => s
  | s, f :: rest => undo (f s) rest

theorem undo_append (s : α) (first second : List (α → α)) :
    undo s (first ++ second) = undo (undo s first) second := by
  induction first generalizing s with
  | nil => rfl
  | cons f rest ih => exact ih (f s)

theorem lifo_execution_recovers (s : α) (effects : List (Effect α))
    (strict : ∀ e ∈ effects, Strict e) :
    undo (execute s effects).1 (execute s effects).2 = s := by
  induction effects generalizing s with
  | nil => rfl
  | cons e rest ih =>
    simp only [execute]
    rw [undo_append, ih _ (fun f hf => strict f (by simp [hf]))]
    exact strict e (by simp) s

/-- Independent inverses commute past the other component's forward action.
This is the precise interference condition for selective component removal. -/
theorem independent_component_removal (f g : Iso α)
    (commute : ∀ x, f.backward (g.forward x) = g.forward (f.backward x)) (s : α) :
    f.backward (g.forward (f.forward s)) = g.forward s := by
  rw [commute, f.backward_forward]

/-- Product-separated state is a concrete sufficient independence criterion. -/
def leftEffect (e : Effect α) : Effect (α × β) := fun s =>
  let result := e s.1
  ((result.1, s.2), fun after => (result.2 after.1, after.2))

def rightEffect (e : Effect β) : Effect (α × β) := fun s =>
  let result := e s.2
  ((s.1, result.1), fun after => (after.1, result.2 after.2))

theorem separated_effects_commute (f : Effect α) (g : Effect β) :
    compose (leftEffect f) (rightEffect g) =
      compose (rightEffect g) (leftEffect f) := rfl

theorem separated_removal_preserves_other (f : Effect α) (g : Effect β)
    (strict : Strict f) (s : α × β) :
    (leftEffect f s).2 ((rightEffect g (leftEffect f s).1).1) =
      (rightEffect g s).1 := by
  simp only [leftEffect, rightEffect]
  rw [strict]

/-- Concrete callbacks distinguish the FIFO pseudocode order in Algorithm 1
from the reverse iteration actually used by Rust `Effect::clean_steps`. -/
def incrementTwo : Effect Nat := fun n => (n + 2, fun m => m - 2)
def doubleNat : Effect Nat := fun n => (2 * n, fun m => m / 2)

theorem increment_two_is_strict : Strict incrementTwo := by
  intro n; simp [incrementTwo]

theorem doubling_is_strict : Strict doubleNat := by
  intro n; simp [doubleNat]

theorem fifo_recovery_counterexample :
    let out := execute 1 [incrementTwo, doubleNat]
    undo out.1 out.2 = 1 ∧ undo out.1 out.2.reverse = 2 := by decide

/-- The semantic interpretation of the existing Rust cleanup-stack model. -/
theorem claimed_cleanup_recovers_callbacks (initial : α) (callbacks : List (Effect α))
    (strict : ∀ e ∈ callbacks, Strict e) :
    let executed := execute initial callbacks
    let owner : Effects.State (α → α) :=
      { running := false, armed := false, pending := executed.2.reverse }
    undo executed.1 (Effects.claim Cordis.cleanup owner).2 = initial := by
  dsimp only
  rw [Effects.claimed_steps_are_lifo _ rfl rfl, List.reverse_reverse]
  exact lifo_execution_recovers initial callbacks strict

end Cordis.RevertibleEffects
