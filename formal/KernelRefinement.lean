import Scopes

/-!
Representation and lock-boundary refinement for `FiberData`, `Scope`, and
`Context::create_effect`. The executable structures retain fixed-width words,
optional task/error fields, owner identities, and concrete effect lists. The
refinement is a Lean theorem about this source-shaped adapter; the source bridge
and its Rust-level trust boundary are documented in notes/kernel-refinement.md.
-/

namespace Cordis.KernelRefinement

inductive Epoch64 where
  | inactive
  | active (revision : UInt64) (impls : List UInt64)
  deriving DecidableEq

def eraseEpoch : Epoch64 → Epoch
  | .inactive => none
  | .active revision impls => some (revision.toNat, impls.map UInt64.toNat)

theorem word_list_projection_injective (xs ys : List UInt64) :
    xs.map UInt64.toNat = ys.map UInt64.toNat ↔ xs = ys := by
  induction xs generalizing ys with
  | nil => cases ys <;> simp
  | cons x xs ih =>
    cases ys with
    | nil => simp
    | cons y ys => simp [UInt64.toNat_inj, ih]

theorem epoch_projection_injective (a b : Epoch64) :
    eraseEpoch a = eraseEpoch b ↔ a = b := by
  cases a <;> cases b <;>
    simp [eraseEpoch, UInt64.toNat_inj, word_list_projection_injective]

theorem epoch_equality_refines (a b : Epoch64) :
    (a == b) = (eraseEpoch a == eraseEpoch b) := by
  apply Bool.eq_iff_iff.mpr
  simp [epoch_projection_injective]

theorem inactive_refines (e : Epoch64) :
    (e == .inactive) = (eraseEpoch e).isNone := by cases e <;> rfl

theorem active_refines (e : Epoch64) :
    (e != .inactive) = (eraseEpoch e).isSome := by cases e <;> rfl

theorem revision_increment_no_wrap (revision : UInt64)
    (room : revision.toNat + 1 < UInt64.size) :
    (revision + 1).toNat = revision.toNat + 1 := by
  simp only [UInt64.toNat_add, UInt64.toNat_ofNat, Nat.reduceMod,
    Nat.mod_eq_of_lt room]

theorem revision_increment_changes_identity (revision : UInt64)
    (room : revision.toNat + 1 < UInt64.size) : revision + 1 ≠ revision := by
  intro same
  have projected := congrArg UInt64.toNat same
  rw [revision_increment_no_wrap revision room] at projected
  omega

theorem notification_increment_no_wrap (count : USize)
    (room : count.toNat + 1 < USize.size) :
    (count + 1).toNat = count.toNat + 1 := by
  rw [USize.toNat_add]
  have one : (1 : USize).toNat = 1 := by simp
  rw [one]
  exact Nat.mod_eq_of_lt room

theorem notification_decrement_no_underflow (count : USize)
    (positive : 0 < count.toNat) :
    (count - 1).toNat = count.toNat - 1 := by
  have bound : (1 : USize) ≤ count := by
    rw [USize.le_iff_toNat_le]
    simpa using positive
  simpa using USize.toNat_sub_of_le count 1 bound

structure FiberData where
  state : Phase
  alive : Bool
  error : Option Nat
  target : Epoch64
  runningEpoch : Epoch64
  inertia : Bool
  suspended : Option Nat
  queued : Bool
  notifications : USize
  effects : List Nat

def eraseFiber (data : FiberData) : Cordis.State :=
  { phase := data.state, alive := data.alive, failed := data.error.isSome,
    target := eraseEpoch data.target, running := eraseEpoch data.runningEpoch,
    inertia := data.inertia, suspended := data.suspended.isSome,
    queued := data.queued, notifications := data.notifications.toNat,
    effects := data.effects.length }

def idleState (data : FiberData) : Phase :=
  idle data.alive data.error.isSome (data.target != .inactive)

def applyRefreshAction (data : FiberData) : Action → FiberData
  | .keep => data
  | .idle => {data with state := idleState data}
  | .load => {data with inertia := true, state := .loading, runningEpoch := data.target}
  | .unload => {data with inertia := true, state := .unloading}

def publishBoundary (old : Phase) (data : FiberData) (action : Action) :
    FiberData × Output :=
  let changed := old != data.state
  ({data with notifications := if changed then data.notifications + 1 else data.notifications},
   {action, status := if changed then some (old, data.state) else none,
    commit := if action == .load then some (eraseEpoch data.target) else none})

