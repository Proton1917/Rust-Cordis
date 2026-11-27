import Lean

/-!
Inspect the kernel environment, including private declarations and generated
helpers. The verifier supplies the closed module inventory and checks every
returned axiom set. Source-level theorem discovery is used only for reporting;
it does not decide which declarations receive this audit.
-/

open Lean Elab Command

def auditModules (modules : Array String) : CommandElabM Unit := do
  let env ← getEnv
  let mut declarations : Array Json := #[]
  for (name, info) in env.constants.toList do
    if let some index := env.getModuleIdxFor? name then
      let moduleName := env.header.moduleNames[index]!.toString
      if modules.contains moduleName then
        let axioms ← collectAxioms name
        declarations := declarations.push <| Json.mkObj [
          ("name", toJson name.toString),
          ("module", toJson moduleName),
          ("theorem", toJson info.isTheorem),
          ("compiler_auxiliary", toJson info.isUnsafe),
          ("partial_definition", toJson info.isPartial),
          ("axioms", toJson (axioms.map Name.toString))]
  liftIO <| IO.println ("CORDIS_AUDIT\t" ++ (Json.arr declarations).compress)
