import StorageProgram
import DirectoryInitialization

open Cordis StorageProgram

def ioLabel : Io → String
  | .preserveName key => s!"preserve:{key}"
  | .syncJournal => "sync-journal"
  | .renameJournal => "rename-journal"
  | .syncJournalParent => "sync-journal-parent"
  | .createTemp key => s!"create:{key}"
  | .writeAll value => s!"write:{value}"
  | .syncFile => "sync-file"
  | .renameFile key => s!"rename:{key}"
  | .removeName key => s!"remove:{key}"
  | .syncDataDirectory => "sync-data"
  | .moveDecision => "move-decision"
  | .syncDecisionDirectory => "sync-decision"

def parseWrite (text : String) : Persistence.Change :=
  let parts := text.splitOn ":"
  (parts[0]!.toNat!, if parts[1]! == "-" then none else some parts[1]!.toNat!)

def main (args : List String) : IO Unit := do
  for plan in args do
    if plan.startsWith "init:" then
      let count := (plan.drop 5).toNat!
      let required := List.range count
      IO.println s!"INITPLAN\t{count}"
      for fault in List.range (count + 1) do
        let mut machine := DirectoryInitialization.initial required (fun _ => false)
        let mut observed : List Nat := []
        for index in List.range count do
          if let some name := DirectoryInitialization.nextIo machine then
            observed := observed ++ [name]
            machine := DirectoryInitialization.returns machine (fault == 0 || index + 1 != fault)
        let firstReady := DirectoryInitialization.ready machine
        let firstConfirmed := machine.confirmed.length
        machine := DirectoryInitialization.retry required machine
        let mut retried : List Nat := []
        for _ in List.range count do
          if let some name := DirectoryInitialization.nextIo machine then
            retried := retried ++ [name]
            machine := DirectoryInitialization.returns machine true
        IO.println s!"INITCASE\t{fault}\t{String.intercalate "," (observed.map toString)}\t{firstConfirmed}\t{firstReady}\t{String.intercalate "," (retried.map toString)}\t{DirectoryInitialization.ready machine}"
      IO.println "INITEND"
      continue
    let parts := plan.splitOn "|"
    let order := (parts[0]!.splitOn ",").map String.toNat!
    let writes := (parts[1]!.splitOn ",").map parseWrite
    let mut machine := initialWithBackupOrder (fun key => some (10 + key)) writes order
    IO.println s!"PLAN\t{plan}"
    for _ in List.range (order.length + 4 * writes.length + 8) do
      if let some operation := nextIo machine then
        IO.println s!"IO\t{ioLabel operation}"
        machine := returns machine true
    if machine.cursor != .committed then throw (IO.userError "storage program failed to terminate")
    IO.println "END"
