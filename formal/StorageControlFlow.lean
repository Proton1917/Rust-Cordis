import StorageProgram

/-!
Ordered control-flow certificate for the supported storage-method subset.
The source extractor emits the template and Lean checks it against this code.
`Execution` independently checks each I/O constructor against `nextIo`; loops
range over arbitrary lists, including deletions. Rust parsing, primitive I/O
and expression bindings remain explicit extractor/OS contracts, separately
checked by compiling and executing unchanged production Rust bodies.
-/
namespace Cordis.StorageControlFlow
open StorageProgram Persistence

inductive Publication where
  | create | write | sync | rename
  deriving DecidableEq, Repr
inductive Journal where
  | backups | sync | rename | confirm
  deriving DecidableEq, Repr
inductive CommitGroup where
  | base | states | overlay | sources
  deriving DecidableEq, Repr
inductive Finish where
  | syncData | moveDecision | syncDecision
  deriving DecidableEq, Repr
inductive EntryBoundary where
  | encode | lock | prepareKeys | commitSame
  deriving DecidableEq, Repr
inductive RestoreFile where
  | removeTemporary | linkJournal | publishTemporary | removeTarget
  deriving DecidableEq, Repr
structure Template where
  publication : List Publication
  journal : List Journal
  groups : List CommitGroup
  finish : List Finish
  entry : List EntryBoundary
  restorePresent : List RestoreFile
  restoreAbsent : List RestoreFile
  deriving DecidableEq, Repr

def canonical : Template :=
  { publication := [.create, .write, .sync, .rename]
    journal := [.backups, .sync, .rename, .confirm]
    groups := [.base, .states, .overlay, .sources]
    finish := [.syncData, .moveDecision, .syncDecision]
    entry := [.encode, .lock, .prepareKeys, .commitSame]
    restorePresent := [.removeTemporary, .linkJournal, .publishTemporary, .removeTemporary]
    restoreAbsent := [.removeTarget] }

def publicationIo (code : List Publication) (key value : Nat) : List Io :=
  code.map fun
    | .create => .createTemp key
    | .write => .writeAll value
    | .sync => .syncFile
    | .rename => .renameFile key

def changeIo (code : List Publication) : Change → List Io
  | (key, some value) => publicationIo code key value
  | (key, none) => [.removeName key]

def journalIo (code : List Journal) (order : List Nat) : List Io :=
  code.flatMap fun
    | .backups => order.map Io.preserveName
    | .sync => [.syncJournal]
    | .rename => [.renameJournal]
    | .confirm => [.syncJournalParent]

def finishIo (code : List Finish) : List Io :=
  code.map fun
    | .syncData => .syncDataDirectory
    | .moveDecision => .moveDecision
    | .syncDecision => .syncDecisionDirectory

def changes (code : List CommitGroup) (base overlay sources : Change)
    (states : List Change) : List Change :=
  code.flatMap fun
    | .base => [base]
    | .states => states
    | .overlay => [overlay]
    | .sources => [sources]

structure Prepared where
  base : Change
  states : List Change
  overlay : Change
  sources : Change

def Prepared.writes (p : Prepared) : List Change :=
  changes canonical.groups p.base p.overlay p.sources p.states

def Prepared.backupNames (p : Prepared) : List Nat :=
  [p.overlay.1, p.sources.1, p.base.1] ++ p.states.map Prod.fst

structure EntryState where
  prepared : Prepared
  encoded : Bool := false
  locked : Bool := false
  backups : List Nat := []
  writes : List Change := []

def entryStep (s : EntryState) : EntryBoundary → EntryState
  | .encode => {s with encoded := true}
  | .lock => if s.encoded then {s with locked := true} else s
  | .prepareKeys =>
    if s.encoded && s.locked then {s with backups := s.prepared.backupNames} else s
  | .commitSame =>
    if s.encoded && s.locked then {s with writes := s.prepared.writes} else s

def entryRun : EntryState → List EntryBoundary → EntryState
  | s, [] => s
  | s, action :: rest => entryRun (entryStep s action) rest

theorem entry_keeps_same_prepared (s : EntryState) (actions : List EntryBoundary) :
    (entryRun s actions).prepared = s.prepared := by
  induction actions generalizing s with
  | nil => rfl
  | cons action rest ih =>
    simp only [entryRun, ih]
    cases action <;> simp only [entryStep]
    all_goals try split
    all_goals rfl

theorem prepared_keys_cover_every_write (p : Prepared) (name : Nat)
    (modified : name ∈ p.writes.map Prod.fst) : name ∈ p.backupNames := by
  simpa [Prepared.writes, Prepared.backupNames, changes, canonical, or_assoc,
    or_left_comm, or_comm] using modified

