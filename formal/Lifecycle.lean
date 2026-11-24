import Std

/-!
Executable decision trees and a reference semantics for the lifecycle controller.

The reference functions retain the branch order of `fiber.rs` at
7e4dd5e41445be82af10def53fae315b5b83f7ea. The generated Rust module is evaluated by
the actual controller. Epochs, effect counts, notification counts and traces have
no finite bound in this model. External callback execution is an input boundary.
-/

namespace Cordis

inductive Phase where
  | pending | loading | active | failed | disposed | unloading
  deriving DecidableEq, Repr

inductive Action where
  | keep | idle | load | unload
  deriving DecidableEq, Repr

inductive Target where
  | keep | resolved | inactive
  deriving DecidableEq, Repr

/-- A first-order decision program; generation only prints branches and leaves. -/
inductive Tree (α : Type) (n : Nat) where
  | leaf (value : α)
  | branch (field : Fin n) (yes no : Tree α n)

def Tree.eval (input : Fin n → Bool) : Tree α n → α
  | .leaf value => value
  | .branch field yes no => if input field then yes.eval input else no.eval input

/-- Predicate reads are part of the cost contract for unbounded epoch vectors. -/
def Tree.reads (input : Fin n → Bool) : Tree α n → List Nat
  | .leaf _ => []
  | .branch field yes no => field.val :: if input field then yes.reads input else no.reads input

def idleRef (alive failed active : Bool) : Phase :=
  if !alive then .disposed else if failed then .failed else if active then .active else .pending

def idleTree : Tree Phase 3 :=
  .branch 0 (.branch 1 (.leaf .failed) (.branch 2 (.leaf .active) (.leaf .pending)))
    (.leaf .disposed)

def idle (alive failed active : Bool) : Phase :=
  idleTree.eval (fun i => [alive, failed, active].getD i.val false)

theorem idle_equiv : ∀ alive failed active, idle alive failed active = idleRef alive failed active := by
  decide

def targetRef (alive failed : Bool) : Target :=
  if failed && alive then .keep else if alive then .resolved else .inactive

def targetTree : Tree Target 2 :=
  .branch 0 (.branch 1 (.leaf .keep) (.leaf .resolved)) (.leaf .inactive)

def target (alive failed : Bool) : Target := targetTree.eval (fun i => [alive, failed].getD i.val false)

theorem target_equiv : ∀ alive failed, target alive failed = targetRef alive failed := by decide

def refreshRef (inertia suspended same alive empty oldInactive active : Bool) : Action :=
  if inertia || suspended then .keep
  else if same && (alive || empty) then .idle
  else if oldInactive && active then .load
  else .unload

def startTree : Tree Action 7 :=
  .branch 5 (.branch 6 (.leaf .load) (.leaf .unload)) (.leaf .unload)

def refreshTree : Tree Action 7 :=
  .branch 0 (.leaf .keep)
    (.branch 1 (.leaf .keep)
      (.branch 2
        (.branch 3 (.leaf .idle) (.branch 4 (.leaf .idle) startTree))
        startTree))

def refresh (inertia suspended same alive empty oldInactive active : Bool) : Action :=
  refreshTree.eval (fun i => [inertia, suspended, same, alive, empty, oldInactive, active].getD i.val false)

theorem refresh_equiv : ∀ inertia suspended same alive empty oldInactive active,
    refresh inertia suspended same alive empty oldInactive active =
      refreshRef inertia suspended same alive empty oldInactive active := by decide

def finishRef (loading same unloading effects active : Bool) : Action :=
  if loading then (if !same then .unload else .idle)
  else if unloading && effects then .unload
  else if active then .load else .idle

def finishTree : Tree Action 5 :=
  .branch 0 (.branch 1 (.leaf .idle) (.leaf .unload))
    (.branch 2
      (.branch 3 (.leaf .unload) (.branch 4 (.leaf .load) (.leaf .idle)))
      (.branch 4 (.leaf .load) (.leaf .idle)))

def finish (loading same unloading effects active : Bool) : Action :=
  finishTree.eval (fun i => [loading, same, unloading, effects, active].getD i.val false)

theorem finish_equiv : ∀ loading same unloading effects active,
    finish loading same unloading effects active = finishRef loading same unloading effects active := by
  decide

def settledRef (inertia queued notifying suspended : Bool) : Bool :=
  !inertia && !queued && !notifying && !suspended

def settledTree : Tree Bool 4 :=
  .branch 0 (.leaf false) (.branch 1 (.leaf false)
    (.branch 2 (.leaf false) (.branch 3 (.leaf false) (.leaf true))))

