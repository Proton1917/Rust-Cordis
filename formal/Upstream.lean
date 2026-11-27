import Lifecycle

/-!
The shared lifecycle observation contract of the pinned TypeScript core and
Rust. TypeScript's early return and Rust's stable-state idle decision both have
no load/unload observation. The shared domain is a live, unsuspended owner;
unload completion has drained its effects. JavaScript identity comparisons are
supplied as exact equality facts (finite numeric identities stay in the exact
integer range). Promise scheduling, object layout and string serialization are
connected by the pinned-source probe and conformance suite, not assumed proved
as JavaScript language semantics here.
-/

namespace Cordis.Upstream

def observe : Action → Option Action
  | .keep | .idle => none
  | .load => some .load
  | .unload => some .unload

def getState (alive failed active : Bool) : Phase :=
  if !alive then .disposed
  else if failed then .failed
  else if active then .active
  else .pending

def setEpoch (failed inertia same oldInactive active : Bool) : Option Action :=
  if same then none
  else if failed then none
  else if inertia then none
  else if active && oldInactive then some .load
  else some .unload

def rustSetEpoch (failed inertia same oldInactive active empty : Bool) : Option Action :=
  let nextSame := if failed then true else same
  let nextActive := if failed then !oldInactive else active
  observe (refresh inertia false nextSame true empty oldInactive nextActive)

def loadFinish (same : Bool) : Option Action :=
  if same then none else some .unload

def unloadFinish (active : Bool) : Option Action :=
  if active then some .load else none

theorem state_observation_refines : ∀ alive failed active,
    getState alive failed active = idle alive failed active := by decide

/-- Stable idle and an early return have the same transition observation. -/
theorem set_epoch_observation_refines : ∀ failed inertia same oldInactive active empty,
    setEpoch failed inertia same oldInactive active =
      rustSetEpoch failed inertia same oldInactive active empty := by decide

theorem load_completion_refines : ∀ same unloading effects active,
    loadFinish same = observe (finish true same unloading effects active) := by decide

theorem drained_unload_completion_refines : ∀ same active,
    unloadFinish active = observe (finish false same true false active) := by decide

/-- A failure belongs to the generation whose callback actually ran. -/
def fail (target attempted : Epoch) : Epoch :=
  if target = attempted then none else target

theorem failure_refines (s : State) (attempted : Epoch) :
    fail s.target attempted = (step generated s (.fail attempted)).1.target := by
  simp [fail, step]
  split <;> rfl

theorem superseded_failure_keeps_target (target attempted : Epoch) (stale : target ≠ attempted) :
    fail target attempted = target := by simp [fail, stale]

structure RefreshObservation where
  failed : Bool
  inertia : Bool
  same : Bool
  oldInactive : Bool
  active : Bool
  empty : Bool

def tsObservations (events : List RefreshObservation) : List (Option Action) :=
  events.map fun event => setEpoch event.failed event.inertia event.same event.oldInactive event.active

def rustObservations (events : List RefreshObservation) : List (Option Action) :=
  events.map fun event => rustSetEpoch event.failed event.inertia event.same event.oldInactive event.active event.empty

theorem arbitrary_observation_trace_refines (events : List RefreshObservation) :
    tsObservations events = rustObservations events := by
  unfold tsObservations rustObservations
  apply List.map_congr_left
  intro event _
  exact set_epoch_observation_refines _ _ _ _ _ _

end Cordis.Upstream
