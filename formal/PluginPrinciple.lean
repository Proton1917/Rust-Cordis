import Lifecycle
import Bindings
import Scopes
import Effects
import Architecture
import WireSemantics
import RevertibleEffects
import ReactiveCoeffects

/-!
The component principle of the Cordis paper, §§3.3–3.4 and 4.2, at the
boundaries implemented by this Rust repository.  A component's implementation
and name are open parameters; declared dependency identities feed the existing
controller.  Finite loader programs compose the same controller operations.

The trace model starts after the registry has assigned instance identities.
Callback completion, dependency snapshots and effect counts are observations
at the existing lock boundaries, not a model of an arbitrary callback's Rust
instructions.  Consequently replacement is conditional on those observations,
and an extension that publishes a used service may deliberately change an old
component's trace.  The source bridge checks the corresponding Rust boundaries.
-/

namespace Cordis.PluginPrinciple

universe u v w

/-- `ComponentDef`'s open implementation parameter and declared injection list.
`Payload` can itself contain a closure, native handle, RPC descriptor or state. -/
structure Definition (Payload : Type u) where
  name : String
  inject : List Bindings.Dependency
  implementation : Payload

/-- The snapshot supplied to the ordered dependency resolver. -/
structure Boundary where
  revision : Nat
  bindings : List (Option Nat)
  table : Bindings.Table

def resolve (definition : Definition Payload) (boundary : Boundary) : Epoch :=
  (Bindings.epochIds boundary.table boundary.bindings definition.inject).map
    fun identities => (boundary.revision, identities.map UInt64.toNat)

/-- Replacing code or a display name leaves dependency resolution unchanged
when the declared injection list and selected snapshot are unchanged. -/
theorem implementation_does_not_choose_epoch
    (left : Definition Payload) (right : Definition OtherPayload)
    (boundary : Boundary) (declared : left.inject = right.inject) :
    resolve left boundary = resolve right boundary := by
  simp only [resolve, declared]

theorem collect_respects_selected_identities (left right : Bindings.Table)
    (pairs : List (Option Nat × Bindings.Dependency))
    (same : ∀ pair ∈ pairs, Bindings.uidAt left pair.1 = Bindings.uidAt right pair.1) :
    Bindings.collect left pairs = Bindings.collect right pairs := by
  induction pairs with
  | nil => rfl
  | cons pair rest ih =>
    have head := same pair (by simp)
    have tail : ∀ item ∈ rest,
        Bindings.uidAt left item.1 = Bindings.uidAt right item.1 :=
      fun item member => same item (by simp [member])
    simp only [Bindings.collect, Bindings.slot, head, ih tail]

/-- Unselected services, owners and payloads do not participate in an epoch.
The premise compares identities only at this declaration's ordered slots. -/
theorem resolution_depends_on_declared_boundary (definition : Definition Payload)
    (revision : Nat) (bindings : List (Option Nat)) (left right : Bindings.Table)
    (same : ∀ pair ∈ bindings.zip definition.inject,
      Bindings.uidAt left pair.1 = Bindings.uidAt right pair.1) :
    resolve definition ⟨revision, bindings, left⟩ =
      resolve definition ⟨revision, bindings, right⟩ := by
  have collected := collect_respects_selected_identities left right
    (bindings.zip definition.inject) same
  simp only [resolve, Bindings.epochIds, collected]

/-- A single observed callback/dependency boundary, routed to its instance. -/
structure Operation where
  owner : Nat
  input : Input
  deriving DecidableEq

abbrev System := Nat → State
abbrev Observation := Nat × Output

def put (system : System) (owner : Nat) (state : State) : System :=
  fun candidate => if candidate = owner then state else system candidate

@[simp] theorem put_same (system : System) (owner : Nat) (state : State) :
    put system owner state owner = state := by simp [put]

@[simp] theorem put_other (system : System) (owner other : Nat) (state : State)
    (distinct : other ≠ owner) : put system owner state other = system other := by
  simp [put, distinct]

/-- The only lifecycle evaluator is the pre-existing generic controller. -/
def step (decisions : Decisions) (system : System) (operation : Operation) :
    System × Observation :=
  let result := Cordis.step decisions (system operation.owner) operation.input
  (put system operation.owner result.1, (operation.owner, result.2))

def execute (decisions : Decisions) : System → List Operation → System × List Observation
  | system, [] => (system, [])
  | system, operation :: rest =>
    let first := step decisions system operation
    let next := execute decisions first.1 rest
    (next.1, first.2 :: next.2)

def inputs (owner : Nat) (operations : List Operation) : List Input :=
  operations.filterMap fun operation =>
    if operation.owner = owner then some operation.input else none

def outputs (owner : Nat) (observations : List Observation) : List Output :=
  observations.filterMap fun observation =>
    if observation.1 = owner then some observation.2 else none

theorem step_frames_other_instances (decisions : Decisions) (system : System)
    (operation : Operation) (other : Nat) (distinct : other ≠ operation.owner) :
    (step decisions system operation).1 other = system other := by
  simp [step, distinct]

theorem execute_append (decisions : Decisions) (system : System)
    (left right : List Operation) :
    execute decisions system (left ++ right) =
      let first := execute decisions system left
      let second := execute decisions first.1 right
      (second.1, first.2 ++ second.2) := by
  induction left generalizing system with
  | nil => simp [execute]
  | cons operation rest ih => simp only [List.cons_append, execute, ih, List.cons_append]

