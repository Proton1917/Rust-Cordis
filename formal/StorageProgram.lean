import Persistence

/-!
Operational storage program, with separate I/O return points. The concrete
cursor follows prepare_deployment, publish, commit_prepared and sync_decision.
An unsuccessful I/O stops that invocation. The OS may persist individual target
names between calls; recovery uses the already durable undo journal.
-/

namespace Cordis.StorageProgram

open Persistence

inductive Cursor where
  | backup | syncJournal | publishJournal | confirmJournal
  | nextFile | created | written | fileSynced
  | dataSynced | decisionMoved | committed
  deriving DecidableEq

def phase : Cursor → Phase
  | .backup | .syncJournal | .publishJournal | .confirmJournal => .preparing
  | .nextFile | .created | .written | .fileSynced => .writing
  | .dataSynced => .dataSynced
  | .decisionMoved => .decisionMoved
  | .committed => .acknowledged

structure Machine where
  cursor : Cursor := .backup
  contents : Image
  stable : Image
  todo : List Change
  backupTodo : List Nat
  journal : Nat → Option (Option Nat) := fun _ => none
  failed : Bool := false

def project (s : Machine) : Persistence.State :=
  { phase := phase s.cursor
    visible := s.contents
    durable := s.stable
    remaining := s.todo
    staged := match s.cursor with | .created | .written | .fileSynced => true | _ => false
    fileSynced := s.cursor == .fileSynced }

def initial (before : Image) (writes : List Change) : Machine :=
  { contents := before, stable := before, todo := writes,
    backupTodo := writes.map Prod.fst }

def initialWithBackupOrder (before : Image) (writes : List Change)
    (backupOrder : List Nat) : Machine :=
  { initial before writes with backupTodo := backupOrder }

inductive Io where
  | preserveName (name : Nat)
  | syncJournal | renameJournal | syncJournalParent
  | createTemp (name : Nat)
  | writeAll (value : Nat)
  | syncFile | renameFile (name : Nat)
  | removeName (name : Nat)
  | syncDataDirectory | moveDecision | syncDecisionDirectory
  deriving DecidableEq

def nextIo (s : Machine) : Option Io :=
  if s.failed then none else
  match s.cursor with
  | .backup => match s.backupTodo with
    | key :: _ => some (.preserveName key)
    | [] => some .syncJournal
  | .syncJournal => some .renameJournal
  | .publishJournal => some .syncJournalParent
  | .confirmJournal => some .syncJournalParent
  | .nextFile => match s.todo with
    | (key, some _) :: _ => some (.createTemp key)
    | (key, none) :: _ => some (.removeName key)
    | [] => some .syncDataDirectory
  | .created => match s.todo with
    | (_, some value) :: _ => some (.writeAll value)
    | _ => none
  | .written => some .syncFile
  | .fileSynced => match s.todo with
    | (key, some _) :: _ => some (.renameFile key)
    | _ => none
  | .dataSynced => some .moveDecision
  | .decisionMoved => some .syncDecisionDirectory
  | .committed => none

def advance (s : Machine) : Machine :=
  match s.cursor with
  | .backup => match s.backupTodo with
    | key :: rest =>
      { s with
        backupTodo := rest
        journal := fun other => if other = key then some (s.contents key) else s.journal other }
    | [] => { s with cursor := .syncJournal }
  | .syncJournal => { s with cursor := .publishJournal }
  | .publishJournal | .confirmJournal => { s with cursor := .nextFile }
  | .nextFile => match s.todo with
    | (key, some value) :: rest => { s with cursor := .created }
    | (key, none) :: rest => { s with contents := put s.contents key none, todo := rest }
    | [] => { s with cursor := .dataSynced, stable := s.contents }
  | .created => { s with cursor := .written }
  | .written => { s with cursor := .fileSynced }
  | .fileSynced => match s.todo with
    | (key, some value) :: rest =>
        { s with cursor := .nextFile, contents := put s.contents key (some value), todo := rest }
    | _ => s
  | .dataSynced => { s with cursor := .decisionMoved }
  | .decisionMoved => { s with cursor := .committed }
  | .committed => s

