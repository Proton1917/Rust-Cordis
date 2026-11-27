import Upstream

/-!
A stateful weak simulation of the shared lifecycle protocol. Epoch replacement,
awaited callback completion, nonempty effect registration/cleanup and committed
provider release are threaded through the same evolving state. Rust status
publication is represented by silent microsteps. Public operations outside this
shared protocol (notably registering new top-level effects while an unload is
already draining) keep their separately stated conformance domains.
-/

namespace Cordis.UpstreamSimulation

structure State where
  target : Epoch := none
  attempted : Epoch := none
  failed : Bool := false
  busy : Bool := false
  loading : Bool := false
  effects : List Nat := []
  cleaning : List Nat := []
  committed : List Nat := []
  deriving DecidableEq

inductive Event where
  | target (epoch : Epoch)
  | loadResult (attempted : Epoch) (failed : Bool)
  | commit (providers : List Nat)
  | collect (effect : Nat)
  | beginCleanup
  | cleaned (effect : Nat)
  | finishUnload
  | recover
  deriving DecidableEq

def launch (s : State) : Action → State
  | .keep | .idle => s
  | .load => {s with busy := true, loading := true, attempted := s.target}
  | .unload => {s with busy := true, loading := false}

def finishAction (s : State) : Action → State
  | .keep | .idle => {s with busy := false}
  | .load => {s with busy := true, loading := true, attempted := s.target}
  | .unload => {s with busy := true, loading := false}

def tsRefresh (s : State) (epoch : Epoch) : State :=
  if epoch = s.target then s
  else if s.failed then s
  else
    let updated := {s with target := epoch}
    if s.busy then updated
    else if epoch.isSome && s.target.isNone then launch updated .load
    else launch updated .unload

def rustRefresh (s : State) (epoch : Epoch) : State :=
  let next := if s.failed then s.target else epoch
  let updated := {s with target := next}
  launch updated (refresh s.busy false (next == s.target) true s.effects.isEmpty
    s.target.isNone next.isSome)

theorem refresh_simulation (s : State) (epoch : Epoch) :
    rustRefresh s epoch = tsRefresh s epoch := by
  rcases s with ⟨target, attempted, failed, busy, loading, effects, cleaning, committed⟩
  cases failed <;> cases busy <;> by_cases same : epoch = target <;>
    simp [rustRefresh, tsRefresh, refresh_equiv, refreshRef, same, launch]
  all_goals cases target <;> cases epoch <;> simp_all [launch]

def reportFailure (s : State) (attempted : Epoch) (failed : Bool) : State :=
  if failed && s.target == attempted then {s with target := none, failed := true} else s

def tsLoadFinish (s : State) : State :=
  if s.target = s.attempted then {s with busy := false}
  else launch s .unload

def rustLoadFinish (s : State) : State :=
  finishAction s (finish true (s.target == s.attempted) false (!s.effects.isEmpty) s.target.isSome)

theorem load_finish_simulation (s : State) : rustLoadFinish s = tsLoadFinish s := by
  simp only [rustLoadFinish, finish_equiv, finishRef]
  by_cases same : s.target = s.attempted <;> simp [tsLoadFinish, same, finishAction, launch]

def tsUnloadFinish (s : State) : State :=
  let released := {s with committed := []}
  if s.target.isSome then launch released .load else {released with busy := false}

def rustUnloadFinish (s : State) : State :=
  finishAction {s with committed := []} (finish false false true false s.target.isSome)

theorem unload_finish_simulation (s : State) : rustUnloadFinish s = tsUnloadFinish s := by
  cases target : s.target <;>
    simp [rustUnloadFinish, tsUnloadFinish, finish_equiv, finishRef, finishAction, launch, target]

def tsStep (s : State) : Event → State
  | .target epoch => tsRefresh s epoch
  | .loadResult attempted failed =>
    if s.busy && s.loading && attempted == s.attempted then tsLoadFinish (reportFailure s attempted failed) else s
  | .commit providers => if s.busy && s.loading then {s with committed := providers} else s
  | .collect effect => if s.busy && s.loading then {s with effects := s.effects ++ [effect]} else s
  | .beginCleanup =>
    if s.busy && !s.loading && s.cleaning.isEmpty then
      {s with cleaning := s.effects.reverse, effects := []} else s
  | .cleaned effect => {s with cleaning := s.cleaning.erase effect}
  | .finishUnload =>
    if s.busy && !s.loading && s.cleaning.isEmpty && s.effects.isEmpty then tsUnloadFinish s else s
  | .recover => {s with failed := false}

