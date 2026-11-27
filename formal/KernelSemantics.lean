import Bindings

/-!
Execution-control and service-resolution obligations surrounding the field
adapter. The controller accounts for a submitted or currently running transition
and a recovered suspended task. It derives the inertia precondition at every
reachable finish entry. Service results preserve stored value/definition identity,
including declared absence and typed handle construction. Numeric exhaustion is
an explicit result, so the fresh-identity proof does not assume spare capacity.
-/

namespace Cordis.KernelSemantics

structure Control where
  inertia : Bool := false
  runner : Bool := false
  suspended : Bool := false
  deriving DecidableEq

inductive ControlEvent where
  | start | park | resume | callback
  | finish (statusChanged continuation : Bool)
  deriving DecidableEq

def controlStep (s : Control) : ControlEvent → Control
  | .start => if s.runner || s.suspended then s else ⟨true, true, false⟩
  | .park => if s.runner then ⟨false, false, true⟩ else s
  | .resume => if s.suspended then ⟨true, true, false⟩ else s
  | .finish changed next =>
    if s.runner && !changed && !next then ⟨false, false, false⟩ else s
  | .callback => s

def ControlInvariant (s : Control) : Prop :=
  s.runner = s.inertia ∧ (s.suspended = true → s.runner = false)

def controlRun : Control → List ControlEvent → Control
  | s, [] => s
  | s, e :: es => controlRun (controlStep s e) es

theorem initial_control : ControlInvariant {} := by simp [ControlInvariant]

theorem control_step_preserves (s : Control) (e : ControlEvent)
    (valid : ControlInvariant s) : ControlInvariant (controlStep s e) := by
  rcases s with ⟨inertia, runner, suspended⟩
  cases inertia <;> cases runner <;> cases suspended <;> cases e <;>
    simp_all [ControlInvariant, controlStep]
  all_goals rename_i changed next
  all_goals cases changed <;> cases next <;> decide

theorem control_trace_preserves (s : Control) (events : List ControlEvent)
    (valid : ControlInvariant s) : ControlInvariant (controlRun s events) := by
  induction events generalizing s with
  | nil => exact valid
  | cons e es ih => exact ih _ (control_step_preserves s e valid)

theorem reachable_finish_has_inertia (events : List ControlEvent)
    (executing : (controlRun {} events).runner = true) :
    (controlRun {} events).inertia = true := by
  have h := (control_trace_preserves {} events initial_control).1
  exact h.symm.trans executing

theorem suspended_task_has_no_runner (events : List ControlEvent)
    (parked : (controlRun {} events).suspended = true) :
    (controlRun {} events).runner = false :=
  (control_trace_preserves {} events initial_control).2 parked

theorem callback_keeps_transition_custody (s : Control) :
    controlStep s .callback = s := rfl

theorem finish_boundary_from_execution (events : List ControlEvent)
    (data : KernelRefinement.FiberData) (loading : Bool)
    (representation : data.inertia = (controlRun {} events).inertia)
    (executing : (controlRun {} events).runner = true)
    (room : data.notifications.toNat + 1 < USize.size) :
    KernelRefinement.eraseBoundary (KernelRefinement.finishBoundary data loading) =
      Cordis.step generated (KernelRefinement.eraseFiber data) (.finish loading) :=
  KernelRefinement.finish_boundary_simulation data loading
    (representation.trans (reachable_finish_has_inertia events executing)) room

def checkedNext (limit current : Nat) : Option Nat :=
  if current < limit then some (current + 1) else none

theorem successful_identity_is_fresh (limit current next : Nat)
    (allocated : checkedNext limit current = some next) :
    next = current + 1 ∧ current < next ∧ next ≤ limit := by
  simp only [checkedNext] at allocated
  split at allocated
  · have eq := Option.some.inj allocated
    omega
  · contradiction

theorem exhaustion_has_no_identity (limit current : Nat) (full : limit ≤ current) :
    checkedNext limit current = none := by simp [checkedNext, Nat.not_lt.mpr full]

theorem allocated_identity_is_nonzero (limit current next : Nat)
    (allocated : checkedNext limit current = some next) : next ≠ 0 := by
  have fresh := (successful_identity_is_fresh limit current next allocated).2.1
  omega

def reserveIdentity {α : Type} (limit current : Nat) (before : α)
    (install : α → Nat → α) : α × Option Nat :=
  match checkedNext limit current with
  | none => (before, none)
  | some next => (install before next, some next)