def eraseBoundary (boundary : FiberData × Output) : Cordis.State × Output :=
  (eraseFiber boundary.1, boundary.2)

theorem refresh_action_refines (data : FiberData) (action : Action) :
    eraseFiber (applyRefreshAction data action) = applyAction generated (eraseFiber data) action := by
  cases action <;> simp [applyRefreshAction, eraseFiber, applyAction, generated,
    idleState, active_refines]

theorem publish_refines (old : Phase) (data : FiberData) (action : Action)
    (room : data.notifications.toNat + 1 < USize.size) :
    eraseBoundary (publishBoundary old data action) = publish old (eraseFiber data) action := by
  cases changed : old != data.state <;>
    simp [publishBoundary, eraseBoundary, publish, eraseFiber, changed,
      notification_increment_no_wrap data.notifications room]

def refreshBoundary (data : FiberData) (resolved : Epoch64) : FiberData × Output :=
  let next := match target data.alive data.error.isSome with
    | .keep => data.target
    | .resolved => resolved
    | .inactive => .inactive
  let oldTarget := data.target
  let updated := {data with target := next, queued := false}
  let action := refresh updated.inertia updated.suspended.isSome (updated.target == oldTarget)
    updated.alive updated.effects.isEmpty (oldTarget == .inactive) (updated.target != .inactive)
  publishBoundary data.state (applyRefreshAction updated action) action

theorem effect_empty_refines (effects : List Nat) :
    effects.isEmpty = (effects.length == 0) := by cases effects <;> rfl

theorem effect_nonempty_refines (effects : List Nat) :
    (!effects.isEmpty) = (effects.length != 0) := by cases effects <;> rfl

theorem refresh_action_keeps_count (data : FiberData) (action : Action) :
    (applyRefreshAction data action).notifications = data.notifications := by cases action <;> rfl

/-- The actual field updates commute with erasure, for every target and effect
    list length, under the precise finite-counter capacity obligation. -/
theorem refresh_boundary_simulation (data : FiberData) (resolved : Epoch64)
    (room : data.notifications.toNat + 1 < USize.size) :
    eraseBoundary (refreshBoundary data resolved) =
      step generated (eraseFiber data) (.refresh (eraseEpoch resolved)) := by
  unfold refreshBoundary
  rw [publish_refines _ _ _ (by simpa only [refresh_action_keeps_count] using room)]
  rw [refresh_action_refines]
  unfold step
  simp only [generated, eraseFiber]
  cases target data.alive data.error.isSome <;> cases ht : data.target <;> cases resolved <;>
    simp [eraseFiber, epoch_equality_refines, inactive_refines, active_refines,
      effect_empty_refines, eraseEpoch, ht]

/-- In `finish_transition`, an existing transition owns inertia throughout the
    loop; unlike refresh, the Rust code does not set it again on continuation. -/
def applyFinishAction (data : FiberData) : Action → FiberData
  | .load => {data with state := .loading, runningEpoch := data.target}
  | .unload => {data with state := .unloading}
  | .keep | .idle => {data with state := idleState data}

theorem finish_never_keeps (loading same unloading effects active : Bool) :
    finish loading same unloading effects active ≠ .keep := by
  cases loading <;> cases same <;> cases unloading <;> cases effects <;> cases active <;> decide

theorem finish_action_refines (data : FiberData) (action : Action)
    (inFlight : data.inertia = true) (notKeep : action ≠ .keep) :
    eraseFiber (applyFinishAction data action) = applyAction generated (eraseFiber data) action := by
  cases action <;> simp_all [applyFinishAction, eraseFiber, applyAction, generated,
    idleState, active_refines]

def finishBoundary (data : FiberData) (loading : Bool) : FiberData × Output :=
  let action := finish loading (data.target == data.runningEpoch) (data.state == .unloading)
    (!data.effects.isEmpty) (data.target != .inactive)
  let next := applyFinishAction data action
  let next := if action == .idle && data.state == next.state then {next with inertia := false} else next
  publishBoundary data.state next action