def settled (inertia queued notifying suspended : Bool) : Bool :=
  settledTree.eval (fun i => [inertia, queued, notifying, suspended].getD i.val false)

theorem settled_equiv : ∀ inertia queued notifying suspended,
    settled inertia queued notifying suspended = settledRef inertia queued notifying suspended := by decide

def currentTree : Tree Bool 2 := .branch 0 (.branch 1 (.leaf true) (.leaf false)) (.leaf false)
def current (alive same : Bool) : Bool := currentTree.eval (fun i => [alive, same].getD i.val false)
theorem current_equiv : ∀ alive same, current alive same = (alive && same) := by decide

def currentEpochTree : Tree Bool 3 :=
  .branch 0 (.branch 1 (.branch 2 (.leaf true) (.leaf false)) (.leaf false)) (.leaf false)
def currentEpoch (alive same expected : Bool) : Bool :=
  currentEpochTree.eval (fun i => [alive, same, expected].getD i.val false)
theorem current_epoch_equiv : ∀ alive same expected,
    currentEpoch alive same expected = (alive && same && expected) := by decide

def cleanupTree : Tree Bool 2 := .branch 0 (.leaf false) (.branch 1 (.leaf false) (.leaf true))
def cleanup (running cleaning : Bool) : Bool := cleanupTree.eval (fun i => [running, cleaning].getD i.val false)
theorem cleanup_equiv : ∀ running cleaning, cleanup running cleaning = (!running && !cleaning) := by decide

/-- `none` is the inactive epoch; an active epoch retains revision and ordered identities. -/
abbrev Epoch := Option (Nat × List Nat)

structure State where
  phase : Phase
  alive : Bool
  failed : Bool
  target : Epoch
  running : Epoch
  inertia : Bool
  suspended : Bool
  queued : Bool
  notifications : Nat
  effects : Nat
  deriving DecidableEq, Repr

/-- Outputs at a lock-protected controller boundary, before callbacks execute. -/
structure Output where
  action : Action := .keep
  status : Option (Phase × Phase) := none
  commit : Option Epoch := none
  deriving DecidableEq, Repr

inductive Input where
  | refresh (resolved : Epoch)
  | finish (loading : Bool)
  | fail (attempt : Epoch)
  | dispose
  | recover
  | suspend
  | resume
  | notified
  | effects (count : Nat)
  deriving DecidableEq, Repr

structure Decisions where
  idle : Bool → Bool → Bool → Phase
  target : Bool → Bool → Target
  refresh : Bool → Bool → Bool → Bool → Bool → Bool → Bool → Action
  finish : Bool → Bool → Bool → Bool → Bool → Action

def reference : Decisions := ⟨idleRef, targetRef, refreshRef, finishRef⟩
def generated : Decisions := ⟨idle, target, refresh, finish⟩

theorem decisions_equiv : generated = reference := by
  have hi : idle = idleRef := funext fun a => funext fun b => funext fun c => idle_equiv a b c
  have ht : target = targetRef := funext fun a => funext fun b => target_equiv a b
  have hr : refresh = refreshRef := funext fun a => funext fun b => funext fun c =>
    funext fun d => funext fun e => funext fun f => funext fun g => refresh_equiv a b c d e f g
  have hf : finish = finishRef := funext fun a => funext fun b => funext fun c =>
    funext fun d => funext fun e => finish_equiv a b c d e
  simp only [generated, reference, hi, ht, hr, hf]

def publish (old : Phase) (state : State) (action : Action := .keep) : State × Output :=
  let changed := old != state.phase
  ({state with notifications := state.notifications + if changed then 1 else 0},
   {action, status := if changed then some (old, state.phase) else none,
    commit := if action == .load then some state.target else none})

def applyAction (d : Decisions) (s : State) (action : Action) : State :=
  match action with
  | .keep => s
  | .idle => {s with phase := d.idle s.alive s.failed s.target.isSome}
  | .load => {s with phase := .loading, running := s.target, inertia := true}
  | .unload => {s with phase := .unloading, inertia := true}

/-- Lock-boundary transitions mirror drain/finish and failure/disposal dispatch paths.
    `effects` and `notified` expose external completion facts instead of inventing scheduler behavior. -/