theorem exhausted_allocation_preserves_registry {α : Type} (limit current : Nat)
    (before : α) (install : α → Nat → α) (full : limit ≤ current) :
    reserveIdentity limit current before install = (before, none) := by
  simp [reserveIdentity, exhaustion_has_no_identity limit current full]

def allocateRun (limit : Nat) : Nat → Nat → Nat × List Nat
  | current, 0 => (current, [])
  | current, fuel + 1 =>
    match checkedNext limit current with
    | none => (current, [])
    | some next =>
      let rest := allocateRun limit next fuel
      (rest.1, next :: rest.2)

theorem allocation_members_above_start (limit current fuel id : Nat)
    (member : id ∈ (allocateRun limit current fuel).2) : current < id := by
  induction fuel generalizing current with
  | zero => simp [allocateRun] at member
  | succ fuel ih =>
    simp only [allocateRun] at member
    cases next : checkedNext limit current with
    | none => simp [next] at member
    | some value =>
      simp only [next, List.mem_cons] at member
      rcases member with rfl | tail
      · exact (successful_identity_is_fresh _ _ _ next).2.1
      · have later := ih _ tail
        have first := (successful_identity_is_fresh _ _ _ next).2.1
        omega

theorem allocations_never_reuse (limit current fuel : Nat) :
    (allocateRun limit current fuel).2.Nodup := by
  induction fuel generalizing current with
  | zero => simp [allocateRun]
  | succ fuel ih =>
    simp only [allocateRun]
    cases next : checkedNext limit current with
    | none => simp
    | some value =>
      simp only [List.nodup_cons]
      refine ⟨?_, ih value⟩
      intro member
      have impossible := allocation_members_above_start limit value fuel value member
      omega

structure Notification where
  revision : Nat
  exhausted : Bool := false
  deriving DecidableEq

def advanceNotification (limit : Nat) (s : Notification) : Notification :=
  match checkedNext limit s.revision with
  | some next => {s with revision := next}
  | none => {s with exhausted := true}

def candidate (id : Nat) (s : Notification) : Option Nat :=
  if s.exhausted then none else some id

theorem exhausted_candidate_absent (id : Nat) (s : Notification) (h : s.exhausted = true) :
    candidate id s = none := by simp [candidate, h]

theorem exhaustion_is_permanent (limit : Nat) (s : Notification) (h : s.exhausted = true) :
    (advanceNotification limit s).exhausted = true := by
  unfold advanceNotification
  cases checkedNext limit s.revision <;> simp [h]

theorem exhausted_snapshot_rejected (previous : Option Nat) (key : String)
    (old : Bindings.Snapshot) (now : Bindings.Current) (id : Nat)
    (captured : old.binding = some id) (invalidated : now.binding = none) (available : Bool) :
    Bindings.installChecked previous key key old now available = previous := by
  simp [Bindings.installChecked, Bindings.validSnapshot, captured, invalidated]

inductive AccessError where
  | unauthorized | optionalAbsent | requiredInactive | wrongType
  deriving DecidableEq

structure Value where
  identity : Nat
  typeId : Nat
  definition : Nat
  deriving DecidableEq

def resolve (located : Option Bindings.Located) (optional : Nat → Bool)
    (store : Nat → Value) : Except AccessError Value :=
  match located with
  | none => .error .unauthorized
  | some (.declaredAbsent owner) =>
    if optional owner then .error .optionalAbsent else .error .requiredInactive
  | some (.provision id) | some (.committed _ id) => .ok (store id)

def getValue (isRoot : Bool) (live : Option Nat) (frames : List Bindings.Frame)
    (key : String) (realm : Nat) (optional : Nat → Bool) (store : Nat → Value) :
    Except AccessError Value :=
  resolve (if isRoot then live.map Bindings.Located.provision else Bindings.locate key realm frames)
    optional store

def typed (expected : Nat) : Except AccessError Value → Except AccessError Value
  | .error error => .error error
  | .ok value => if value.typeId = expected then .ok value else .error .wrongType

theorem root_reads_live (live : Option Nat) (frames : List Bindings.Frame) (key : String)
    (realm : Nat) (optional : Nat → Bool) (store : Nat → Value) :
    getValue true live frames key realm optional store =
      match live with | none => .error .unauthorized | some id => .ok (store id) := by
  cases live <;> rfl

theorem committed_value_retains_definition (owner id : Nat) (optional : Nat → Bool)
    (store : Nat → Value) :
    resolve (some (.committed owner id)) optional store = .ok (store id) := rfl