theorem finish_boundary_simulation (data : FiberData) (loading : Bool)
    (inFlight : data.inertia = true)
    (room : data.notifications.toNat + 1 < USize.size) :
    eraseBoundary (finishBoundary data loading) =
      step generated (eraseFiber data) (.finish loading) := by
  let action := finish loading (data.target == data.runningEpoch) (data.state == .unloading)
    (!data.effects.isEmpty) (data.target != .inactive)
  have notKeep : action ≠ .keep := finish_never_keeps loading
    (data.target == data.runningEpoch) (data.state == .unloading)
    (!data.effects.isEmpty) (data.target != .inactive)
  have actionEq := finish_action_refines data action inFlight notKeep
  have count : (applyFinishAction data action).notifications = data.notifications := by
    cases action <;> rfl
  unfold finishBoundary
  change eraseBoundary (publishBoundary data.state
    (if action == .idle && data.state == (applyFinishAction data action).state then
      {applyFinishAction data action with inertia := false} else applyFinishAction data action) action) = _
  rw [publish_refines _ _ _ (by split <;> simpa only [count] using room)]
  unfold step
  simp only [generated, eraseFiber, ← epoch_equality_refines, ← active_refines,
    ← effect_nonempty_refines]
  change publish data.state (eraseFiber
    (if action == .idle && data.state == (applyFinishAction data action).state then
      {applyFinishAction data action with inertia := false} else applyFinishAction data action)) action =
    publish data.state
      (if action == .idle && data.state == (applyAction generated (eraseFiber data) action).phase then
        {applyAction generated (eraseFiber data) action with inertia := false}
      else applyAction generated (eraseFiber data) action) action
  rw [← actionEq]
  split <;> rename_i h <;> simp [eraseFiber, h]

/-! A persistent scope has concrete owner identities. Fiber boundaries retain
    their parent pointers. Freshness is local: a newly allocated child identity
    differs from its parent's live identity. -/

inductive View where
  | root (owner : Nat)
  | plain (parent : View)
  | bound (parent : View) (epoch : Epoch64)
  | child (owner : Nat) (parent : View)

def owner : View → Nat
  | .root id | .child id _ => id
  | .plain parent | .bound parent _ => owner parent

def cached : View → Bool
  | .root _ | .child _ _ => false
  | .plain parent => cached parent
  | .bound _ _ => true

def parent : View → Option View
  | .root _ => none
  | .plain scope | .bound scope _ | .child _ scope => some scope

def abstractView : View → Scopes.Scope
  | .root _ => Scopes.root
  | .plain scope => Scopes.derive (abstractView scope)
  | .bound scope epoch => Scopes.bind (abstractView scope) (eraseEpoch epoch)
  | .child _ scope => Scopes.child (abstractView scope)

inductive Constructed : View → Prop where
  | root id : Constructed (.root id)
  | plain {scope} : Constructed scope → Constructed (.plain scope)
  | bound {scope} epoch : Constructed scope → Constructed (.bound scope epoch)
  | child {scope} id : Constructed scope → id ≠ owner scope → Constructed (.child id scope)

def scan (fiber : Nat) : View → List Epoch64
  | .root id => if id == fiber then [] else []
  | .plain scope => if owner scope == fiber then scan fiber scope else []
  | .bound scope epoch => if owner scope == fiber then epoch :: scan fiber scope else []
  | .child id scope => if id == fiber then scan fiber scope else []

theorem foreign_owner_stops (view : View) (fiber : Nat) (foreign : owner view ≠ fiber) :
    scan fiber view = [] := by cases view <;> simp_all [scan, owner]

theorem view_projection_wellformed (view : View) : Scopes.WellFormed (abstractView view) := by
  induction view with
  | root id => exact .root
  | plain scope ih => exact .derive ih
  | bound scope epoch ih => exact .bind _ ih
  | child id scope ih => exact .child ih

theorem owner_boundary_matches_layer_boundary (view : View) (built : Constructed view) :
    (scan (owner view) view).map eraseEpoch = Scopes.guards (abstractView view) := by
  induction built with
  | root id => simp [scan, owner, abstractView, Scopes.root, Scopes.guards]
  | plain built ih => simpa [scan, owner, abstractView, Scopes.derive, Scopes.guards] using ih
  | bound epoch built ih => simp [scan, owner, abstractView, Scopes.bind, Scopes.guards, ih]
  | child id built fresh ih =>
    simp [scan, owner, foreign_owner_stops _ _ (Ne.symm fresh), abstractView,
      Scopes.child, Scopes.guards]

theorem scope_cache_refines (view : View) : cached view = Scopes.cached (abstractView view) := by
  induction view with
  | root _ | child _ _ _ => rfl
  | plain _ ih => exact ih
  | bound _ _ _ => rfl