def rustStep (s : State) : Event → State
  | .target epoch => rustRefresh s epoch
  | .loadResult attempted failed =>
    if s.busy && s.loading && attempted == s.attempted then rustLoadFinish (reportFailure s attempted failed) else s
  | .commit providers => if s.busy && s.loading then {s with committed := providers} else s
  | .collect effect => if s.busy && s.loading then {s with effects := s.effects ++ [effect]} else s
  | .beginCleanup =>
    if s.busy && !s.loading && s.cleaning.isEmpty then
      {s with cleaning := s.effects.reverse, effects := []} else s
  | .cleaned effect => {s with cleaning := s.cleaning.erase effect}
  | .finishUnload =>
    if s.busy && !s.loading && s.cleaning.isEmpty && s.effects.isEmpty then rustUnloadFinish s else s
  | .recover => {s with failed := false}

theorem state_step_simulation (s : State) (event : Event) : rustStep s event = tsStep s event := by
  cases event <;> simp [rustStep, tsStep, refresh_simulation, load_finish_simulation,
    unload_finish_simulation]

def tsRun : State → List Event → State
  | s, [] => s
  | s, event :: rest => tsRun (tsStep s event) rest

structure RustState where
  core : State
  statusPending : Bool := false
  deriving DecidableEq

inductive MicroEvent where
  | visible (event : Event)
  | statusPublished | executorYield
  deriving DecidableEq

def microStep (s : RustState) : MicroEvent → RustState
  | .visible event => {core := rustStep s.core event, statusPending := true}
  | .statusPublished => {s with statusPending := false}
  | .executorYield => s

def observable : MicroEvent → Option Event
  | .visible event => some event
  | .statusPublished | .executorYield => none

def microRun : RustState → List MicroEvent → RustState
  | s, [] => s
  | s, event :: rest => microRun (microStep s event) rest

theorem silent_steps_preserve_state (s : RustState) :
    (microStep s .statusPublished).core = s.core ∧ (microStep s .executorYield).core = s.core := by
  exact ⟨rfl, rfl⟩

theorem arbitrary_microtrace_simulation (s : RustState) (events : List MicroEvent) :
    (microRun s events).core = tsRun s.core (events.filterMap observable) := by
  induction events generalizing s with
  | nil => rfl
  | cons event rest ih =>
    cases event <;>
      simp only [microRun, ih, microStep, observable, List.filterMap_cons, tsRun,
        state_step_simulation]

theorem trace_concatenation (s : RustState) (left right : List MicroEvent) :
    (microRun s (left ++ right)).core =
      tsRun s.core ((left ++ right).filterMap observable) :=
  arbitrary_microtrace_simulation s (left ++ right)

theorem stale_callback_preserves_new_target (s : State) (attempted : Epoch)
    (stale : s.target ≠ attempted) (failed : Bool) :
    (reportFailure s attempted failed).target = s.target := by
  simp [reportFailure, stale]

theorem nonempty_cleanup_keeps_commit (s : State) (nonempty : s.cleaning ≠ []) :
    (tsStep s .finishUnload).committed = s.committed := by
  simp [tsStep, List.isEmpty_eq_false_iff.mpr nonempty]

theorem cleanup_starts_in_reverse_order (s : State) (busy : s.busy = true)
    (unloading : s.loading = false) (empty : s.cleaning = []) :
    (tsStep s .beginCleanup).cleaning = s.effects.reverse ∧
    (tsStep s .beginCleanup).effects = [] := by simp [tsStep, busy, unloading, empty]

theorem completed_unload_releases_providers (s : State) (busy : s.busy = true)
    (unloading : s.loading = false) (empty : s.cleaning = []) (drained : s.effects = []) :
    (tsStep s .finishUnload).committed = [] := by
  simp [tsStep, busy, unloading, empty, drained, tsUnloadFinish]
  split <;> simp [launch]

end Cordis.UpstreamSimulation