/-- Every instance projection of an arbitrary interleaving is exactly its
existing lifecycle controller, including all outputs and its final state. -/
theorem instance_projection (decisions : Decisions) (system : System)
    (operations : List Operation) (owner : Nat) :
    ((execute decisions system operations).1 owner,
      outputs owner (execute decisions system operations).2) =
      Cordis.run decisions (system owner) (inputs owner operations) := by
  induction operations generalizing system with
  | nil => rfl
  | cons operation rest ih =>
    by_cases selected : operation.owner = owner
    · subst owner
      have tail := ih (step decisions system operation).1
      simpa only [execute, inputs, outputs, List.filterMap_cons, if_pos rfl,
        Option.toList_some, List.cons_append, Cordis.run, step, put_same] using
        congrArg (fun result : State × List Output =>
          (result.1, (Cordis.step decisions (system operation.owner) operation.input).2 ::
            result.2)) tail
    · have tail := ih (step decisions system operation).1
      simpa only [execute, inputs, outputs, List.filterMap_cons, if_neg selected,
        Option.toList_none, List.nil_append, step, put_other _ _ _ _ (Ne.symm selected)]
        using tail

/-- Generated decisions remain equivalent to the reference for any number of
instances and any interleaving.  No plugin category is an evaluator argument. -/
theorem system_controller_refinement (system : System) (operations : List Operation) :
    execute generated system operations = execute reference system operations := by
  rw [decisions_equiv]

/-- This is an observational replacement theorem, not an assumption that two
implementations have the same behavior merely because their types agree. -/
theorem equal_boundary_traces_preserve_instance
    (decisions : Decisions) (left right : System)
    (leftTrace rightTrace : List Operation) (owner : Nat)
    (initial : left owner = right owner)
    (boundary : inputs owner leftTrace = inputs owner rightTrace) :
    ((execute decisions left leftTrace).1 owner,
      outputs owner (execute decisions left leftTrace).2) =
    ((execute decisions right rightTrace).1 owner,
      outputs owner (execute decisions right rightTrace).2) := by
  rw [instance_projection, instance_projection, initial, boundary]

theorem inputs_append (owner : Nat) (left right : List Operation) :
    inputs owner (left ++ right) = inputs owner left ++ inputs owner right := by
  simp only [inputs, List.filterMap_append]

theorem unrelated_inputs_empty (owner : Nat) (operations : List Operation)
    (unrelated : ∀ operation ∈ operations, operation.owner ≠ owner) :
    inputs owner operations = [] := by
  induction operations with
  | nil => rfl
  | cons operation rest ih =>
    have head := unrelated operation (by simp)
    have tail : ∀ other ∈ rest, other.owner ≠ owner :=
      fun other member => unrelated other (by simp [member])
    simpa [inputs, head] using ih tail

/-- Adding an unrelated plugin's complete finite execution preserves both the
old instance state and every old-instance lifecycle observation. -/
theorem extension_preserves_existing_instance
    (decisions : Decisions) (system : System) (old extension : List Operation)
    (owner : Nat) (fresh : ∀ operation ∈ extension, operation.owner ≠ owner) :
    ((execute decisions system (old ++ extension)).1 owner,
      outputs owner (execute decisions system (old ++ extension)).2) =
    ((execute decisions system old).1 owner,
      outputs owner (execute decisions system old).2) := by
  apply equal_boundary_traces_preserve_instance
  · rfl
  · rw [inputs_append, unrelated_inputs_empty owner extension fresh, List.append_nil]

/-- Names and implementation representations are arbitrary.  `observe` is the
finite sequence returned across the generic boundary by a payload; it may be
effectful application logic or an RPC adapter in the concrete implementation.
`inside` interprets a nested loader/group under an already assigned child id. -/
inductive Program (Payload : Type u) where
  | idle
  | perform (payload : Payload)
  | sequence (left right : Program Payload)
  | inside (owner : Nat) (body : Program Payload)

def compile (observe : Payload → List Input) (owner : Nat) :
    Program Payload → List Operation
  | .idle => []
  | .perform payload => (observe payload).map fun input => ⟨owner, input⟩
  | .sequence left right => compile observe owner left ++ compile observe owner right
  | .inside child body => compile observe child body

/-- Independent recursive meaning of source composition. -/
def interpret (decisions : Decisions) (observe : Payload → List Input)
    (owner : Nat) (system : System) : Program Payload → System × List Observation
  | .idle => (system, [])
  | .perform payload =>
    execute decisions system ((observe payload).map fun input => ⟨owner, input⟩)
  | .sequence left right =>
    let first := interpret decisions observe owner system left
    let second := interpret decisions observe owner first.1 right
    (second.1, first.2 ++ second.2)
  | .inside child body => interpret decisions observe child system body

/-- Every finite nested plugin/loader program compiles to the same fixed
controller operations with its complete structural denotation preserved. -/
theorem finite_program_compilation (decisions : Decisions)
    (observe : Payload → List Input) (owner : Nat) (system : System)
    (program : Program Payload) :
    execute decisions system (compile observe owner program) =
      interpret decisions observe owner system program := by
  induction program generalizing owner system with
  | idle => rfl
  | perform payload => rfl
  | sequence left right leftIH rightIH =>
    simp only [compile, execute_append, interpret, leftIH, rightIH]
  | inside child body ih => exact ih child system

def Program.map (change : Payload → OtherPayload) : Program Payload → Program OtherPayload
  | .idle => .idle
  | .perform payload => .perform (change payload)
  | .sequence left right => .sequence (left.map change) (right.map change)
  | .inside child body => .inside child (body.map change)

/-- A representation change can replace every opaque implementation, including
inside nested loaders, provided its actual boundary observations agree. -/
theorem compilation_respects_implementation_replacement
    (before : Payload → List Input) (after : OtherPayload → List Input)
    (replace : Payload → OtherPayload)
    (boundary : ∀ payload, after (replace payload) = before payload)
    (owner : Nat) (program : Program Payload) :
    compile after owner (program.map replace) = compile before owner program := by
  induction program generalizing owner with
  | idle => rfl
  | perform payload => simp only [Program.map, compile, boundary]
  | sequence left right leftIH rightIH => simp only [Program.map, compile, leftIH, rightIH]
  | inside child body ih => exact ih child