def returns (s : Machine) (success : Bool) : Machine :=
  if s.failed then s else if success then advance s else { s with failed := true }

def Control (s : Machine) : Prop :=
  (s.cursor = .created ∨ s.cursor = .written ∨ s.cursor = .fileSynced) →
    ∃ key value rest, s.todo = (key, some value) :: rest

theorem initial_control (before : Image) (writes : List Change) : Control (initial before writes) := by
  simp [Control, initial]

theorem advance_preserves_control (s : Machine) (h : Control s) : Control (advance s) := by
  cases hc : s.cursor <;> simp only [advance, hc]
  · cases s.backupTodo <;> simp [Control, hc]
  · simp [Control]
  · simp [Control]
  · simp [Control]
  · cases ht : s.todo with
    | nil => simp [Control]
    | cons change rest =>
      rcases change with ⟨key, value⟩
      cases value <;> simp [Control, hc, ht]
  · obtain ⟨key, value, rest, todo⟩ := h (Or.inl hc)
    simp [Control, todo]
  · obtain ⟨key, value, rest, todo⟩ := h (Or.inr (Or.inl hc))
    simp [Control, todo]
  · obtain ⟨key, value, rest, todo⟩ := h (Or.inr (Or.inr hc))
    simp [todo, Control]
  · simp [Control]
  · simp [Control]
  · exact h

theorem advance_simulates_model (s : Machine) (h : Control s) :
    Persistence.Step (project s) (project (advance s)) := by
  cases hc : s.cursor with
  | backup =>
    cases hb : s.backupTodo <;>
      simpa [advance, project, phase, hc, hb] using Persistence.Step.fault (project s)
  | syncJournal =>
    simpa [advance, project, phase, hc] using Persistence.Step.fault (project s)
  | publishJournal =>
    simpa [advance, project, phase, hc] using
      Persistence.Step.prepare (project s) (by simp [project, phase, hc])
  | confirmJournal =>
    simpa [advance, project, phase, hc] using
      Persistence.Step.prepare (project s) (by simp [project, phase, hc])
  | nextFile =>
    cases ht : s.todo with
    | nil =>
      simpa [advance, project, phase, hc, ht] using
        Persistence.Step.syncDataDirectory (project s)
          (by simp [project, phase, hc]) (by simp [project, ht])
    | cons change rest =>
      rcases change with ⟨key, value⟩
      cases value with
      | none =>
        simpa [advance, project, phase, hc, ht] using
          Persistence.Step.remove (project s) key rest
            (by simp [project, phase, hc]) (by simp [project, ht])
      | some value =>
        simpa [advance, project, phase, hc, ht] using
          Persistence.Step.stage (project s) key value rest
            (by simp [project, phase, hc]) (by simp [project, ht])
  | created =>
    simpa [advance, project, phase, hc] using Persistence.Step.fault (project s)
  | written =>
    simpa [advance, project, phase, hc] using
      Persistence.Step.syncFile (project s) (by simp [project, phase, hc])
        (by simp [project, hc])
  | fileSynced =>
    obtain ⟨key, value, rest, ht⟩ := h (Or.inr (Or.inr hc))
    simpa [advance, project, phase, hc, ht] using
      Persistence.Step.publish (project s) key value rest
        (by simp [project, phase, hc]) (by simp [project, ht]) (by simp [project, hc])
  | dataSynced =>
    simpa [advance, project, phase, hc] using
      Persistence.Step.moveDecision (project s) (by simp [project, phase, hc])
  | decisionMoved =>
    simpa [advance, project, phase, hc] using
      Persistence.Step.confirmDecision (project s) (by simp [project, phase, hc])
  | committed =>
    simpa [advance, project, phase, hc] using Persistence.Step.fault (project s)

theorem return_simulates_model (s : Machine) (success : Bool) (h : Control s) :
    Persistence.Step (project s) (project (returns s success)) := by
  unfold returns
  split
  · exact Persistence.Step.fault _
  · split
    · exact advance_simulates_model s h
    · exact Persistence.Step.fault _