theorem optional_absence_has_distinct_error (owner : Nat) (optional : Nat → Bool)
    (store : Nat → Value) (h : optional owner = true) :
    resolve (some (.declaredAbsent owner)) optional store = .error .optionalAbsent := by
  simp [resolve, h]

theorem required_absence_has_distinct_error (owner : Nat) (optional : Nat → Bool)
    (store : Nat → Value) (h : optional owner = false) :
    resolve (some (.declaredAbsent owner)) optional store = .error .requiredInactive := by
  simp [resolve, h]

theorem typed_success_preserves_identity (expected : Nat) (value result : Value)
    (success : typed expected (.ok value) = .ok result) :
    result = value ∧ value.typeId = expected := by
  simp only [typed] at success
  split at success
  · exact ⟨(Except.ok.inj success).symm, by assumption⟩
  · contradiction

theorem typed_failure_propagates (expected : Nat) (error : AccessError) :
    typed expected (.error error) = .error error := rfl

structure Handle where
  value : Value
  caller : Nat
  requester : Nat
  deriving DecidableEq

def fromBinding (caller expected : Nat) (value : Value) : Option Handle :=
  if value.typeId = expected then some ⟨value, caller, caller⟩ else none

def definitionCalls (expected : Nat) (value : Value) : Nat :=
  if value.typeId = expected then 1 else 0

theorem typed_handle_preserves_origins (caller expected : Nat) (value : Value)
    (result : Handle) (success : fromBinding caller expected value = some result) :
    result.value = value ∧ result.caller = caller ∧ result.requester = caller ∧
      value.typeId = expected := by
  unfold fromBinding at success
  split at success
  · cases Option.some.inj success
    exact ⟨rfl, rfl, rfl, by assumption⟩
  · contradiction

theorem wrong_type_does_not_evaluate_definition (caller expected : Nat) (value : Value)
    (wrong : value.typeId ≠ expected) :
    fromBinding caller expected value = none ∧ definitionCalls expected value = 0 := by
  simp [fromBinding, definitionCalls, wrong]

def declarationMetadata {α : Type} (default : α) (located : Option Bindings.Located)
    (metadata : Nat → Option α) : α :=
  match located with
  | some (.committed owner _) | some (.declaredAbsent owner) => (metadata owner).getD default
  | _ => default

def collectMetadata {α : Type} (key : String) : List (String × Option α) → Option (List α)
  | [] => some []
  | (name, value) :: rest =>
    if name = key then do
      let value ← value
      let tail ← collectMetadata key rest
      pure (value :: tail)
    else collectMetadata key rest

def mergeMetadata {α : Type} (merge : α → α → α) (initial : α)
    (insideOut : List α) : α := insideOut.reverse.foldl merge initial

def checkedMetadata {α : Type} (key : String) (merge : α → α → α) (initial : α)
    (insideOut : List (String × Option α)) : Option α :=
  (collectMetadata key insideOut).map (mergeMetadata merge initial)

theorem own_provision_has_default_metadata {α : Type} (default : α)
    (metadata : Nat → Option α) (id : Nat) :
    declarationMetadata default (some (.provision id)) metadata = default := rfl

theorem committed_metadata_uses_requester_declaration {α : Type} (default : α)
    (metadata : Nat → Option α) (owner id : Nat) :
    declarationMetadata default (some (.committed owner id)) metadata =
      (metadata owner).getD default := rfl

theorem metadata_inner_layer_applies_last {α : Type} (merge : α → α → α)
    (initial inner : α) (outside : List α) :
    mergeMetadata merge initial (inner :: outside) =
      merge (mergeMetadata merge initial outside) inner := by
  simp [mergeMetadata, List.foldl_append]

theorem metadata_unrelated_key_is_ignored {α : Type} (key other : String)
    (value : Option α) (rest : List (String × Option α)) (different : other ≠ key) :
    collectMetadata key ((other, value) :: rest) = collectMetadata key rest := by
  simp [collectMetadata, different]

theorem malformed_matching_metadata_fails {α : Type} (key : String)
    (merge : α → α → α) (initial : α) (rest : List (String × Option α)) :
    checkedMetadata key merge initial ((key, none) :: rest) = none := by
  simp [checkedMetadata, collectMetadata]

theorem empty_interception_preserves_declaration {α : Type} (key : String)
    (merge : α → α → α) (initial : α) :
    checkedMetadata key merge initial [] = some initial := rfl

end Cordis.KernelSemantics