theorem source_entry_constructs_both_plans (code : Template) (certificate : code = canonical)
    (p : Prepared) :
    (entryRun {prepared := p} code.entry).backups = p.backupNames ∧
      (entryRun {prepared := p} code.entry).writes = p.writes := by
  subst code
  simp [entryRun, entryStep, canonical]

theorem source_entry_derives_backup_coverage (code : Template)
    (certificate : code = canonical) (p : Prepared) (name : Nat)
    (modified : name ∈ (entryRun {prepared := p} code.entry).writes.map Prod.fst) :
    name ∈ (entryRun {prepared := p} code.entry).backups := by
  have plans := source_entry_constructs_both_plans code certificate p
  rw [plans.1]
  apply prepared_keys_cover_every_write p name
  simpa [plans.2] using modified

structure RestoringFile where
  target : Option Nat
  temporary : Option Nat
  deriving DecidableEq

def restoreFileStep (original : Nat) (s : RestoringFile) : RestoreFile → RestoringFile
  | .removeTemporary => {s with temporary := none}
  | .linkJournal => {s with temporary := some original}
  | .publishTemporary => ⟨s.temporary, none⟩
  | .removeTarget => {s with target := none}

def restoreFileRun (original : Nat) : RestoringFile → List RestoreFile → RestoringFile
  | s, [] => s
  | s, action :: rest => restoreFileRun original (restoreFileStep original s action) rest

def restoreFile (code : Template) (original : Option Nat) (target temporary : Option Nat) : Option Nat :=
  match original with
  | none => (restoreFileRun 0 ⟨target, temporary⟩ code.restoreAbsent).target
  | some bytes => (restoreFileRun bytes ⟨target, temporary⟩ code.restorePresent).target

theorem certified_file_restore_uses_journal_bytes (code : Template)
    (certificate : code = canonical) (original target temporary : Option Nat) :
    restoreFile code original target temporary = original := by
  subst code
  cases original <;> rfl

def sourceReplay (code : Template) (journal : Nat → Option (Option Nat)) :
    List Nat → Image → Option Image
  | [], image => some image
  | name :: rest, image => match journal name with
    | none => none
    | some original =>
      sourceReplay code journal rest (put image name (restoreFile code original (image name) none))

theorem certified_recovery_loop_refines_replay (code : Template)
    (certificate : code = canonical) (journal : Nat → Option (Option Nat))
    (names : List Nat) (image : Image) :
    sourceReplay code journal names image = replayJournal journal names image := by
  induction names generalizing image with
  | nil => rfl
  | cons name rest ih =>
    simp only [sourceReplay, replayJournal]
    cases entry : journal name with
    | none => rfl
    | some original =>
      change sourceReplay code journal rest (put image name (restoreFile code original (image name) none)) =
        replayJournal journal rest (put image name original)
      rw [certified_file_restore_uses_journal_bytes code certificate]
      exact ih _

theorem certified_recovery_restores_every_original_name (code : Template)
    (certificate : code = canonical) (before image : Image)
    (journal : Nat → Option (Option Nat)) (names : List Nat)
    (complete : ∀ name ∈ names, journal name = some (before name)) :
    sourceReplay code journal names image = some (restore before names image) := by
  rw [certified_recovery_loop_refines_replay code certificate]
  exact complete_journal_replays_original before image journal names complete

def sourceRecovery (code : Template) (names : List Nat) (s : Machine)
    (markerPersisted : Bool) : Option Image :=
  match phase s.cursor with
  | .preparing | .acknowledged => some s.stable
  | .writing | .dataSynced => sourceReplay code s.journal names s.stable
  | .decisionMoved =>
    if markerPersisted then some s.stable else sourceReplay code s.journal names s.stable

theorem certified_recovery_matches_actual_recovery (code : Template)
    (certificate : code = canonical) (names : List Nat) (s : Machine)
    (markerPersisted : Bool) :
    sourceRecovery code names s markerPersisted = actualRecovery names s markerPersisted := by
  cases h : phase s.cursor <;>
    simp [sourceRecovery, actualRecovery, h, certified_recovery_loop_refines_replay code certificate]

theorem certified_entry_transaction_recovers_old_or_new (code : Template)
    (certificate : code = canonical) (before : Image) (p : Prepared)
    (events : List StorageProgram.Event) (markerPersisted : Bool) :
    let entry := entryRun {prepared := p} code.entry
    let machine := run (initialWithBackupOrder before entry.writes entry.backups) events
    sourceRecovery code (entry.writes.map Prod.fst) machine markerPersisted = some before ∨
      sourceRecovery code (entry.writes.map Prod.fst) machine markerPersisted =
        some (applyWrites before entry.writes) := by
  dsimp only
  rw [certified_recovery_matches_actual_recovery code certificate]
  exact complete_source_transaction_recovers_old_or_new before
    (entryRun {prepared := p} code.entry).writes
    (entryRun {prepared := p} code.entry).backups
    (source_entry_derives_backup_coverage code certificate p) events markerPersisted

