import UpstreamSimulation

open Cordis Cordis.Upstream

def bit (mask field : Nat) : Bool := mask / 2 ^ field % 2 == 1

def phaseName : Phase → String
  | .pending => "Pending" | .loading => "Loading" | .active => "Active"
  | .failed => "Failed" | .disposed => "Disposed" | .unloading => "Unloading"

def observedName : Option Action → String
  | some .load => "Load"
  | some .unload => "Unload"
  | _ => "Keep"

def main : IO Unit := do
  for mask in List.range 8 do
    IO.println s!"idle\t{mask}\t{phaseName (idle (bit mask 0) (bit mask 1) (bit mask 2))}"
  for mask in List.range 64 do
    let old := mask / 4 % 4
    let next := mask / 16 % 4
    let failed := bit mask 0
    let inertia := bit mask 1
    let result := rustSetEpoch failed inertia (old == next) (old == 0) (next != 0) false
    let stored := if failed then old else next
    IO.println s!"set_epoch\t{mask}\t{observedName result}:{stored}"
  for mask in List.range 8 do
    let sameBefore := bit mask 0
    let throws := bit mask 1
    let supersededDuringCallback := bit mask 2
    let current := sameBefore && !supersededDuringCallback
    let finalSame := current && !throws
    let epoch := if current then (if throws then "inactive" else "old") else "next"
    let calls := if sameBefore then 1 else 0
    let recordedError := current && throws
    IO.println s!"reload\t{mask}\t{observedName (observe (finish true finalSame false false true))}:{epoch}:{calls}:{recordedError}"
  for mask in List.range 2 do
    let active := bit mask 0
    IO.println s!"unload\t{mask}\t{observedName (observe (finish false false true false active))}"
  for mask in List.range 32 do
    let count := mask % 4
    let target : Epoch := if bit mask 2 then some (1, []) else none
    let initial : UpstreamSimulation.State :=
      {target, busy := true, effects := List.range count, committed := [1, 2]}
    let started := UpstreamSimulation.tsStep initial .beginCleanup
    let events := (List.range count).map UpstreamSimulation.Event.cleaned ++ [.finishUnload]
    let finished := UpstreamSimulation.tsRun started events
    let action := if finished.loading then "Load" else "Keep"
    let order := String.intercalate "," (started.cleaning.map toString)
    let errors := if bit mask 3 && count > 0 then 1 else 0
    IO.println s!"cleanup\t{mask}\t{action}|{order}|{finished.committed.isEmpty}|1|1|{errors}|true|false"
