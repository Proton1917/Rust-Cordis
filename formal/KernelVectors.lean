import KernelSemantics

/-! Executable observations for the independently compiled production fragments.
    These finite vectors supplement the universal theorems; their size is not a
    proof bound. -/

open Cordis Cordis.KernelRefinement Cordis.Bindings

namespace Cordis.KernelVectors

def bit (mask index : Nat) : Bool := mask / 2 ^ index % 2 == 1
def number (value : Bool) : Nat := if value then 1 else 0
def csv (values : List Nat) : String := String.intercalate "," (values.map toString)

def phase : Nat → Phase
  | 0 => .pending | 1 => .loading | 2 => .active | 3 => .failed | 4 => .disposed | _ => .unloading

def phaseCode : Phase → Nat
  | .pending => 0 | .loading => 1 | .active => 2 | .failed => 3 | .disposed => 4 | .unloading => 5

def epoch : Nat → Epoch64
  | 0 => .inactive
  | 1 => .active 0 []
  | 2 => .active 1 [7, 0]
  | _ => .active 18446744073709551615 [18446744073709551615, 1]

def epochCode (value : Epoch64) : Nat :=
  if value == epoch 0 then 0 else if value == epoch 1 then 1 else if value == epoch 2 then 2 else 3

def actionCode : Action → Nat
  | .keep => 0 | .idle => 1 | .load => 2 | .unload => 3

def fiber (state mask target running : Nat) : FiberData :=
  {state := phase state, alive := bit mask 0, error := if bit mask 1 then some 0 else none,
   target := epoch target, runningEpoch := epoch running, inertia := bit mask 2,
   suspended := if bit mask 3 then some 0 else none, queued := bit mask 4,
   notifications := 7, effects := if bit mask 5 then [0, 1, 2] else []}

def boundaryCode (result : FiberData × Output) : String :=
  let data := result.1
  csv [phaseCode data.state, epochCode data.target, epochCode data.runningEpoch,
    number data.alive, number data.error.isSome, number data.inertia,
    number data.suspended.isSome, number data.queued, data.notifications.toNat,
    data.effects.length, actionCode result.2.action, number result.2.status.isSome,
    number result.2.commit.isSome]

def view (kind depth captured : Nat) : View :=
  let base := View.root 0
  let initial := match kind with
    | 0 => base
    | 1 => .bound base (epoch captured)
    | 2 => .bound (.bound base (epoch ((captured + 1) % 4))) (epoch captured)
    | 3 => .child 1 (.bound base (epoch captured))
    | 4 => .bound (.child 1 (.bound base (epoch ((captured + 1) % 4)))) (epoch captured)
    | _ => .bound (.bound base (epoch captured)) (epoch captured)
  (List.range depth).foldl (fun parent _ => .plain parent) initial

def registrationCode (state : LockedState) (scope : View) : String :=
  let result := registerLocked state scope state.records.length
  csv [number result.2, result.1.fiber.effects.length, result.1.records.length]

def dependencies (length mask : Nat) : List Dependency :=
  (List.range length).map fun index => ⟨"k" ++ toString index, bit mask index⟩

def bindingList (length base code : Nat) : List (Option Nat) :=
  (List.range length).map fun index => match code / base ^ index % base with
    | 0 => none | 1 => some 0 | 2 => some 1 | _ => some 9

def table : Table
  | 0 => some ⟨1, 0⟩
  | 1 => some ⟨18446744073709551615, 1⟩
  | _ => none

def uidsCode : Option (List UInt64) → String
  | none => "none"
  | some values => "some:" ++ String.intercalate "," (values.map fun value => toString value.toNat)

def pairsCode (pairs : List (String × Nat)) : String :=
  String.intercalate ";" (pairs.map fun (key, id) => key ++ ":" ++ toString id)

def locationCode : Option Located → String
  | none => "none"
  | some (.provision id) => "p:" ++ toString id
  | some (.committed owner id) => "c:" ++ toString owner ++ ":" ++ toString id
  | some (.declaredAbsent owner) => "a:" ++ toString owner