theorem replacement_preserves_structural_execution
    (decisions : Decisions) (before : Payload → List Input)
    (after : OtherPayload → List Input) (replace : Payload → OtherPayload)
    (boundary : ∀ payload, after (replace payload) = before payload)
    (owner : Nat) (system : System) (program : Program Payload) :
    interpret decisions after owner system (program.map replace) =
      interpret decisions before owner system program := by
  rw [← finite_program_compilation, ← finite_program_compilation,
    compilation_respects_implementation_replacement before after replace boundary]

/-- The potential instance support is a syntactic upper bound independent of
the number or content of callback observations. -/
def support (owner : Nat) : Program Payload → List Nat
  | .idle => []
  | .perform _ => [owner]
  | .sequence left right => support owner left ++ support owner right
  | .inside child body => support child body

theorem compiled_operations_have_declared_owners
    (observe : Payload → List Input) (owner : Nat) (program : Program Payload)
    (operation : Operation) (member : operation ∈ compile observe owner program) :
    operation.owner ∈ support owner program := by
  induction program generalizing owner with
  | idle => simp [compile] at member
  | perform payload =>
    obtain ⟨input, _, equal⟩ := List.mem_map.mp member
    cases equal
    simp [support]
  | sequence left right leftIH rightIH =>
    rcases List.mem_append.mp member with inLeft | inRight
    · exact List.mem_append_left _ (leftIH owner inLeft)
    · exact List.mem_append_right _ (rightIH owner inRight)
  | inside child body ih => exact ih child member

theorem nested_program_extension_preserves_instance
    (decisions : Decisions) (observe : Payload → List Input) (system : System)
    (scope owner : Nat) (old extension : Program Payload)
    (fresh : owner ∉ support scope extension) :
    ((interpret decisions observe scope system (.sequence old extension)).1 owner,
      outputs owner (interpret decisions observe scope system (.sequence old extension)).2) =
    ((interpret decisions observe scope system old).1 owner,
      outputs owner (interpret decisions observe scope system old).2) := by
  rw [← finite_program_compilation, ← finite_program_compilation]
  apply extension_preserves_existing_instance
  intro operation member equal
  apply fresh
  rw [← equal]
  exact compiled_operations_have_declared_owners observe scope extension operation member

/-- Instance identities select state; their particular numeric values carry no
component semantics.  An injective renaming retains that distinction. -/
def renameOperations (rename : Nat → Nat) (operations : List Operation) : List Operation :=
  operations.map fun operation => ⟨rename operation.owner, operation.input⟩

theorem renaming_preserves_selected_inputs (rename : Nat → Nat)
    (injective : ∀ a b, rename a = rename b → a = b)
    (owner : Nat) (operations : List Operation) :
    inputs (rename owner) (renameOperations rename operations) = inputs owner operations := by
  induction operations with
  | nil => rfl
  | cons operation rest ih =>
    have equal : rename operation.owner = rename owner ↔ operation.owner = owner :=
      ⟨injective _ _, congrArg rename⟩
    simp only [renameOperations, List.map_cons, inputs, List.filterMap_cons, equal] at *
    rw [ih]

theorem instance_identity_equivariance (decisions : Decisions) (rename : Nat → Nat)
    (injective : ∀ a b, rename a = rename b → a = b) (before after : System) (owner : Nat)
    (initial : after (rename owner) = before owner) (operations : List Operation) :
    ((execute decisions after (renameOperations rename operations)).1 (rename owner),
      outputs (rename owner) (execute decisions after (renameOperations rename operations)).2) =
    ((execute decisions before operations).1 owner,
      outputs owner (execute decisions before operations).2) := by
  rw [instance_projection, instance_projection,
    renaming_preserves_selected_inputs rename injective, initial]

/-! ## Recovery on independently owned state

The kernel schedules a disposer once; the disposer supplies its own semantic
inverse.  The following sufficient ownership condition gives selective recovery
its precise meaning: each callback updates its allocated resource cell, and its
inverse updates the same cell.  Shared services can additionally trigger the
dependency transitions already modeled above.
-/

def replaceOwned (state : Nat → Resource) (owner : Nat) (value : Resource) : Nat → Resource :=
  fun candidate => if candidate = owner then value else state candidate

@[simp] theorem replace_owned_same (state : Nat → Resource) (owner : Nat) (value : Resource) :
    replaceOwned state owner value owner = value := by simp [replaceOwned]

@[simp] theorem replace_owned_other (state : Nat → Resource) (owner other : Nat)
    (value : Resource) (different : other ≠ owner) :
    replaceOwned state owner value other = state other := by simp [replaceOwned, different]

/-- A callback and its returned inverse address the same owner's state. -/
def ownedEffect (owner : Nat) (effect : RevertibleEffects.Effect Resource) :
    RevertibleEffects.Effect (Nat → Resource) := fun state =>
  let result := effect (state owner)
  (replaceOwned state owner result.1,
    fun later => replaceOwned later owner (result.2 (later owner)))

theorem owned_effect_is_strict (owner : Nat) (effect : RevertibleEffects.Effect Resource)
    (strict : RevertibleEffects.Strict effect) :
    RevertibleEffects.Strict (ownedEffect owner effect) := by
  intro state
  funext candidate
  by_cases same : candidate = owner
  · subst candidate
    simp only [ownedEffect, replace_owned_same]
    exact strict (state owner)
  · simp [ownedEffect, replaceOwned, same]

theorem owned_callback_and_inverse_frame_others
    (owner other : Nat) (effect : RevertibleEffects.Effect Resource)
    (before after : Nat → Resource) (different : other ≠ owner) :
    (ownedEffect owner effect before).1 other = before other ∧
      (ownedEffect owner effect before).2 after other = after other := by
  simp [ownedEffect, different]