inductive Event where
  | ioReturn (success : Bool)
  | persistName (name : Nat)
  deriving DecidableEq

def step (s : Machine) : Event → Machine
  | .ioReturn success => returns s success
  | .persistName name =>
    if phase s.cursor = .writing then
      { s with stable := put s.stable name (s.contents name) }
    else s

theorem step_preserves_control (s : Machine) (event : Event) (h : Control s) :
    Control (step s event) := by
  cases event with
  | ioReturn success =>
    change Control (returns s success)
    unfold returns
    split
    · exact h
    · split
      · exact advance_preserves_control s h
      · exact h
  | persistName name =>
    change Control (if phase s.cursor = .writing then
      { s with stable := put s.stable name (s.contents name) } else s)
    split <;> exact h

theorem step_simulates_model (s : Machine) (event : Event) (h : Control s) :
    Persistence.Step (project s) (project (step s event)) := by
  cases event with
  | ioReturn success => exact return_simulates_model s success h
  | persistName name =>
    by_cases hp : phase s.cursor = .writing
    · simpa [step, hp, project] using
        Persistence.Step.persistName (project s) name (by simpa [project] using hp)
    · simpa [step, hp] using Persistence.Step.fault (project s)

def run (s : Machine) : List Event → Machine
  | [] => s
  | event :: rest => run (step s event) rest

theorem source_program_projects_reachable (before : Image) (writes : List Change)
    (events : List Event) :
    Persistence.Reachable before writes (project (run (initial before writes) events)) := by
  have general : ∀ events s, Control s → Persistence.Reachable before writes (project s) →
      Persistence.Reachable before writes (project (run s events)) := by
    intro es
    induction es with
    | nil => intro s control reachable; exact reachable
    | cons event rest ih =>
      intro s control reachable
      exact ih _ (step_preserves_control s event control)
        (.next reachable (step_simulates_model s event control))
  exact general events _ (initial_control before writes)
    (by simpa [project, initial, phase, Persistence.initial] using
      Persistence.Reachable.initial (before := before) (writes := writes))

theorem every_io_prefix_recovers_atomically (before : Image) (writes : List Change)
    (events : List Event) (markerPersisted : Bool) :
    recover before (writes.map Prod.fst) (project (run (initial before writes) events)) markerPersisted
      = before ∨
    recover before (writes.map Prod.fst) (project (run (initial before writes) events)) markerPersisted
      = applyWrites before writes :=
  crash_recovery_is_atomic before writes _
    (source_program_projects_reachable before writes events) markerPersisted

theorem successful_acknowledgement_is_durable (before : Image) (writes : List Change)
    (events : List Event) (done : (run (initial before writes) events).cursor = .committed)
    (markerPersisted : Bool) :
    recover before (writes.map Prod.fst) (project (run (initial before writes) events)) markerPersisted
      = applyWrites before writes := by
  exact acknowledged_deployment_survives_crash before writes _
    (source_program_projects_reachable before writes events)
    (by simp [project, phase, done]) markerPersisted

def BackupControl (s : Machine) : Prop := s.cursor ≠ .backup → s.backupTodo = []

def Backed (before : Image) (names : List Nat) (s : Machine) : Prop :=
  ∀ name ∈ names, name ∈ s.backupTodo ∨ s.journal name = some (before name)

theorem advance_preserves_backup_control (s : Machine) (h : BackupControl s) :
    BackupControl (advance s) := by
  cases hc : s.cursor <;> simp only [advance, hc]
  · cases hb : s.backupTodo <;> simp [BackupControl, hc]
  · simpa [BackupControl, hc] using h
  · simpa [BackupControl, hc] using h
  · simpa [BackupControl, hc] using h
  · cases ht : s.todo with
    | nil => simpa [BackupControl, hc] using h
    | cons change rest =>
      rcases change with ⟨name, value⟩
      cases value <;> simpa [BackupControl, hc] using h
  · simpa [BackupControl, hc] using h
  · simpa [BackupControl, hc] using h
  · cases ht : s.todo with
    | nil => exact h
    | cons change rest =>
      rcases change with ⟨name, value⟩
      cases value
      · exact h
      · simpa [BackupControl, hc] using h
  · simpa [BackupControl, hc] using h
  · simpa [BackupControl, hc] using h
  · exact h

