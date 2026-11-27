import Architecture

open Cordis.Architecture

def enabled (mask bit : Nat) : Bool := mask / 2^bit % 2 == 1

def metadataKeys : List MetadataKey :=
  [.text "cordis.", .text "cordis.caller", .text "cordis.caller.extra",
   .text "cordis", .text "cordisExtra", .text "", .text "用户", .other 7]

def keyName : MetadataKey → String
  | .text key => "s:" ++ key
  | .other id => "n:" ++ toString id

def renderFields (fields : Metadata) : String :=
  String.intercalate ";" (fields.map (fun field => keyName field.1 ++ "=" ++ toString field.2))

def metadataExpected (kind mask query : Nat) (trusted : Bool) : String :=
  let untrusted := if query = 1 then [(.text "other.user", 700)]
    else if query = 0 ∧ kind = 2 then
    metadataKeys.zipIdx |>.filterMap (fun (key, id) => if enabled mask id then some (key,id+10) else none)
    else []
  if query = 0 ∧ kind = 3 then "error:2"
  else if !trusted then "error:4"
  else renderFields (scopedMetadata untrusted
    [(.text "cordis.caller", 900), (.text "cordis.caller", 901), (.text "origin", 902)])

def originExpected (mask stored query : Nat) : String :=
  let available := enabled mask 0 && enabled mask 1 && enabled mask 2 && stored == query
  let active := enabled mask 3 && enabled mask 4 && enabled mask 5 &&
    !enabled mask 6 && !enabled mask 7
  let generation : Generation := ⟨11,stored,31,41,51,active⟩
  if available then
    match requestOrigin generation (if enabled mask 8 then 11 else 12) query with
    | none => "error:4"
    | some origin => s!"{origin.connection},{origin.context},{origin.instanceId},{origin.group},{origin.plugin}"
  else "error:4"

def main : IO Unit := do
  for kind in List.range 5 do
    for mask in List.range 256 do
      for trusted in [false,true] do
        for query in List.range 3 do
          let id := ((kind * 256 + mask) * 2 + if trusted then 1 else 0) * 3 + query
          IO.println s!"metadata\t{id}\t{metadataExpected kind mask query trusted}"
  for (stored, storedIndex) in ([1,23,18446744073709551615] : List Nat).zipIdx do
    for (query, queryIndex) in ([1,23,18446744073709551615,0] : List Nat).zipIdx do
      for mask in List.range 512 do
        let id := (storedIndex * 4 + queryIndex) * 512 + mask
        IO.println s!"origin\t{id}\t{originExpected mask stored query}"