/-- Other owners may change their own resources between loading and unloading.
Recovery restores this owner's pre-load state and retains those surviving data. -/
theorem selective_recovery_preserves_surviving_resources
    (owner : Nat) (effect : RevertibleEffects.Effect Resource)
    (strict : RevertibleEffects.Strict effect) (before after : Nat → Resource)
    (ownState : after owner = (effect (before owner)).1) :
    (ownedEffect owner effect before).2 after = replaceOwned after owner (before owner) := by
  simp only [ownedEffect, ownState]
  rw [strict (before owner)]

/-- Arbitrarily many callbacks at an owner use the same Rust LIFO cleanup
protocol; reversibility is proved from each returned callback inverse. -/
theorem owned_callback_trace_recovers (owner : Nat) (initial : Nat → Resource)
    (callbacks : List (RevertibleEffects.Effect Resource))
    (strict : ∀ effect ∈ callbacks, RevertibleEffects.Strict effect) :
    let effects := callbacks.map (ownedEffect owner)
    let executed := RevertibleEffects.execute initial effects
    let cleanupState : Effects.State ((Nat → Resource) → (Nat → Resource)) :=
      { running := false, armed := false, pending := executed.2.reverse }
    RevertibleEffects.undo executed.1 (Effects.claim Cordis.cleanup cleanupState).2 = initial := by
  apply RevertibleEffects.claimed_cleanup_recovers_callbacks
  intro effect member
  rcases List.mem_map.mp member with ⟨callback, occurs, rfl⟩
  exact owned_effect_is_strict owner callback (strict callback occurs)

/-! ## Components and finite context layers (paper Definitions 20 and 24)

The paper writes its universal context as a recursive schematic containing
functions on that context.  The Rust implementation constructs finite parent
chains.  The following indexed tower realizes every finite unfolding without
postulating a negatively recursive datatype or a universal semantic domain.
-/

/-- The paper's component pair: a finite coeffect specification and one effect.
All application state remains inside the parameter `Context`. -/
structure Component (Key : Type u) (Context : Type v) where
  dependencies : ReactiveCoeffects.Specification Key
  effect : RevertibleEffects.Effect Context

def target {Key : Type u} {Value : Key → Type v}
    (component : Component Key Context) (alive : Bool)
    (environment : ReactiveCoeffects.Store Value) : Bool :=
  alive && ReactiveCoeffects.Satisfied environment component.dependencies

theorem component_target_iff {Key : Type u} {Value : Key → Type v}
    [DecidableEq Key] (component : Component Key Context) (alive : Bool)
    (environment : ReactiveCoeffects.Store Value) :
    target component alive environment = true ↔
      alive = true ∧ ∀ key ∈ component.dependencies, (environment key).isSome = true := by
  simp only [target, Bool.and_eq_true, ReactiveCoeffects.satisfaction_iff]

/-- Synchronous composition with both specifications as initial prerequisites.
A component that provides a later component's dependency is represented by
successive resolver/lifecycle boundaries, rather than by this conservative
combination.  Inertial scheduling remains governed by `Cordis.step` above. -/
def composeComponents (after before : Component Key Context) : Component Key Context :=
  ⟨before.dependencies ++ after.dependencies,
    RevertibleEffects.compose after.effect before.effect⟩

theorem composed_target {Key : Type u} {Value : Key → Type v}
    [DecidableEq Key] (after before : Component Key Context) (alive : Bool)
    (environment : ReactiveCoeffects.Store Value) :
    target (composeComponents after before) alive environment =
      (target before alive environment && target after alive environment) := by
  simp only [target, composeComponents, ReactiveCoeffects.satisfaction_append]
  cases alive <;> simp

theorem composed_component_is_revertible (after before : Component Key Context)
    (afterLaw : RevertibleEffects.Strict after.effect)
    (beforeLaw : RevertibleEffects.Strict before.effect) :
    RevertibleEffects.Strict (composeComponents after before).effect :=
  RevertibleEffects.strict_composition _ _ afterLaw beforeLaw

/-- Level zero contains base state and coeffects.  Every successor level adds
the current lower context, its accumulated inverse, and this level's coeffects. -/
def ContextTower (Base : Type u) (Coeffects : Type v) : Nat → Type (max u v)
  | 0 => Base × Coeffects
  | depth + 1 => RevertibleEffects.Journal (ContextTower Base Coeffects depth) × Coeffects

def enterLayer (state : ContextTower Base Coeffects depth) (environment : Coeffects) :
    ContextTower Base Coeffects (depth + 1) := ((state, id), environment)

def recordLayer (effect : RevertibleEffects.Effect (ContextTower Base Coeffects depth))
    (context : ContextTower Base Coeffects (depth + 1)) :
    ContextTower Base Coeffects (depth + 1) :=
  (RevertibleEffects.record effect context.1, context.2)

def recordLayerRun (context : ContextTower Base Coeffects (depth + 1))
    (effects : List (RevertibleEffects.Effect (ContextTower Base Coeffects depth))) :
    ContextTower Base Coeffects (depth + 1) :=
  (RevertibleEffects.recordRun context.1 effects, context.2)

def recoverLayer (context : ContextTower Base Coeffects (depth + 1)) :
    ContextTower Base Coeffects (depth + 1) :=
  (RevertibleEffects.recoverJournal context.1, context.2)

/-- At every finite depth, composition uses the same effect operation. -/
theorem layer_composition
    (after before : RevertibleEffects.Effect (ContextTower Base Coeffects depth))
    (context : ContextTower Base Coeffects (depth + 1)) :
    recordLayer after (recordLayer before context) =
      recordLayer (RevertibleEffects.compose after before) context := rfl

theorem layer_recovery_idempotent (context : ContextTower Base Coeffects (depth + 1)) :
    recoverLayer (recoverLayer context) = recoverLayer context := rfl

