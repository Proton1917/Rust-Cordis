import Protocol
import SdkProtocol

/-! Executable vectors for the actual `Lease::grow` body. This is a bounded
correspondence experiment; the quantified budget proofs live in Protocol.lean. -/

open Cordis.Protocol

private def emitBudget (maximum reserve usedData usedControl leaseData leaseControl : Nat)
    (privileged : Bool) (target : Nat) : IO Unit := do
  let b : Budget := ⟨maximum, reserve⟩
  let u : Usage := ⟨usedData, usedControl⟩
  let needed := target - (leaseData + leaseControl)
  let accepted := canGrow b u needed privileged
  let next := grow b u needed privileged
  let a := allocation b u needed
  let nextData := leaseData + if accepted then a.data else 0
  let nextControl := leaseControl + if accepted then a.control else 0
  let row := [maximum, reserve, usedData, usedControl, leaseData, leaseControl,
    bit privileged, target, bit accepted, next.data, next.control, nextData, nextControl]
  IO.println (String.intercalate "\t" (row.map toString))

private def budgetVectors : IO Unit := do
  for maximum in [0, 1, 4, 8] do
    for reserve in [0, 1, 3] do
      for usedData in List.range (maximum + 1) do
        for usedControl in List.range (reserve + 1) do
          for leaseData in List.range (usedData + 1) do
            for leaseControl in List.range (usedControl + 1) do
              for privileged in [false, true] do
                let capacity := leaseData + leaseControl
                for target in [0, 1, capacity, capacity + 1, maximum + reserve,
                    maximum + reserve + 1] do
                  emitBudget maximum reserve usedData usedControl leaseData leaseControl privileged target

  -- Include finite-width boundaries without enumerating their entire ranges.
  for maximum in [2147483647, 4294967292, 9223372036854775807, 18446744073709551612] do
    for reserve in [0, 1, 3] do
      for usedData in [0, 1, maximum - 1, maximum] do
        for usedControl in [0, reserve] do
          for leaseData in [0, usedData] do
            for leaseControl in [0, usedControl] do
              for privileged in [false, true] do
                for target in [0, 1, maximum, maximum + reserve] do
                  emitBudget maximum reserve usedData usedControl leaseData leaseControl privileged target

private def sdkTraces : Nat → List (List Cordis.SdkProtocol.Event)
  | 0 => [[]]
  | n + 1 => [.ok, .error, .cleanup].flatMap fun event =>
      (sdkTraces n).map (event :: ·)

private def sdkVectors : IO Unit := do
  for style in [Cordis.SdkProtocol.Style.typescript, .python] do
    for length in List.range 7 do
      for events in sdkTraces length do
        let result := Cordis.SdkProtocol.run style {} events
        let code := String.mk (events.map fun event =>
          match event with | .ok => 'o' | .error => 'e' | _ => 'c')
        let code := if code.isEmpty then "-" else code
        let styleCode := match style with | .typescript => "0" | .python => "1"
        let values := [bit result.present, bit result.done, result.settlements,
          result.payloads, result.accepted, result.lateClosed]
        IO.println (String.intercalate "\t" (styleCode :: code :: values.map toString))

def main (args : List String) : IO Unit := do
  if args == ["--sdk"] then sdkVectors else budgetVectors
