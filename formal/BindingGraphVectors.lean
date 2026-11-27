import BindingGraph

namespace Cordis.BindingGraphVectors
open BindingGraph

def bit (mask index : Nat) : Bool := mask / 2^index % 2 == 1

def snapshot (mask : Nat) : Snapshot := Id.run do
  let incoming := bit mask 1
  let actualKey := if bit mask 2 then "other@1" else "service@1"
  let actualMinor := if bit mask 3 then 1 else 3
  let inherited := if bit mask 7 then 1 else 2
  let replacement : Provider := ⟨0, actualKey, actualMinor, inherited, 9⟩
  let existing : Provider := if incoming then ⟨0, "service@1", 3, inherited, 9⟩ else replacement
  let sibling : List Provider := if bit mask 14 then [⟨2, "service@1", 3, 1, 9⟩] else []
  let kind := mask / 2^10 % 4
  let binding : Option (Option Nat) :=
    if bit mask 8 || bit mask 9 || kind == 0 then none
    else if kind == 1 then some none
    else some (some (if kind == 2 then 0 else 2))
  let consumer : Consumer := ⟨1, "service@1", 2, 1, binding⟩
  return {
    parents := if bit mask 0 then [(2, 1)] else []
    consumers := if !(bit mask 4) && bit mask 5 then [] else [consumer]
    current := existing :: sibling
    incoming := if incoming then [replacement] else []
    replaced := if incoming then [0] else []
    named := fun realm => if realm == 1 then some (if bit mask 6 then 9 else 8) else none }

def roots (mask : Nat) : List Nat :=
  (if bit mask 12 then [0] else []) ++ (if bit mask 13 then [2] else [])

def observe (mask : Nat) : String :=
  let selected := (Deployment.select (adjacency (pairs (snapshot mask))) (roots mask)).selected
  let ordered := (List.range 3).filter (fun node => node ∈ selected)
  String.intercalate "," (ordered.map toString)

end Cordis.BindingGraphVectors

def main : IO Unit := do
  for mask in List.range 32768 do
    IO.println s!"{mask}\t{Cordis.BindingGraphVectors.observe mask}"