/-- Recovering an arbitrary completed effect prefix restores its original
lower context and keeps this context's coeffect environment. -/
theorem nested_completed_prefix_recovers
    (state : ContextTower Base Coeffects depth) (environment : Coeffects)
    (effects : List (RevertibleEffects.Effect (ContextTower Base Coeffects depth)))
    (strict : ∀ effect ∈ effects, RevertibleEffects.Strict effect) :
    recoverLayer (recordLayerRun (enterLayer state environment) effects) =
      enterLayer state environment := by
  simp only [recoverLayer, recordLayerRun, enterLayer]
  rw [RevertibleEffects.completed_prefix_recovers state effects strict]

theorem layer_recovery_keeps_outer_coeffects
    (context : ContextTower Base Coeffects (depth + 1)) :
    (recoverLayer context).2 = context.2 := rfl

/-- Lift any effect through an arbitrary finite number of context levels. -/
def raiseEffect (Coeffects : Type v) :
    (depth : Nat) → RevertibleEffects.Effect Base →
      RevertibleEffects.Effect (ContextTower Base Coeffects depth)
  | 0, effect => RevertibleEffects.leftEffect effect
  | depth + 1, effect =>
    RevertibleEffects.leftEffect (RevertibleEffects.lift (raiseEffect Coeffects depth effect))

theorem left_effect_is_strict (effect : RevertibleEffects.Effect α)
    (strict : RevertibleEffects.Strict effect) :
    RevertibleEffects.Strict (RevertibleEffects.leftEffect (β := β) effect) := by
  intro state
  simp only [RevertibleEffects.leftEffect]
  rw [strict]

theorem raising_preserves_revertibility (Coeffects : Type v) (depth : Nat)
    (effect : RevertibleEffects.Effect Base) (strict : RevertibleEffects.Strict effect) :
    RevertibleEffects.Strict (raiseEffect Coeffects depth effect) := by
  induction depth with
  | zero => exact left_effect_is_strict effect strict
  | succ depth ih =>
    exact left_effect_is_strict _ (RevertibleEffects.operational_lift_is_strict _ ih)

theorem raising_preserves_composition (Coeffects : Type v) (depth : Nat)
    (after before : RevertibleEffects.Effect Base) :
    raiseEffect Coeffects depth (RevertibleEffects.compose after before) =
      RevertibleEffects.compose (raiseEffect Coeffects depth after)
        (raiseEffect Coeffects depth before) := by
  induction depth with
  | zero => rfl
  | succ depth ih =>
    simp only [raiseEffect, ih, ← RevertibleEffects.operational_lift_composition]
    rfl

theorem right_effect_is_strict (effect : RevertibleEffects.Effect β)
    (strict : RevertibleEffects.Strict effect) :
    RevertibleEffects.Strict (RevertibleEffects.rightEffect (α := α) effect) := by
  intro state
  simp only [RevertibleEffects.rightEffect]
  rw [strict]

/-- Every layer's coeffect store is itself part of that layer's effect state.
The enclosing journal can therefore track registration and removal at exactly
that level, using the same effect interface as every other context update. -/
def provideAtLayer {Key : Type u} {Value : Key → Type v} [DecidableEq Key]
    (Base : Type w) (depth : Nat) (key : Key) (value : Value key) :
    RevertibleEffects.Effect (ContextTower Base (ReactiveCoeffects.Store Value) depth) :=
  match depth with
  | 0 => RevertibleEffects.rightEffect (ReactiveCoeffects.provisionEffect key value)
  | _ + 1 => RevertibleEffects.rightEffect (ReactiveCoeffects.provisionEffect key value)

def layerCoeffects (depth : Nat) : ContextTower Base Coeffects depth → Coeffects :=
  match depth with
  | 0 => Prod.snd
  | _ + 1 => Prod.snd

theorem same_layer_provision_is_strict {Key : Type u} {Value : Key → Type v}
    [DecidableEq Key] (Base : Type w) (depth : Nat) (key : Key) (value : Value key) :
    RevertibleEffects.Strict (provideAtLayer Base depth key value) := by
  cases depth <;>
    exact right_effect_is_strict _ (ReactiveCoeffects.provision_effect_strict key value)

/-- A successful registration changes this level's store, including an
arbitrary typed value, rather than merely preserving a separate environment. -/
theorem same_layer_provision_changes_selected_slot {Key : Type u} {Value : Key → Type v}
    [DecidableEq Key] (Base : Type w) (depth : Nat) (key : Key) (value : Value key)
    (context : ContextTower Base (ReactiveCoeffects.Store Value) depth)
    (vacant : layerCoeffects depth context key = none) :
    layerCoeffects depth (provideAtLayer Base depth key value context).1 key = some value := by
  cases depth <;>
    simp [layerCoeffects, provideAtLayer, RevertibleEffects.rightEffect,
      ReactiveCoeffects.provisionEffect, ReactiveCoeffects.provide] at vacant ⊢ <;>
    rw [vacant] <;> simp

theorem same_layer_registration_recovers {Key : Type u} {Value : Key → Type v}
    [DecidableEq Key] (Base : Type w) (depth : Nat) (key : Key) (value : Value key)
    (context : ContextTower Base (ReactiveCoeffects.Store Value) depth)
    (outer : ReactiveCoeffects.Store Value) :
    recoverLayer (recordLayerRun (enterLayer context outer)
      [provideAtLayer Base depth key value]) = enterLayer context outer := by
  apply nested_completed_prefix_recovers
  intro effect member
  have same : effect = provideAtLayer Base depth key value := by simpa using member
  subst effect
  exact same_layer_provision_is_strict Base depth key value

/-! ## Immutable spatial derivation and owner admission -/

inductive Derivation where
  | inherit
  | capture (epoch : Epoch)
  | child
  deriving DecidableEq

