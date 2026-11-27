import NativeOwnership
import Concurrency
import AsyncSemantics

open Cordis NativeOwnership

def bools : List Bool := [false, true]
def maximum : Nat := 2 ^ 64 - 1

def main : IO Unit := do
  for count in [0, 1, 2, 4, maximum - 1, maximum] do
    for retired in bools do
      for unload in bools do
        for descriptor in [none, some false, some true] do
          for action in [0, 1, 2] do
            let s : Lease := { count, retired, unload, descriptor }
            let event := if action == 0 then LeaseEvent.acquire else
              if action == 1 then LeaseEvent.release else LeaseEvent.reap
            let t := leaseStep maximum s event
            let d := match descriptor with | none => 0 | some false => 1 | some true => 2
            IO.println s!"{count}\t{bit retired}\t{bit unload}\t{d}\t{action}\t{t.count}\t{bit t.libraryPresent}\t{bit t.cachePresent}\t{t.closes}"