theorem advance_preserves_backups (before : Image) (names : List Nat) (s : Machine)
    (h : Backed before names s)
    (original : phase s.cursor = .preparing → s.contents = before) :
    Backed before names (advance s) := by
  cases hc : s.cursor <;> simp only [advance, hc]
  · cases hb : s.backupTodo with
    | nil => simpa [Backed, hb] using h
    | cons key rest =>
      intro name member
      have old := h name member
      by_cases same : name = key
      · right
        simp [same, original (by simp [phase, hc])]
      · rcases old with pending | saved
        · left
          simpa [hb, same] using pending
        · right
          simpa [same] using saved
  · exact h
  · exact h
  · exact h
  · cases ht : s.todo with
    | nil => exact h
    | cons change rest =>
      rcases change with ⟨name, value⟩
      cases value <;> exact h
  · exact h
  · exact h
  · cases ht : s.todo with
    | nil => exact h
    | cons change rest =>
      rcases change with ⟨name, value⟩
      cases value <;> exact h
  · exact h
  · exact h
  · exact h

theorem step_preserves_backups (before : Image) (names : List Nat) (s : Machine)
    (event : Event) (h : Backed before names s) (control : BackupControl s)
    (original : phase s.cursor = .preparing → s.contents = before) :
    Backed before names (step s event) ∧ BackupControl (step s event) := by
  cases event with
  | ioReturn success =>
    change Backed before names (returns s success) ∧ BackupControl (returns s success)
    unfold returns
    split
    · exact ⟨h, control⟩
    · split
      · exact ⟨advance_preserves_backups before names s h original,
          advance_preserves_backup_control s control⟩
      · exact ⟨h, control⟩
  | persistName name =>
    by_cases hp : phase s.cursor = .writing <;>
      simpa [step, hp, Backed, BackupControl] using And.intro h control

theorem journal_covers_every_modified_name (before : Image) (writes : List Change)
    (events : List Event) :
    Backed before (writes.map Prod.fst) (run (initial before writes) events) ∧
      BackupControl (run (initial before writes) events) := by
  have general : ∀ events s, Control s → Backed before (writes.map Prod.fst) s →
      BackupControl s → Persistence.Reachable before writes (project s) →
      Backed before (writes.map Prod.fst) (run s events) ∧ BackupControl (run s events) := by
    intro es
    induction es with
    | nil => intro s control backed backup reachable; exact ⟨backed, backup⟩
    | cons event rest ih =>
      intro s control backed backup reachable
      have original : phase s.cursor = .preparing → s.contents = before := by
        intro hp
        exact Persistence.preparing_preserves_original_contents before writes _ reachable hp
      have next := step_preserves_backups before _ s event backed backup original
      exact ih _ (step_preserves_control s event control) next.1 next.2
        (.next reachable (step_simulates_model s event control))
  exact general events _ (initial_control before writes)
    (by intro name member; exact Or.inl member)
    (by simp [BackupControl, initial])
    (by simpa [project, initial, phase, Persistence.initial] using
      Persistence.Reachable.initial (before := before) (writes := writes))

theorem published_journal_contains_original_bytes (before : Image) (writes : List Change)
    (events : List Event) (name : Nat) (modified : name ∈ writes.map Prod.fst)
    (published : (run (initial before writes) events).cursor ≠ .backup) :
    (run (initial before writes) events).journal name = some (before name) := by
  have invariant := journal_covers_every_modified_name before writes events
  have complete := invariant.2 published
  have covered := invariant.1 name modified
  simpa [complete] using covered