def deriveScope (scope : Scopes.Scope) : Derivation → Scopes.Scope
  | .inherit => Scopes.derive scope
  | .capture epoch => Scopes.bind scope epoch
  | .child => Scopes.child scope

def deriveScopes : Scopes.Scope → List Derivation → Scopes.Scope
  | scope, [] => scope
  | scope, derivation :: rest => deriveScopes (deriveScope scope derivation) rest

theorem derivation_preserves_shape (scope : Scopes.Scope) (path : List Derivation)
    (wellFormed : Scopes.WellFormed scope) :
    Scopes.WellFormed (deriveScopes scope path) := by
  induction path generalizing scope with
  | nil => exact wellFormed
  | cons derivation rest ih =>
    apply ih
    cases derivation with
    | inherit => exact .derive wellFormed
    | capture epoch => exact .bind epoch wellFormed
    | child => exact .child wellFormed

/-- Derived views retain the whole old parent chain, including through a child
fiber boundary.  The boundary changes guard ownership rather than the chain. -/
theorem derivation_retains_parent (scope : Scopes.Scope) (path : List Derivation) :
    ∃ ancestors, deriveScopes scope path = ancestors ++ scope := by
  induction path generalizing scope with
  | nil => exact ⟨[], rfl⟩
  | cons derivation rest ih =>
    obtain ⟨ancestors, same⟩ := ih (deriveScope scope derivation)
    cases derivation with
    | inherit =>
      exact ⟨ancestors ++ [⟨.plain, Scopes.cached scope⟩], by
        simpa [deriveScopes, deriveScope, Scopes.derive, List.append_assoc] using same⟩
    | capture epoch =>
      exact ⟨ancestors ++ [⟨.epoch epoch, true⟩], by
        simpa [deriveScopes, deriveScope, Scopes.bind, List.append_assoc] using same⟩
    | child =>
      exact ⟨ancestors ++ [⟨.fiber, false⟩], by
        simpa [deriveScopes, deriveScope, Scopes.child, List.append_assoc] using same⟩

/-- Arbitrarily nested contexts still register against the owner and captured
epoch conjunction proved by the existing admission calculus. -/
theorem every_derived_context_uses_owner_admission (facts : Scopes.Facts)
    (scope : Scopes.Scope) (path : List Derivation)
    (wellFormed : Scopes.WellFormed scope) (owned : List Nat) (effect : Nat) :
    Scopes.register facts (deriveScopes scope path) owned effect =
      Admission.register (Scopes.admissionState facts (deriveScopes scope path) owned) effect :=
  Scopes.cached_registration_equiv facts _ owned effect
    (derivation_preserves_shape scope path wellFormed)

/-- The registration/initialization boundary receives an allocator-selected
identity.  `createGuard` below supplies that identity constructively. -/
def generateEffect (Cleanup : Type) (facts : Scopes.Facts) (scope : Scopes.Scope)
    (owned : List Nat) (identity : Nat) : Admission.State × Option (Effects.State Cleanup) :=
  let registration := Scopes.register facts scope owned identity
  (registration.1, if registration.2 then some {} else none)

theorem generated_effect_exists_iff_admitted (Cleanup : Type) (facts : Scopes.Facts)
    (scope : Scopes.Scope) (owned : List Nat) (identity : Nat)
    (wellFormed : Scopes.WellFormed scope) :
    (generateEffect Cleanup facts scope owned identity).2.isSome =
      Admission.admitted (Scopes.admissionState facts scope owned) := by
  unfold generateEffect
  rw [Scopes.cached_registration_equiv facts scope owned identity wellFormed]
  cases admitted : Admission.admitted (Scopes.admissionState facts scope owned) <;>
    simp [Admission.register, admitted]

theorem generated_effect_has_initial_guard {Cleanup : Type}
    (facts : Scopes.Facts) (scope : Scopes.Scope)
    (owned : List Nat) (identity : Nat) (state : Effects.State Cleanup)
    (created : (generateEffect Cleanup facts scope owned identity).2 = some state) :
    state = {} := by
  dsimp only [generateEffect] at created
  split at created
  · exact (Option.some.inj created).symm
  · contradiction

/-- An initially clear cleanup guard starts cleanup at most once for all finite
interleavings through handles that share this guard state. -/
theorem generated_effect_disposes_at_most_once {Cleanup : Type} (facts : Scopes.Facts)
    (scope : Scopes.Scope) (owned : List Nat) (identity : Nat)
    (state : Effects.State Cleanup)
    (created : (generateEffect Cleanup facts scope owned identity).2 = some state)
    (commands : List (Effects.Command Cleanup)) :
    (Effects.run Cordis.cleanup state commands).1.starts ≤ 1 := by
  rw [generated_effect_has_initial_guard facts scope owned identity state created]
  exact Effects.cleanup_starts_at_most_once commands

/-- A constructive interpretation of `SlotMap::insert`'s fresh-key contract.
`next` is an abstract allocation serial, not the concrete SlotMap key layout or
the checked AtomicU64 allocator used by Realm/ComponentId.  Cells remain in the
model to retain the history of spent guards while handles can still observe it. -/
structure GuardArena (Cleanup : Type) where
  next : Nat
  cells : Nat → Option (Effects.State Cleanup)

def GuardArena.Valid (arena : GuardArena Cleanup) : Prop :=
  ∀ identity, arena.cells identity ≠ none → identity < arena.next

def emptyGuards (Cleanup : Type) : GuardArena Cleanup := ⟨0, fun _ => none⟩

theorem empty_guards_valid (Cleanup : Type) : (emptyGuards Cleanup).Valid := by
  intro identity present
  exact False.elim (present rfl)

def allocateGuard (arena : GuardArena Cleanup) : GuardArena Cleanup × Nat :=
  (⟨arena.next + 1, replaceOwned arena.cells arena.next (some {})⟩, arena.next)