def compile (code : Template) (order : List Nat) (writes : List Change) : List Io :=
  journalIo code.journal order ++ writes.flatMap (changeIo code.publication) ++
    finishIo code.finish

inductive Execution : Machine → List Io → Machine → Prop
  | done (s : Machine) : Execution s [] s
  | call {s t : Machine} {operation : Io} {rest : List Io}
      (next : nextIo s = some operation)
      (tail : Execution (returns s true) rest t) :
      Execution s (operation :: rest) t

theorem execution_append {s middle t : Machine} {first second : List Io}
    (head : Execution s first middle) (tail : Execution middle second t) :
    Execution s (first ++ second) t := by
  induction head with
  | done => exact tail
  | call next _ ih => exact .call next (ih tail)

theorem backups_execute (order : List Nat) (s : Machine)
    (active : s.failed = false) (cursor : s.cursor = .backup)
    (pending : s.backupTodo = order) :
    ∃ t, Execution s (order.map Io.preserveName) t ∧
      t.failed = false ∧ t.cursor = .backup ∧ t.backupTodo = [] ∧ t.todo = s.todo := by
  induction order generalizing s with
  | nil => exact ⟨s, .done s, active, cursor, pending, rfl⟩
  | cons key rest ih =>
    have next : nextIo s = some (.preserveName key) := by simp [nextIo, active, cursor, pending]
    have result := ih (returns s true)
      (by simp [returns, advance, active, cursor, pending])
      (by simp [returns, advance, active, cursor, pending])
      (by simp [returns, advance, active, cursor, pending])
    obtain ⟨t, execution, ht, hc, hb, hw⟩ := result
    refine ⟨t, .call next execution, ht, hc, hb, ?_⟩
    simpa [returns, advance, active, cursor, pending] using hw

theorem journal_barriers_execute (s : Machine)
    (active : s.failed = false) (cursor : s.cursor = .backup)
    (empty : s.backupTodo = []) :
    ∃ t, Execution s [.syncJournal, .renameJournal, .syncJournalParent] t ∧
      t.failed = false ∧ t.cursor = .nextFile ∧ t.todo = s.todo := by
  let a := returns s true
  let b := returns a true
  let t := returns b true
  refine ⟨t, .call ?_ (.call ?_ (.call ?_ (.done t))), ?_, ?_, ?_⟩ <;>
    simp [a, b, t, nextIo, returns, advance, active, cursor, empty]

theorem changes_execute (writes : List Change) (s : Machine)
    (active : s.failed = false) (cursor : s.cursor = .nextFile)
    (pending : s.todo = writes) :
    ∃ t, Execution s (writes.flatMap (changeIo canonical.publication)) t ∧
      t.failed = false ∧ t.cursor = .nextFile ∧ t.todo = [] := by
  induction writes generalizing s with
  | nil => exact ⟨s, .done s, active, cursor, pending⟩
  | cons change rest ih =>
    rcases change with ⟨key, value⟩
    cases value with
    | none =>
      obtain ⟨t, execution, ht, hc, hw⟩ := ih (returns s true)
        (by simp [returns, advance, active, cursor, pending])
        (by simp [returns, advance, active, cursor, pending])
        (by simp [returns, advance, active, cursor, pending])
      refine ⟨t, ?_, ht, hc, hw⟩
      simpa [changeIo] using Execution.call
        (s := s) (operation := .removeName key)
        (by simp [nextIo, active, cursor, pending]) execution
    | some value =>
      let a := returns s true
      let b := returns a true
      let c := returns b true
      let d := returns c true
      obtain ⟨t, execution, ht, hc, hw⟩ := ih d
        (by simp [a, b, c, d, returns, advance, active, cursor, pending])
        (by simp [a, b, c, d, returns, advance, active, cursor, pending])
        (by simp [a, b, c, d, returns, advance, active, cursor, pending])
      refine ⟨t, ?_, ht, hc, hw⟩
      have head : Execution s [.createTemp key, .writeAll value, .syncFile, .renameFile key] d := by
        refine .call ?_ (.call ?_ (.call ?_ (.call ?_ (.done d)))) <;>
          simp [a, b, c, d, nextIo, returns, advance, active, cursor, pending]
      simpa [changeIo, publicationIo, canonical] using execution_append head execution