def frames (mask : Nat) : List Frame :=
  let realm := if bit mask 0 then 2 else 0
  let own := (if bit mask 2 then [Provision.mk 0 "svc" realm] else []) ++
    (if bit mask 3 then [Provision.mk 1 "svc" realm] else []) ++
    (if bit mask 9 then [Provision.mk 3 "svc" (realm + 1)] else [])
  [⟨1, own,
      (if bit mask 0 then ["svc"] else []) ++ (if bit mask 1 then ["other"] else []),
      if bit mask 4 then [("svc", 4)] else [], if bit mask 5 then ["svc"] else []⟩,
   ⟨0, if bit mask 7 then [⟨2, "svc", realm⟩] else [], [],
      if bit mask 6 then [("svc", 5)] else [], if bit mask 8 then ["svc"] else []⟩]

def optionCode : Option Nat → String
  | none => "none" | some value => toString value

def snapshotCase (mask : Nat) : String :=
  let old : Snapshot := ⟨some 0, some 1, true, some 0, if bit mask 6 then none else some 0⟩
  let now : Current := ⟨if bit mask 8 then none else some (if bit mask 0 then 1 else 0),
    if bit mask 8 then none else some (if bit mask 1 then 2 else 1),
    !(bit mask 2 || bit mask 8), if bit mask 3 then some 1 else some 0,
    if bit mask 8 then none else if bit mask 4 then some 1 else some 0⟩
  optionCode (installChecked (some 9) "svc" (if bit mask 5 then "other" else "svc")
    old now (bit mask 7))

def getValueCase (mask : Nat) : String :=
  let store := fun id =>
    {identity := 0, typeId := 0, definition := if id == 2 || id == 5 then 0 else 1 : KernelSemantics.Value}
  let optional := fun owner => owner == 1 && !bit mask 11
  let value := KernelSemantics.getValue (bit mask 10) (if bit mask 2 then some 0 else none)
    (frames mask) "svc" (if bit mask 0 then 2 else 0) optional store
  match value with
  | .ok value => "ok:" ++ csv [value.identity, value.definition]
  | .error .optionalAbsent => "optional"
  | .error .requiredInactive => "required"
  | .error _ => "unauthorized"

def typedHandleCase (actual expected mask : Nat) : String :=
  let caller := if bit mask 0 then 1 else 0
  let definition := if bit mask 1 then 1 else 0
  let value : KernelSemantics.Value := ⟨19, actual, definition⟩
  match KernelSemantics.fromBinding caller expected value with
  | none => "wrong:0"
  | some result => "ok:" ++ csv [result.value.identity, result.caller,
      result.value.definition, result.requester, 1, KernelSemantics.definitionCalls expected value]

def boundaryCounter : Nat → Nat
  | 0 => 0 | 1 => 1 | 2 => 18446744073709551613
  | 3 => 18446744073709551614 | 4 => 18446744073709551615 | _ => 9007199254740992

def identityCase (index existing : Nat) : String :=
  let current := boundaryCounter index
  match KernelSemantics.checkedNext 18446744073709551615 current with
  | none => csv [0, current, 1 + existing]
  | some next => csv [1, next, 2]

def notificationCase (index poisoned : Nat) : String :=
  let after := KernelSemantics.advanceNotification 18446744073709551615
    ⟨boundaryCounter index, poisoned == 1⟩
  csv [after.revision, number after.exhausted]

def metadataCase (mask intercept : Nat) : String :=
  let metadata := fun owner =>
    if bit intercept 1 then none
    else if owner == 1 && bit mask 5 || owner == 0 && bit mask 8 then some [100, owner]
    else none
  let base := KernelSemantics.declarationMetadata []
    (locate "svc" (if bit mask 0 then 2 else 0) (frames mask)) metadata
  let layers : List (String × Option (List Nat)) :=
    (if bit intercept 3 then [("svc", none)] else []) ++
    (if bit intercept 2 then [("svc", some [20])] else []) ++
    (if bit intercept 1 then [("other", none)] else []) ++
    (if bit intercept 0 then [("svc", some [10])] else [])
  match KernelSemantics.checkedMetadata "svc" List.append base layers with
  | none => "panic"
  | some values => "ok:" ++ csv values

