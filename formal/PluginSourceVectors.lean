import PluginPrinciple

/-!
The source bridge has two distinct observation boundaries. `Definition` stores
an opaque callback in the kernel. A supervisor generation projects the backend
connection and the `apply` request without interpreting application payloads.
Transport, scheduling and serialization are explicit adapters in the compiled
probe. This module does not claim to verify those adapters.
-/

namespace Cordis.PluginSource

structure RemoteGeneration (Payload Inherited : Type) where
  plugin : String
  component : String
  library : Option Nat
  context : Nat
  config : Payload
  inherited : Option Inherited
  connection : Nat
  deriving DecidableEq

structure ApplyRequest (Payload Inherited : Type) where
  plugin : String
  component : String
  library : Option Nat
  context : Nat
  config : Payload
  inherited : Option Inherited
  deriving DecidableEq

structure ApplyRoute (Payload Inherited : Type) where
  connection : Nat
  method : String
  request : ApplyRequest Payload Inherited
  timeout : Nat
  deriving DecidableEq

def requestOf (generation : RemoteGeneration Payload Inherited) : ApplyRequest Payload Inherited :=
  ⟨generation.plugin, generation.component, generation.library, generation.context,
    generation.config, generation.inherited⟩

def routeApply (generation : RemoteGeneration Payload Inherited) (timeout : Nat) :
    ApplyRoute Payload Inherited :=
  ⟨generation.connection, "apply", requestOf generation, timeout⟩

def optional (value : Option Nat) : String :=
  match value with
  | none => "-"
  | some n => toString n

def dependencyText (dependencies : List Cordis.Bindings.Dependency) : String :=
  String.intercalate "," (dependencies.map fun d => s!"{d.key}={d.required}")

def dependencyCase : Nat → List Cordis.Bindings.Dependency
  | 0 => []
  | 1 => [⟨"alpha@1", true⟩]
  | 2 => [⟨"beta@2", false⟩]
  | _ => [⟨"α@1", false⟩, ⟨"β@2", true⟩]

def nameCase : Nat → String
  | 0 => ""
  | 1 => "fresh.extension/Ω"
  | 2 => "cordis-supervisor"
  | _ => "web-owner-new"

def constructorLine (case : Nat) : String := Id.run do
  let implementation := case % 2
  let raw := [0, 9, 35][case / 2 % 3]!
  let context := if case / 6 % 2 == 0 then 13 else 29
  let dependencies := dependencyCase (case / 12 % 4)
  let name := nameCase (case / 48)
  let definition : PluginPrinciple.Definition Nat := ⟨name, dependencies, implementation⟩
  return s!"{definition.name}|{dependencyText definition.inject}|{context}|{raw}|{definition.implementation}|true"

def remoteLine (case : Nat) : String := Id.run do
  let plugin := if case % 2 == 0 then "plugin/α" else "new.application"
  let component := if case / 2 % 2 == 0 then "component/β" else "worker"
  let library := if case / 4 % 2 == 0 then none else some 37
  let context := if case / 8 % 2 == 0 then 41 else 53
  let config := if case / 16 % 2 == 0 then 67 else 71
  let inherited := if case / 32 % 2 == 0 then none else some 83
  let connection := if case / 64 % 2 == 0 then 97 else 101
  let timeout := if case / 128 % 2 == 0 then 103 else 107
  let response := case / 256 % 2 == 0
  let generation : RemoteGeneration Nat Nat :=
    ⟨plugin, component, library, context, config, inherited, connection⟩
  let route := routeApply generation timeout
  let request := route.request
  return s!"{route.connection}|{route.method}|{request.plugin}|{request.component}|{optional request.library}|{request.context}|{request.config}|{optional request.inherited}|{route.timeout}|{response}"

def identityLimit : Nat := 18446744073709551615

/-- The counter names the next identity; saturation preserves its value. -/
def allocate (counter : Nat) : Option Nat × Nat :=
  match WireSemantics.allocateId identityLimit counter with
  | some (identity, next) => (some identity, next)
  | none => (none, counter)

def identityLine (case : Nat) : String := Id.run do
  let initial := [0, 1, identityLimit - 2, identityLimit - 1, identityLimit][case / 3]!
  let mut counter := initial
  let mut result := none
  for _ in List.range (case % 3 + 1) do
    let next := allocate counter
    result := next.1
    counter := next.2
  return s!"{optional result}|{counter}"

end Cordis.PluginSource

def main : IO Unit := do
  for i in List.range 192 do
    IO.println s!"constructor\t{i}\t{Cordis.PluginSource.constructorLine i}"
  for i in List.range 512 do
    IO.println s!"remote\t{i}\t{Cordis.PluginSource.remoteLine i}"
  for i in List.range 15 do
    IO.println s!"identity\t{i}\t{Cordis.PluginSource.identityLine i}"