theorem finish_execute (s : Machine)
    (active : s.failed = false) (cursor : s.cursor = .nextFile) (empty : s.todo = []) :
    ∃ t, Execution s [.syncDataDirectory, .moveDecision, .syncDecisionDirectory] t ∧
      t.failed = false ∧ t.cursor = .committed := by
  let a := returns s true
  let b := returns a true
  let t := returns b true
  refine ⟨t, .call ?_ (.call ?_ (.call ?_ (.done t))), ?_, ?_⟩ <;>
    simp [a, b, t, nextIo, returns, advance, active, cursor, empty]

theorem canonical_code_follows_every_io (before : Image)
    (writes : List Change) (order : List Nat) :
    ∃ t, Execution (initialWithBackupOrder before writes order)
      (compile canonical order writes) t ∧ t.cursor = .committed := by
  let initialState := initialWithBackupOrder before writes order
  obtain ⟨a, first, ha, ca, ba, wa⟩ := backups_execute order initialState rfl rfl rfl
  obtain ⟨b, second, hb, cb, wb⟩ := journal_barriers_execute a ha ca ba
  obtain ⟨c, third, hc, cc, wc⟩ := changes_execute writes b hb cb (by
    simpa [initialState, initialWithBackupOrder, StorageProgram.initial] using wb.trans wa)
  obtain ⟨t, fourth, ht, ct⟩ := finish_execute c hc cc wc
  refine ⟨t, ?_, ct⟩
  have all := execution_append (execution_append (execution_append first second) third) fourth
  simpa [compile, canonical, journalIo, finishIo, List.append_assoc] using all

theorem source_certificate_covers_arbitrary_loops (code : Template)
    (certificate : code = canonical) (before : Image)
    (writes : List Change) (order : List Nat) :
    ∃ t, Execution (initialWithBackupOrder before writes order)
      (compile code order writes) t ∧ t.cursor = .committed := by
  subst code
  exact canonical_code_follows_every_io before writes order

theorem execution_uses_actual_cursor {s t : Machine} {operation : Io} {rest : List Io}
    (execution : Execution s (operation :: rest) t) : nextIo s = some operation := by
  cases execution with | call next _ => exact next

theorem execution_consumes_returns {s t : Machine} {operations : List Io}
    (execution : Execution s operations t) :
    run s (operations.map fun _ => Event.ioReturn true) = t := by
  induction execution with
  | done => rfl
  | call next tail ih => simpa [run, step] using ih

theorem execution_prefix {s t : Machine} {operations : List Io}
    (execution : Execution s operations t) (count : Nat) :
    ∃ prefixState, Execution s (operations.take count) prefixState := by
  induction count generalizing s t operations with
  | zero => exact ⟨s, .done s⟩
  | succ count ih =>
    cases execution with
    | done => exact ⟨s, .done s⟩
    | call next tail =>
      obtain ⟨prefixState, prefixExecution⟩ := ih tail
      exact ⟨prefixState, .call next prefixExecution⟩

theorem certified_source_prefix_recovers_from_its_journal (code : Template)
    (certificate : code = canonical) (before : Image)
    (writes : List Change) (order : List Nat)
    (covers : ∀ name ∈ writes.map Prod.fst, name ∈ order)
    (count : Nat) (markerPersisted : Bool) :
    ∃ prefixState,
      Execution (initialWithBackupOrder before writes order)
        ((compile code order writes).take count) prefixState ∧
      (actualRecovery (writes.map Prod.fst) prefixState markerPersisted = some before ∨
       actualRecovery (writes.map Prod.fst) prefixState markerPersisted = some (applyWrites before writes)) := by
  obtain ⟨t, execution, committed⟩ :=
    source_certificate_covers_arbitrary_loops code certificate before writes order
  obtain ⟨prefixState, prefixExecution⟩ := execution_prefix execution count
  refine ⟨prefixState, prefixExecution, ?_⟩
  have recovered := complete_source_transaction_recovers_old_or_new before writes order covers
    (((compile code order writes).take count).map fun _ => Event.ioReturn true) markerPersisted
  dsimp only at recovered
  rw [execution_consumes_returns prefixExecution] at recovered
  exact recovered

theorem source_groups_cover_arbitrary_state_count (code : Template)
    (certificate : code = canonical) (before : Image)
    (base overlay sources : Change) (states : List Change) (order : List Nat) :
    let writes := changes code.groups base overlay sources states
    ∃ t, Execution (initialWithBackupOrder before writes order)
      (compile code order writes) t ∧ t.cursor = .committed :=
  source_certificate_covers_arbitrary_loops code certificate before _ order

end Cordis.StorageControlFlow