theorem allocated_guard_identity_was_unused (arena : GuardArena Cleanup)
    (valid : arena.Valid) : arena.cells (allocateGuard arena).2 = none := by
  by_cases vacant : arena.cells arena.next = none
  · exact vacant
  · have impossible := valid arena.next vacant
    omega

theorem allocated_guard_is_initial (arena : GuardArena Cleanup) :
    (allocateGuard arena).1.cells (allocateGuard arena).2 = some {} := by
  simp [allocateGuard]

theorem allocating_guard_preserves_previous_guards (arena : GuardArena Cleanup)
    (identity : Nat) (different : identity ≠ (allocateGuard arena).2) :
    (allocateGuard arena).1.cells identity = arena.cells identity := by
  exact replace_owned_other _ _ _ _ different

theorem allocating_guard_preserves_validity (arena : GuardArena Cleanup)
    (valid : arena.Valid) : (allocateGuard arena).1.Valid := by
  intro identity present
  by_cases same : identity = arena.next
  · simp only [allocateGuard]
    omega
  · have occupied : arena.cells identity ≠ none := by
      simpa [allocateGuard, same] using present
    have earlier := valid identity occupied
    simp only [allocateGuard]
    omega

def allocateGuardRun : GuardArena Cleanup → Nat → GuardArena Cleanup
  | arena, 0 => arena
  | arena, count + 1 => allocateGuardRun (allocateGuard arena).1 count

theorem guard_allocation_trace_preserves_validity (arena : GuardArena Cleanup)
    (count : Nat) (valid : arena.Valid) : (allocateGuardRun arena count).Valid := by
  induction count generalizing arena with
  | zero => exact valid
  | succ count ih => exact ih _ (allocating_guard_preserves_validity arena valid)

theorem every_generated_guard_is_fresh (arena : GuardArena Cleanup) (count : Nat)
    (valid : arena.Valid) :
    (allocateGuardRun arena count).cells (allocateGuard (allocateGuardRun arena count)).2 = none :=
  allocated_guard_identity_was_unused _ (guard_allocation_trace_preserves_validity arena count valid)

theorem consecutive_generated_guards_are_distinct (arena : GuardArena Cleanup) :
    (allocateGuard arena).2 ≠ (allocateGuard (allocateGuard arena).1).2 := by
  simp [allocateGuard]

structure GuardGeneration (Cleanup : Type) where
  arena : GuardArena Cleanup
  ownership : Admission.State
  handle : Option Nat

/-- The wrapper itself chooses a fresh identity only after admission.  Failed
admission changes neither the guard arena nor its allocation serial. -/
def createGuard (arena : GuardArena Cleanup) (facts : Scopes.Facts) (scope : Scopes.Scope)
    (owned : List Nat) : GuardGeneration Cleanup :=
  let registration := generateEffect Cleanup facts scope owned arena.next
  match registration.2 with
  | none => ⟨arena, registration.1, none⟩
  | some _ => ⟨(allocateGuard arena).1, registration.1, some (allocateGuard arena).2⟩

theorem admitted_generation_uses_fresh_guard (arena : GuardArena Cleanup)
    (valid : arena.Valid) (facts : Scopes.Facts) (scope : Scopes.Scope)
    (owned : List Nat) (identity : Nat)
    (created : (createGuard arena facts scope owned).handle = some identity) :
    arena.cells identity = none ∧
      (createGuard arena facts scope owned).arena.cells identity = some {} ∧
      (createGuard arena facts scope owned).arena.Valid := by
  cases registration : (generateEffect Cleanup facts scope owned arena.next).2 with
  | none => simp [createGuard, registration] at created
  | some initial =>
    have same : (allocateGuard arena).2 = identity := by
      simpa [createGuard, registration] using created
    subst identity
    simp only [createGuard, registration]
    exact ⟨allocated_guard_identity_was_unused arena valid,
      allocated_guard_is_initial arena, allocating_guard_preserves_validity arena valid⟩

theorem generation_preserves_existing_guards (arena : GuardArena Cleanup)
    (facts : Scopes.Facts) (scope : Scopes.Scope) (owned : List Nat)
    (identity : Nat) (different : identity ≠ arena.next) :
    (createGuard arena facts scope owned).arena.cells identity = arena.cells identity := by
  cases registration : (generateEffect Cleanup facts scope owned arena.next).2 <;>
    simp [createGuard, registration, allocateGuard, different]

def runGuard (arena : GuardArena Cleanup) (handle : Nat)
    (commands : List (Effects.Command Cleanup)) : GuardArena Cleanup × List Cleanup :=
  match arena.cells handle with
  | none => (arena, [])
  | some state =>
    let result := Effects.run Cordis.cleanup state commands
    (⟨arena.next, replaceOwned arena.cells handle (some result.1)⟩, result.2)

/-- Cloning the Rust handle retains its identity and the same shared guard;
it does not call `allocateGuard` or reset the spent state. -/
def cloneGuardHandle (handle : Nat) : Nat := handle

theorem cloned_handle_routes_to_same_guard (arena : GuardArena Cleanup) (handle : Nat)
    (commands : List (Effects.Command Cleanup)) :
    runGuard arena (cloneGuardHandle handle) commands = runGuard arena handle commands := rfl

theorem guard_commands_preserve_other_guards (arena : GuardArena Cleanup)
    (handle other : Nat) (commands : List (Effects.Command Cleanup)) (different : other ≠ handle) :
    (runGuard arena handle commands).1.cells other = arena.cells other := by
  cases present : arena.cells handle <;> simp [runGuard, present, different]