def releaseCase (length code beforeMask provisionMask : Nat) : String :=
  let committed := commitPairs (dependencies length 0) (bindingList length 3 code)
  let before := fun binding consumer => bit beforeMask (2 * binding + consumer - 1)
  let after := released before 1 committed
  let provisions := [0, 1].filter (fun binding => bit provisionMask binding)
  let readyBefore := provisions.all (fun binding => !before binding 1 && !before binding 2)
  let readyAfter := provisions.all (fun binding => !after binding 1 && !after binding 2)
  csv [number (after 0 1), number (after 0 2), number (after 1 1), number (after 1 2),
    number readyBefore, number readyAfter]

def emit (family : String) (input : List Nat) (result : String) : IO Unit :=
  IO.println (family ++ "\t" ++ csv input ++ "\t" ++ result)

def vectors : IO Unit := do
  for state in List.range 6 do
    for mask in List.range 64 do
      for target in List.range 4 do
        for running in List.range 4 do
          for resolved in List.range 4 do
            emit "refresh" [state, mask, target, running, resolved]
              (boundaryCode (refreshBoundary (fiber state mask target running) (epoch resolved)))
          for loading in [false, true] do
            let data := {fiber state mask target running with inertia := true}
            emit "finish" [state, mask, target, running, number loading]
              (boundaryCode (finishBoundary data loading))
  for mask in List.range 4 do
    for target in List.range 4 do
      for running in List.range 4 do
        for kind in List.range 6 do
          for depth in [0, 1, 2, 17, 129] do
            for captured in List.range 4 do
              let scope := view kind depth captured
              let effects := if bit mask 1 then [0, 1, 2] else []
              let data := {fiber 2 mask target running with effects := effects}
              let state : LockedState := ⟨data, owner scope, effects.map fun id => (id, owner scope)⟩
              emit "register" [mask, target, running, kind, depth, captured] (registrationCode state scope)
  for length in List.range 5 do
    for code in List.range (4 ^ length) do
      for required in List.range (2 ^ length) do
        emit "epoch" [length, code, required, length]
          (uidsCode (epochIds table (bindingList length 4 code) (dependencies length required)))
      emit "epoch" [length, code, 0, length + 1]
        (uidsCode (epochIds table (bindingList length 4 code) (dependencies (length + 1) 0)))
      if length > 0 then
        emit "epoch" [length, code, 0, length - 1]
          (uidsCode (epochIds table (bindingList length 4 code) (dependencies (length - 1) 0)))
    for code in List.range (3 ^ length) do
      for ownerMask in List.range 4 do
        let committed := commitPairs (dependencies length 0) (bindingList length 3 code)
        let owns := fun id => if bit ownerMask id then 1 else 0
        emit "commit" [length, code, ownerMask]
          (pairsCode committed ++ "|" ++ csv [number (retained committed 0 0 owns),
            number (retained committed 0 1 owns)])
      for beforeMask in List.range 16 do
        for provisionMask in List.range 4 do
          emit "release" [length, code, beforeMask, provisionMask]
            (releaseCase length code beforeMask provisionMask)
  for mask in List.range 1024 do
    emit "locate" [mask] (locationCode (locate "svc" (if bit mask 0 then 2 else 0) (frames mask)))
  for mask in List.range 512 do emit "snapshot" [mask] (snapshotCase mask)
  for mask in List.range 4096 do emit "get_value" [mask] (getValueCase mask)
  for actual in List.range 4 do
    for expected in List.range 3 do
      for mask in List.range 4 do
        emit "typed_handle" [actual, expected, mask] (typedHandleCase actual expected mask)
  for mask in List.range 1024 do
    for intercept in List.range 16 do
      emit "metadata" [mask, intercept] (metadataCase mask intercept)
  for index in List.range 6 do
    for flag in List.range 2 do
      emit "identity" [index, flag] (identityCase index flag)
      emit "notification" [index, flag] (notificationCase index flag)
  for state in List.range 6 do
    for mask in [1, 5, 35] do
      for target in List.range 4 do
        for resolved in List.range 4 do
          for nearLimit in [false, true] do
            let count := if nearLimit then USize.ofNat (USize.size - 2) else 0
            let data := {fiber state mask target target with notifications := count}
            emit "counter" [state, mask, target, target, resolved, number nearLimit]
              (boundaryCode (refreshBoundary data (epoch resolved)))

end Cordis.KernelVectors

def main : IO Unit := Cordis.KernelVectors.vectors