def step (d : Decisions) (s : State) (input : Input) : State × Output :=
  match input with
  | .refresh resolved =>
    let next := match d.target s.alive s.failed with
      | .keep => s.target
      | .resolved => resolved
      | .inactive => none
    let action := d.refresh s.inertia s.suspended (next == s.target) s.alive
      (s.effects == 0) s.target.isNone next.isSome
    publish s.phase (applyAction d {s with target := next, queued := false} action) action
  | .finish loading =>
    let action := d.finish loading (s.target == s.running) (s.phase == .unloading)
      (s.effects != 0) s.target.isSome
    let next := applyAction d s action
    let next := if action == .idle && s.phase == next.phase then {next with inertia := false} else next
    publish s.phase next action
  | .fail attempt =>
    (if s.target == attempt then {s with failed := true, target := none} else s, {})
  | .dispose => ({s with alive := false, queued := true}, {})
  | .recover => ({s with failed := false, queued := true}, {})
  | .suspend => publish s.phase {s with phase := .failed, suspended := true, inertia := false}
  | .resume => if s.suspended then
      publish s.phase {s with phase := .unloading, suspended := false, inertia := true}
    else (s, {})
  | .notified => ({s with notifications := s.notifications - 1}, {})
  | .effects count => ({s with effects := count}, {})

theorem step_equiv (s : State) (input : Input) :
    step generated s input = step reference s input := by rw [decisions_equiv]

def run (d : Decisions) : State → List Input → State × List Output
  | s, [] => (s, [])
  | s, input :: rest =>
    let (next, output) := step d s input
    let (last, outputs) := run d next rest
    (last, output :: outputs)

/-- All six phases, arbitrary epoch identity vectors, arbitrary interleavings and trace lengths. -/
theorem trace_equiv (s : State) (inputs : List Input) :
    run generated s inputs = run reference s inputs := by
  induction inputs generalizing s with
  | nil => rfl
  | cons input rest ih => simp only [run, step_equiv, ih]

/-- Infinite executions agree at every finite observation prefix. No exploration bound is used. -/
theorem stream_prefix_equiv (s : State) (inputs : Nat → Input) (length : Nat) :
    run generated s ((List.range length).map inputs) =
      run reference s ((List.range length).map inputs) := trace_equiv s _

theorem failed_refresh_preserves_target (s : State) (resolved : Epoch)
    (ha : s.alive = true) (he : s.failed = true) :
    (step generated s (.refresh resolved)).1.target = s.target := by
  simp [step, generated, target, targetTree, Tree.eval, ha, he, publish, applyAction]
  split <;> rfl

theorem stale_failure_preserves_state (s : State) (attempt : Epoch) (h : s.target ≠ attempt) :
    step generated s (.fail attempt) = (s, {}) := by simp [step, h]

theorem inertia_preserves_transition (s : State) (resolved : Epoch) (h : s.inertia = true) :
    (step generated s (.refresh resolved)).1.phase = s.phase := by
  simp [step, generated, refresh, refreshTree, Tree.eval, h, publish, applyAction]

theorem suspended_preserves_transition (s : State) (resolved : Epoch) (h : s.suspended = true) :
    (step generated s (.refresh resolved)).1.phase = s.phase := by
  cases hi : s.inertia <;>
    simp [step, generated, refresh, refreshTree, Tree.eval, h, hi, publish, applyAction]

theorem active_epoch_change_unloads (same alive empty : Bool) :
    refresh false false same alive empty false true ≠ .load := by
  cases same <;> cases alive <;> cases empty <;> decide

theorem inertial_refresh_reads_no_epoch (suspended same alive empty oldInactive active : Bool) :
    refreshTree.reads (fun i => [true, suspended, same, alive, empty, oldInactive, active].getD i.val false) = [0] := rfl

theorem suspended_refresh_reads_no_epoch (same alive empty oldInactive active : Bool) :
    refreshTree.reads (fun i => [false, true, same, alive, empty, oldInactive, active].getD i.val false) = [0, 1] := rfl

theorem unloading_finish_reads_no_epoch : ∀ same unloading effects active : Bool,
    1 ∉ finishTree.reads (fun i => [false, same, unloading, effects, active].getD i.val false) := by decide

theorem disposed_current_reads_no_epoch (same : Bool) :
    currentTree.reads (fun i => [false, same].getD i.val false) = [0] := rfl

theorem disposed_generation_reads_no_epoch (same expected : Bool) :
    currentEpochTree.reads (fun i => [false, same, expected].getD i.val false) = [0] := rfl

theorem outdated_generation_skips_expected (expected : Bool) :
    currentEpochTree.reads (fun i => [true, false, expected].getD i.val false) = [0, 1] := rfl

end Cordis