theorem source_order_projects_reachable (before : Image) (writes : List Change)
    (backupOrder : List Nat) (events : List Event) :
    Persistence.Reachable before writes
      (project (run (initialWithBackupOrder before writes backupOrder) events)) := by
  have general : ∀ events s, Control s → Persistence.Reachable before writes (project s) →
      Persistence.Reachable before writes (project (run s events)) := by
    intro es
    induction es with
    | nil => intro s control reachable; exact reachable
    | cons event rest ih =>
      intro s control reachable
      exact ih _ (step_preserves_control s event control)
        (.next reachable (step_simulates_model s event control))
  exact general events _ (initial_control before writes)
    (by simpa [project, initialWithBackupOrder, initial, phase, Persistence.initial] using
      Persistence.Reachable.initial (before := before) (writes := writes))

theorem source_order_backups_cover_changes (before : Image) (writes : List Change)
    (backupOrder : List Nat) (covers : ∀ name ∈ writes.map Prod.fst, name ∈ backupOrder)
    (events : List Event) :
    Backed before (writes.map Prod.fst) (run (initialWithBackupOrder before writes backupOrder) events) ∧
      BackupControl (run (initialWithBackupOrder before writes backupOrder) events) := by
  have general : ∀ events s, Control s → Backed before (writes.map Prod.fst) s →
      BackupControl s → Persistence.Reachable before writes (project s) →
      Backed before (writes.map Prod.fst) (run s events) ∧ BackupControl (run s events) := by
    intro es
    induction es with
    | nil => intro s control backed backup reachable; exact ⟨backed, backup⟩
    | cons event rest ih =>
      intro s control backed backup reachable
      have original : phase s.cursor = .preparing → s.contents = before := by
        intro hp
        exact Persistence.preparing_preserves_original_contents before writes _ reachable hp
      have next := step_preserves_backups before _ s event backed backup original
      exact ih _ (step_preserves_control s event control) next.1 next.2
        (.next reachable (step_simulates_model s event control))
  exact general events _ (initial_control before writes)
    (by intro name member; exact Or.inl (covers name member))
    (by simp [BackupControl, initialWithBackupOrder, initial])
    (by simpa [project, initialWithBackupOrder, initial, phase, Persistence.initial] using
      Persistence.Reachable.initial (before := before) (writes := writes))

theorem source_order_crash_recovery_is_atomic (before : Image) (writes : List Change)
    (backupOrder : List Nat) (events : List Event) (markerPersisted : Bool) :
    recover before (writes.map Prod.fst)
      (project (run (initialWithBackupOrder before writes backupOrder) events)) markerPersisted = before ∨
    recover before (writes.map Prod.fst)
      (project (run (initialWithBackupOrder before writes backupOrder) events)) markerPersisted =
      applyWrites before writes :=
  crash_recovery_is_atomic before writes _
    (source_order_projects_reachable before writes backupOrder events) markerPersisted

theorem source_order_published_journal_is_complete (before : Image) (writes : List Change)
    (backupOrder : List Nat) (covers : ∀ name ∈ writes.map Prod.fst, name ∈ backupOrder)
    (events : List Event) (name : Nat) (modified : name ∈ writes.map Prod.fst)
    (published : (run (initialWithBackupOrder before writes backupOrder) events).cursor ≠ .backup) :
    (run (initialWithBackupOrder before writes backupOrder) events).journal name = some (before name) := by
  have invariant := source_order_backups_cover_changes before writes backupOrder covers events
  have complete := invariant.2 published
  have covered := invariant.1 name modified
  simpa [complete] using covered

def replayJournal (journal : Nat → Option (Option Nat)) : List Nat → Image → Option Image
  | [], image => some image
  | name :: rest, image => match journal name with
    | none => none
    | some bytes => replayJournal journal rest (put image name bytes)

theorem complete_journal_replays_original (before image : Image)
    (journal : Nat → Option (Option Nat)) (names : List Nat)
    (complete : ∀ name ∈ names, journal name = some (before name)) :
    replayJournal journal names image = some (restore before names image) := by
  induction names generalizing image with
  | nil => rfl
  | cons name rest ih =>
    simp only [replayJournal, complete name (by simp)]
    exact ih _ (fun other member => complete other (by simp [member]))