/-- Allocation and disposal can interleave: running an existing handle keeps
the arena invariant needed by the next fresh allocation. -/
theorem guard_commands_preserve_validity (arena : GuardArena Cleanup)
    (valid : arena.Valid) (handle : Nat) (commands : List (Effects.Command Cleanup)) :
    (runGuard arena handle commands).1.Valid := by
  cases present : arena.cells handle with
  | none => simpa [runGuard, present] using valid
  | some state =>
    intro identity occupied
    simp only [runGuard, present] at occupied ⊢
    by_cases same : identity = handle
    · subst identity
      exact valid handle (by simp [present])
    · apply valid identity
      simpa [runGuard, present, same] using occupied

/-- The generated private handle reaches the same initially clear state used
by the at-most-once theorem, for any finite command trace through its aliases. -/
theorem private_guard_cleanup_starts_at_most_once (arena : GuardArena Cleanup)
    (facts : Scopes.Facts) (scope : Scopes.Scope) (owned : List Nat)
    (handle : Nat) (created : (createGuard arena facts scope owned).handle = some handle)
    (commands : List (Effects.Command Cleanup)) (after : Effects.State Cleanup)
    (result : (runGuard (createGuard arena facts scope owned).arena handle commands).1.cells handle =
      some after) : after.starts ≤ 1 := by
  cases registration : (generateEffect Cleanup facts scope owned arena.next).2 with
  | none => simp [createGuard, registration] at created
  | some initial =>
    have same : (allocateGuard arena).2 = handle := by
      simpa [createGuard, registration] using created
    subst handle
    have initialized := allocated_guard_is_initial arena
    simp only [createGuard, registration] at result
    simp [runGuard, initialized] at result
    rw [← result]
    exact Effects.cleanup_starts_at_most_once commands

/-! ## Fresh identities used by instance and realm extension -/

/-- A finite successful allocation transaction uses the same checked frontier
as `identity::allocate`; `maximum` is the terminal, unallocatable frontier. -/
def allocateMany (maximum frontier : Nat) : Nat → Option (List Nat × Nat)
  | 0 => some ([], frontier)
  | count + 1 => do
    let (identity, next) ← WireSemantics.allocateId maximum frontier
    let (identities, finalFrontier) ← allocateMany maximum next count
    pure (identity :: identities, finalFrontier)

/-- Successful allocations are strictly increasing for an arbitrary trace
length.  Its member bounds also compare separate allocation transactions. -/
theorem allocation_trace_strictly_increases (maximum frontier count : Nat)
    (identities : List Nat) (finalFrontier : Nat)
    (success : allocateMany maximum frontier count = some (identities, finalFrontier)) :
    finalFrontier = frontier + count ∧ identities.length = count ∧
      (∀ identity ∈ identities,
        frontier ≤ identity ∧ identity < finalFrontier ∧ identity < maximum) ∧
      identities.Pairwise (· < ·) ∧ identities.Nodup := by
  induction count generalizing frontier identities finalFrontier with
  | zero =>
    simp only [allocateMany, Option.some.injEq, Prod.mk.injEq] at success
    rcases success with ⟨rfl, rfl⟩
    simp
  | succ count ih =>
    cases first : WireSemantics.allocateId maximum frontier with
    | none => simp [allocateMany, first] at success
    | some allocation =>
      rcases allocation with ⟨identity, next⟩
      cases tail : allocateMany maximum next count with
      | none => simp [allocateMany, first, tail] at success
      | some result =>
        rcases result with ⟨rest, final⟩
        simp [allocateMany, first, tail] at success
        rcases success with ⟨rfl, rfl⟩
        have allocated := WireSemantics.allocated_id_advances_frontier _ _ _ _ first
        obtain ⟨position, length, bounds, sorted, unique⟩ := ih next rest final tail
        refine ⟨by omega, by simp [length], ?_, ?_, ?_⟩
        · intro value member
          rcases List.mem_cons.mp member with equal | member
          · subst value; exact ⟨by omega, by omega, allocated.2.2⟩
          · have bound := bounds value member
            exact ⟨by omega, bound.2.1, bound.2.2⟩
        · apply List.pairwise_cons.mpr
          refine ⟨?_, sorted⟩
          intro value member
          have bound := bounds value member
          omega
        · apply List.nodup_cons.mpr
          refine ⟨?_, unique⟩
          intro member
          have bound := bounds identity member
          omega

theorem successful_identity_trace_never_reuses (maximum frontier count : Nat)
    (identities : List Nat) (finalFrontier : Nat)
    (success : allocateMany maximum frontier count = some (identities, finalFrontier)) :
    identities.Nodup :=
  (allocation_trace_strictly_increases _ _ _ _ _ success).2.2.2.2

theorem positive_identity_frontier_reserves_default (maximum frontier count : Nat)
    (identities : List Nat) (finalFrontier : Nat) (positive : 0 < frontier)
    (success : allocateMany maximum frontier count = some (identities, finalFrontier)) :
    0 ∉ identities := by
  intro member
  have bound := (allocation_trace_strictly_increases _ _ _ _ _ success).2.2.1 0 member
  omega

theorem separate_identity_traces_never_reuse (maximum start middle final : Nat)
    (firstCount secondCount : Nat) (first second : List Nat)
    (one : allocateMany maximum start firstCount = some (first, middle))
    (two : allocateMany maximum middle secondCount = some (second, final))
    (old fresh : Nat) (oldMember : old ∈ first) (freshMember : fresh ∈ second) :
    old < fresh := by
  have left := (allocation_trace_strictly_increases _ _ _ _ _ one).2.2.1 old oldMember
  have right := (allocation_trace_strictly_increases _ _ _ _ _ two).2.2.1 fresh freshMember
  omega

theorem exhausted_identity_frontier_stays_closed (maximum frontier remaining : Nat)
    (exhausted : maximum ≤ frontier) :
    allocateMany maximum frontier (remaining + 1) = none := by
  have rejected : WireSemantics.allocateId maximum frontier = none := by
    simp [WireSemantics.allocateId, Nat.not_lt.mpr exhausted]
  simp [allocateMany, rejected]

end Cordis.PluginPrinciple