def permits (data : FiberData) (view : View) : Bool :=
  if !data.alive then false else if !cached view then true
  else if !current data.alive (data.target == data.runningEpoch) then false
  else (scan (owner view) view).all (fun epoch => data.target == epoch)

def facts (data : FiberData) : Scopes.Facts :=
  ⟨data.alive, eraseEpoch data.target, eraseEpoch data.runningEpoch⟩

theorem scope_admission_refines (data : FiberData) (view : View) (built : Constructed view) :
    permits data view = Scopes.fast (facts data) (abstractView view) := by
  have scanEq := owner_boundary_matches_layer_boundary view built
  unfold permits Scopes.fast facts
  rw [← scope_cache_refines, ← scanEq]
  simp only [List.all_map, Function.comp_def, ← epoch_equality_refines]
  cases data.alive <;> cases cached view <;>
    cases current true (data.target == data.runningEpoch) <;> simp [current_equiv]

structure LockedState where
  fiber : FiberData
  owner : Nat
  records : List (Nat × Nat)

def abstractAdmission (state : LockedState) (view : View) : Admission.State :=
  Scopes.admissionState (facts state.fiber) (abstractView view) state.fiber.effects

/-- Both mutations occur after admission within the same lock-held operation.
    A scheduler may interleave operations only at these returned boundaries. -/
def registerLocked (state : LockedState) (view : View) (id : Nat) : LockedState × Bool :=
  if permits state.fiber view then
    ({state with
        records := state.records ++ [(id, state.owner)]
        fiber := {state.fiber with effects := state.fiber.effects ++ [id]}}, true)
  else (state, false)

theorem locked_registration_simulation (state : LockedState) (view : View) (id : Nat)
    (built : Constructed view) :
    (abstractAdmission (registerLocked state view id).1 view, (registerLocked state view id).2) =
      Admission.register (abstractAdmission state view) id := by
  have admission := Scopes.cached_registration_equiv (facts state.fiber)
    (abstractView view) state.fiber.effects id (view_projection_wellformed view)
  unfold abstractAdmission
  rw [← admission]
  simp only [registerLocked, scope_admission_refines _ _ built, Scopes.register]
  split <;> rfl

def Owned (state : LockedState) : Prop :=
  ∀ id ∈ state.fiber.effects, (id, state.owner) ∈ state.records

theorem locked_registration_preserves_ownership (state : LockedState) (view : View) (id : Nat)
    (owned : Owned state) : Owned (registerLocked state view id).1 := by
  unfold registerLocked
  split
  · intro effect member
    simp only [List.mem_append, List.mem_singleton] at member
    rcases member with old | rfl
    · exact List.mem_append_left _ (owned effect old)
    · simp
  · exact owned

theorem rejected_registration_preserves_both_tables (state : LockedState) (view : View) (id : Nat)
    (rejected : permits state.fiber view = false) :
    registerLocked state view id = (state, false) := by simp [registerLocked, rejected]

theorem successful_registration_uses_same_snapshot (state : LockedState) (view : View) (id : Nat)
    (success : (registerLocked state view id).2 = true) :
    permits state.fiber view = true ∧
      (registerLocked state view id).1.fiber.effects = state.fiber.effects ++ [id] ∧
      (id, state.owner) ∈ (registerLocked state view id).1.records := by
  unfold registerLocked at *
  split at success <;> simp_all

theorem successful_registration_records_view_owner (state : LockedState) (view : View) (id : Nat)
    (sameOwner : state.owner = owner view)
    (success : (registerLocked state view id).2 = true) :
    (id, owner view) ∈ (registerLocked state view id).1.records := by
  rw [← sameOwner]
  exact (successful_registration_uses_same_snapshot state view id success).2.2

theorem fresh_registration_preserves_unique_effect_ids (state : LockedState) (view : View) (id : Nat)
    (unique : state.fiber.effects.Nodup) (fresh : id ∉ state.fiber.effects) :
    (registerLocked state view id).1.fiber.effects.Nodup := by
  unfold registerLocked
  split
  · apply List.pairwise_append.mpr
    refine ⟨unique, by simp, ?_⟩
    intro effect member other singleton
    have same : other = id := by simpa using singleton
    subst other
    intro equal
    exact fresh (equal ▸ member)
  · exact unique

end Cordis.KernelRefinement