def actualRecovery (names : List Nat) (s : Machine) (markerPersisted : Bool) : Option Image :=
  match phase s.cursor with
  | .preparing | .acknowledged => some s.stable
  | .writing | .dataSynced => replayJournal s.journal names s.stable
  | .decisionMoved =>
    if markerPersisted then some s.stable else replayJournal s.journal names s.stable

theorem actual_recovery_uses_constructed_journal (before : Image) (writes : List Change)
    (backupOrder : List Nat) (covers : ∀ name ∈ writes.map Prod.fst, name ∈ backupOrder)
    (events : List Event) (markerPersisted : Bool) :
    let s := run (initialWithBackupOrder before writes backupOrder) events
    actualRecovery (writes.map Prod.fst) s markerPersisted =
      some (recover before (writes.map Prod.fst) (project s) markerPersisted) := by
  dsimp only
  let s := run (initialWithBackupOrder before writes backupOrder) events
  have complete (published : s.cursor ≠ .backup) :
      replayJournal s.journal (writes.map Prod.fst) s.stable =
        some (restore before (writes.map Prod.fst) s.stable) := by
    apply complete_journal_replays_original
    intro name member
    exact source_order_published_journal_is_complete before writes backupOrder covers events
      name member published
  change actualRecovery _ s markerPersisted = some (recover before _ (project s) markerPersisted)
  cases hc : s.cursor <;> cases markerPersisted <;>
    simp [actualRecovery, project, recover, phase, hc]
  all_goals exact complete (by simp [hc])

theorem complete_source_transaction_recovers_old_or_new (before : Image) (writes : List Change)
    (backupOrder : List Nat) (covers : ∀ name ∈ writes.map Prod.fst, name ∈ backupOrder)
    (events : List Event) (markerPersisted : Bool) :
    let s := run (initialWithBackupOrder before writes backupOrder) events
    actualRecovery (writes.map Prod.fst) s markerPersisted = some before ∨
      actualRecovery (writes.map Prod.fst) s markerPersisted = some (applyWrites before writes) := by
  dsimp only
  rw [actual_recovery_uses_constructed_journal before writes backupOrder covers events markerPersisted]
  rcases source_order_crash_recovery_is_atomic before writes backupOrder events markerPersisted with old | new
  · exact Or.inl (congrArg some old)
  · exact Or.inr (congrArg some new)

theorem failed_invocation_stops_io (s : Machine) (failed : s.failed = true) :
    nextIo s = none ∧ returns s true = s ∧ returns s false = s := by
  simp [nextIo, returns, failed]

theorem rename_requires_file_sync (s : Machine) (name : Nat)
    (rename : nextIo s = some (.renameFile name)) : s.cursor = .fileSynced := by
  cases hf : s.failed <;> simp [nextIo, hf] at rename
  cases hc : s.cursor <;> simp [hc] at rename
  · cases hb : s.backupTodo <;> simp_all
  · cases ht : s.todo with
    | nil => simp_all
    | cons change rest =>
      rcases change with ⟨key, value⟩
      cases value <;> simp_all
  · cases ht : s.todo with
    | nil => simp_all
    | cons change rest =>
      rcases change with ⟨key, value⟩
      cases value <;> simp_all
  · rfl

theorem decision_requires_data_sync (s : Machine)
    (decision : nextIo s = some .moveDecision) : s.cursor = .dataSynced := by
  cases hf : s.failed <;> simp [nextIo, hf] at decision
  cases hc : s.cursor <;> simp [hc] at decision
  · cases hb : s.backupTodo <;> simp_all
  · cases ht : s.todo with
    | nil => simp_all
    | cons change rest =>
      rcases change with ⟨key, value⟩
      cases value <;> simp_all
  · cases ht : s.todo with
    | nil => simp_all
    | cons change rest =>
      rcases change with ⟨key, value⟩
      cases value <;> simp_all
  · cases ht : s.todo with
    | nil => simp_all
    | cons change rest =>
      rcases change with ⟨key, value⟩
      cases value <;> simp_all
  · rfl

end Cordis.StorageProgram
