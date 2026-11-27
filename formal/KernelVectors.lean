import Bindings

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
  let now : Current := ⟨some (if bit mask 0 then 1 else 0), some (if bit mask 1 then 2 else 1),
    !bit mask 2, if bit mask 3 then some 1 else some 0, if bit mask 4 then some 1 else some 0⟩
  optionCode (installChecked (some 9) "svc" (if bit mask 5 then "other" else "svc")
    old now (bit mask 7))

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
  for mask in List.range 256 do emit "snapshot" [mask] (snapshotCase mask)
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
