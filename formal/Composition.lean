import Architecture
import Protocol
import Routing
import Execution
import StorageProgram

/-!
Composition of the actual outgoing-batch boundary: first-write ownership is
claimed under RequestWrite's lock; invoking or dropping its FnOnce callback
later commits or rolls back Routing::Batch. The callback interval is explicit.
Global traces interleave these coupled transitions with independent authority
updates, per-resource events, cleanup and storage operations.
-/

namespace Cordis.Composition

structure Transfer where
  wire : Protocol.RequestWrite := {}
  peer : Routing.Peer := {}
  pending : Protocol.Pending := { ownsDeadline := true }
  commitDue : Nat := 0
  rollbackDue : Nat := 0
  committed : Nat := 0
  rolledBack : Nat := 0

def TransferInvariant (s : Transfer) : Prop :=
  Protocol.WriteInvariant s.wire ∧ Routing.PeerInvariant s.peer ∧
    Protocol.PendingInvariant s.pending ∧
    s.committed + s.commitDue = s.wire.granted ∧
    s.rolledBack + s.rollbackDue = s.wire.rolledBack

theorem initial_transfer_invariant : TransferInvariant {} :=
  ⟨Protocol.write_initial, Routing.initial_peer_invariant,
    Protocol.pending_initial true 0, rfl, rfl⟩

def handoff (s : Transfer) (event : Protocol.WriteEvent) : Transfer :=
  let next := Protocol.writeStep s.wire event
  { s with wire := next
           commitDue := s.commitDue + (next.granted - s.wire.granted)
           rollbackDue := s.rollbackDue + (next.rolledBack - s.wire.rolledBack) }

theorem write_claims_monotone (s : Protocol.RequestWrite) (event : Protocol.WriteEvent) :
    s.granted ≤ (Protocol.writeStep s event).granted ∧
      s.rolledBack ≤ (Protocol.writeStep s event).rolledBack := by
  cases event <;> cases hs : s.started <;> cases hc : s.cancelled <;>
    simp [Protocol.writeStep, hs, hc]

theorem handoff_preserves (s : Transfer) (event : Protocol.WriteEvent)
    (h : TransferInvariant s) : TransferInvariant (handoff s event) := by
  rcases h with ⟨wire, peer, pending, commit, rollback⟩
  have monotone := write_claims_monotone s.wire event
  refine ⟨Protocol.write_preserves s.wire event wire, peer, pending, ?_, ?_⟩
  · simp only [handoff]
    omega
  · simp only [handoff]
    omega

inductive TransferEvent where
  | prepare (entry : Routing.Entry) (capacityAvailable : Bool)
  | write (event : Protocol.WriteEvent)
  | terminal (event : Protocol.Terminal)
  | invokeCommit | dropRollback | destroyDetached
  | release (ids : List Nat)
  | retire (generation : Nat)
  | sweep (generation : Nat)
  | close
  deriving DecidableEq

def transferStep (maxId batch : Nat) (s : Transfer) : TransferEvent → Transfer
  | .prepare entry capacity =>
    if s.wire.token && s.pending.present then
      { s with peer := (Routing.step maxId s.peer
          (.exportRef {entry with batch := some batch} capacity)) }
    else s
  | .write event => handoff s event
  | .terminal event => { s with pending := Protocol.claim s.pending event }
  | .invokeCommit =>
    if 0 < s.commitDue then
      { s with peer := Routing.commit s.peer batch,
               commitDue := s.commitDue - 1, committed := s.committed + 1 }
    else s
  | .dropRollback =>
    if 0 < s.rollbackDue then
      { s with peer := Routing.detach s.peer (Routing.belongsToBatch s.peer batch),
               rollbackDue := s.rollbackDue - 1, rolledBack := s.rolledBack + 1 }
    else s
  | .destroyDetached => { s with peer := Routing.dropDetached s.peer }
  | .release ids => { s with peer := Routing.step maxId s.peer (.release ids) }
  | .retire generation => { s with peer := Routing.step maxId s.peer (.markRetired generation) }
  | .sweep generation => { s with peer := Routing.step maxId s.peer (.sweepRetired generation) }
  | .close => { (handoff s .cancel) with
      peer := Routing.step maxId s.peer .closeRecipient
      pending := Protocol.claim s.pending .disconnect }

theorem transfer_step_preserves (maxId batch : Nat) (s : Transfer) (event : TransferEvent)
    (h : TransferInvariant s) : TransferInvariant (transferStep maxId batch s event) := by
  rcases h with ⟨wire, peer, pending, commit, rollback⟩
  cases event with
  | prepare entry capacity =>
    simp only [transferStep]
    split
    · exact ⟨wire, Routing.step_preserves _ _ _ peer, pending, commit, rollback⟩
    · exact ⟨wire, peer, pending, commit, rollback⟩
  | write event => exact handoff_preserves s event ⟨wire, peer, pending, commit, rollback⟩
  | terminal event => exact ⟨wire, peer, Protocol.claim_preserves _ _ pending, commit, rollback⟩
  | invokeCommit =>
    simp only [transferStep]
    split
    · rename_i positive
      refine ⟨wire, Routing.commit_preserves _ batch peer, pending, ?_, rollback⟩
      simp only
      omega
    · exact ⟨wire, peer, pending, commit, rollback⟩
  | dropRollback =>
    simp only [transferStep]
    split
    · rename_i positive
      refine ⟨wire, Routing.detach_preserves _ _ peer, pending, commit, ?_⟩
      simp only
      omega
    · exact ⟨wire, peer, pending, commit, rollback⟩
  | destroyDetached => exact ⟨wire, Routing.drop_preserves _ peer, pending, commit, rollback⟩
  | release ids => exact ⟨wire, Routing.step_preserves _ _ _ peer, pending, commit, rollback⟩
  | retire generation => exact ⟨wire, Routing.step_preserves _ _ _ peer, pending, commit, rollback⟩
  | sweep generation => exact ⟨wire, Routing.step_preserves _ _ _ peer, pending, commit, rollback⟩
  | close =>
    have next := handoff_preserves s .cancel ⟨wire, peer, pending, commit, rollback⟩
    exact ⟨next.1, Routing.step_preserves _ _ _ peer,
      Protocol.claim_preserves _ _ pending, next.2.2.2.1, next.2.2.2.2⟩

def transferRun (maxId batch : Nat) (s : Transfer) : List TransferEvent → Transfer
  | [] => s
  | event :: rest => transferRun maxId batch (transferStep maxId batch s event) rest

theorem transfer_trace_preserves (maxId batch : Nat) (s : Transfer)
    (events : List TransferEvent) (h : TransferInvariant s) :
    TransferInvariant (transferRun maxId batch s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (transfer_step_preserves maxId batch s event h)

theorem callbacks_have_one_original_owner (s : Transfer) (h : TransferInvariant s) :
    s.committed + s.commitDue + s.rolledBack + s.rollbackDue + Protocol.bit s.wire.token = 1 := by
  have count := h.1.1
  have commit := h.2.2.2.1
  have rollback := h.2.2.2.2
  omega

theorem commit_and_rollback_are_exclusive (maxId batch : Nat) (events : List TransferEvent) :
    (transferRun maxId batch {} events).committed +
      (transferRun maxId batch {} events).rolledBack ≤ 1 := by
  have h := callbacks_have_one_original_owner _
    (transfer_trace_preserves maxId batch {} events initial_transfer_invariant)
  omega

theorem queued_commit_survives_cancellation (s : Transfer) (started : s.wire.started = true) :
    (handoff s .cancel).commitDue = s.commitDue ∧
      (handoff s .cancel).rollbackDue = s.rollbackDue := by
  simp [handoff, Protocol.writeStep, started]

theorem batch_commit_preserves_every_reference (s : Transfer) (maxId batch id : Nat) :
    Routing.referenceCount (transferStep maxId batch s .invokeCommit).peer id =
      Routing.referenceCount s.peer id := by
  simp only [transferStep]
  split
  · exact Routing.commit_preserves_count s.peer batch id
  · rfl

theorem batch_rollback_defers_destructors (s : Transfer) (maxId batch id : Nat) :
    (transferStep maxId batch s .dropRollback).peer.dropped id = s.peer.dropped id := by
  simp only [transferStep]
  split <;> rfl

theorem preparation_records_the_callback_batch (s : Transfer) (maxId batch : Nat)
    (entry : Routing.Entry) (token : s.wire.token = true) (pending : s.pending.present = true)
    (alive : s.peer.alive = true)
    (generation : Routing.generationAllowed s.peer {entry with batch := some batch} = true)
    (room : s.peer.next < maxId) :
    (transferStep maxId batch s (.prepare entry true)).peer.entries s.peer.next =
      some {entry with batch := some batch} := by
  simp [transferStep, token, pending, Routing.step, alive, generation, room, Routing.issue]

theorem preparation_after_handoff_is_rejected (s : Transfer) (maxId batch : Nat)
    (entry : Routing.Entry) (capacity : Bool) (claimed : s.wire.token = false) :
    transferStep maxId batch s (.prepare entry capacity) = s := by
  simp [transferStep, claimed]

theorem rollback_moves_prepared_reference_out_of_lock (s : Transfer)
    (maxId batch id : Nat) (entry : Routing.Entry) (due : 0 < s.rollbackDue)
    (present : s.peer.entries id = some entry) (prepared : entry.batch = some batch) :
    (transferStep maxId batch s .dropRollback).peer.entries id = none ∧
      (transferStep maxId batch s .dropRollback).peer.detached id = s.peer.detached id + 1 ∧
      (transferStep maxId batch s .dropRollback).peer.dropped id = s.peer.dropped id := by
  simp [transferStep, due, Routing.detach, Routing.belongsToBatch, present, prepared,
    Routing.occupied]

theorem each_prepared_reference_has_one_owner (maxId batch : Nat)
    (events : List TransferEvent) (id : Nat) :
    Routing.referenceCount (transferRun maxId batch {} events).peer id =
      Routing.allocated (transferRun maxId batch {} events).peer.next id :=
  (transfer_trace_preserves maxId batch {} events initial_transfer_invariant).2.1.2 id

theorem reachable_nonempty_batch_commits (maxId batch owner : Nat) (room : 1 < maxId) :
    let s := transferRun maxId batch {}
      [.prepare ⟨some owner, none, none⟩ true, .write .positiveWrite, .invokeCommit]
    s.peer.entries 1 = some ⟨some owner, none, none⟩ ∧
      s.committed = 1 ∧ Routing.referenceCount s.peer 1 = 1 := by
  simp [transferRun, transferStep, handoff, Protocol.writeStep, Protocol.bit,
    Routing.step, Routing.generationAllowed, Routing.issue, Routing.commit,
    Routing.referenceCount, Routing.occupied, room]

theorem reachable_nonempty_batch_rolls_back (maxId batch owner : Nat) (room : 1 < maxId) :
    let s := transferRun maxId batch {}
      [.prepare ⟨some owner, none, none⟩ true, .write .cancel, .dropRollback, .destroyDetached]
    s.peer.entries 1 = none ∧ s.rolledBack = 1 ∧ s.peer.dropped 1 = 1 := by
  simp [transferRun, transferStep, handoff, Protocol.writeStep, Protocol.bit,
    Routing.step, Routing.generationAllowed, Routing.issue, Routing.detach,
    Routing.belongsToBatch, Routing.dropDetached, Routing.occupied, room]

/-! Top-level transition system. All subsystems share an event trace, and the
wire/directory handoff above changes two subsystems in one logical boundary.
Fact publication, each resource id and storage have explicit frame conditions.
Cleanup completion releases the same owner resource after its LIFO runner
has consumed all work; observing or cancelling its waiter never consumes it. -/

structure System where
  facts : Architecture.Facts := Architecture.initialFacts
  resources : Architecture.Resources := fun _ => {}
  transfer : Transfer := {}
  cleanup : Execution.CleanupState Nat
  cleanupResource : Nat := 0
  cleanupOwner : Architecture.Owner := .kernel
  storage : StorageProgram.Machine

def initial (before : Persistence.Image) (writes : List Persistence.Change)
    (cleanup : List Nat) : System :=
  { cleanup := { pending := cleanup }, storage := StorageProgram.initial before writes }

def SystemInvariant (before : Persistence.Image) (writes : List Persistence.Change)
    (original : List Nat) (s : System) : Prop :=
  Architecture.Provenance s.facts ∧ Architecture.SnapshotValid s.facts ∧
  Architecture.AllConserved s.resources ∧ TransferInvariant s.transfer ∧
  Execution.CleanupInvariant original s.cleanup ∧ StorageProgram.Control s.storage ∧
  Persistence.Reachable before writes (StorageProgram.project s.storage)

inductive Event where
  | fact (event : Architecture.FactEvent)
  | resource (id : Nat) (event : Architecture.ResourceEvent)
  | transfer (event : TransferEvent)
  | cleanup (event : Execution.CleanupEvent)
  | releaseCleaned (id : Nat) (owner : Architecture.Owner)
  | storage (event : StorageProgram.Event)

def step (maxId batch : Nat) (s : System) : Event → System
  | .fact event => { s with facts := Architecture.factStep s.facts event }
  | .resource id event =>
    match event with
    | .completeCleanup _ => s
    | _ => { s with resources := Architecture.updateResource s.resources id event }
  | .transfer event => { s with transfer := transferStep maxId batch s.transfer event }
  | .cleanup event => { s with cleanup := Execution.cleanupStep s.cleanup event }
  | .releaseCleaned id owner =>
    if s.cleanup.terminal = true ∧ id = s.cleanupResource ∧ owner = s.cleanupOwner then
      { s with resources := Architecture.updateResource s.resources id (.completeCleanup owner) }
    else s
  | .storage event => { s with storage := StorageProgram.step s.storage event }

theorem initial_system_invariant (before : Persistence.Image) (writes : List Persistence.Change)
    (cleanup : List Nat) : SystemInvariant before writes cleanup (initial before writes cleanup) := by
  refine ⟨?_, ?_, ?_, initial_transfer_invariant, Execution.initial_cleanup_invariant _,
    StorageProgram.initial_control _ _, ?_⟩
  · simp [initial, Architecture.Provenance, Architecture.initialFacts]
  · simp [initial, Architecture.SnapshotValid, Architecture.initialFacts]
  · intro id
    simp [initial, Architecture.Conserved, Architecture.live]
  · exact StorageProgram.source_program_projects_reachable before writes []

theorem system_step_preserves (before : Persistence.Image) (writes : List Persistence.Change)
    (original : List Nat) (maxId batch : Nat) (s : System) (event : Event)
    (h : SystemInvariant before writes original s) :
    SystemInvariant before writes original (step maxId batch s event) := by
  rcases h with ⟨provenance, snapshot, resources, transfer, cleanup, control, storage⟩
  cases event with
  | fact event =>
    refine ⟨?_, ?_, resources, transfer, cleanup, control, storage⟩
    · exact Architecture.fact_trace_provenance s.facts [event] provenance
    · exact Architecture.fact_trace_preserves_snapshots s.facts [event] snapshot
  | resource id event =>
    cases event
    all_goals first
      | exact ⟨provenance, snapshot, Architecture.update_resource_conserves_all _ _ _ resources,
          transfer, cleanup, control, storage⟩
      | exact ⟨provenance, snapshot, resources, transfer, cleanup, control, storage⟩
  | transfer event =>
    exact ⟨provenance, snapshot, resources, transfer_step_preserves _ _ _ _ transfer,
      cleanup, control, storage⟩
  | cleanup event =>
    exact ⟨provenance, snapshot, resources, transfer,
      Execution.cleanup_step_preserves _ _ _ cleanup, control, storage⟩
  | releaseCleaned id owner =>
    simp only [step]
    split
    · exact ⟨provenance, snapshot, Architecture.update_resource_conserves_all _ _ _ resources,
        transfer, cleanup, control, storage⟩
    · exact ⟨provenance, snapshot, resources, transfer, cleanup, control, storage⟩
  | storage event =>
    exact ⟨provenance, snapshot, resources, transfer, cleanup,
      StorageProgram.step_preserves_control _ _ control,
      .next storage (StorageProgram.step_simulates_model _ _ control)⟩

def run (maxId batch : Nat) (s : System) : List Event → System
  | [] => s
  | event :: rest => run maxId batch (step maxId batch s event) rest

theorem global_trace_preserves (before : Persistence.Image) (writes : List Persistence.Change)
    (original : List Nat) (maxId batch : Nat) (s : System) (events : List Event)
    (h : SystemInvariant before writes original s) :
    SystemInvariant before writes original (run maxId batch s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (system_step_preserves before writes original _ _ _ _ h)

theorem all_reachable_systems_satisfy_contracts (before : Persistence.Image)
    (writes : List Persistence.Change) (cleanup : List Nat) (maxId batch : Nat)
    (events : List Event) :
    SystemInvariant before writes cleanup (run maxId batch (initial before writes cleanup) events) :=
  global_trace_preserves before writes cleanup _ _ _ _ (initial_system_invariant _ _ _)

def storageEvent : Event → Option StorageProgram.Event
  | .storage event => some event
  | _ => none

theorem global_storage_trace_projection (maxId batch : Nat) (s : System)
    (events : List Event) :
    (run maxId batch s events).storage =
      StorageProgram.run s.storage (events.filterMap storageEvent) := by
  induction events generalizing s with
  | nil => rfl
  | cons event rest ih =>
    rw [run, ih]
    cases event with
    | fact event => rfl
    | resource id event => cases event <;> rfl
    | transfer event => rfl
    | cleanup event => rfl
    | releaseCleaned id owner =>
      simp only [List.filterMap_cons, storageEvent]
      by_cases done : s.cleanup.terminal = true ∧ id = s.cleanupResource ∧ owner = s.cleanupOwner
      · simp [step, done]
      · simp [step, done]
    | storage event => rfl

theorem global_crash_recovery_is_atomic (before : Persistence.Image)
    (writes : List Persistence.Change) (cleanup : List Nat) (maxId batch : Nat)
    (events : List Event) (markerPersisted : Bool) :
    let s := run maxId batch (initial before writes cleanup) events
    StorageProgram.actualRecovery (writes.map Prod.fst) s.storage markerPersisted = some before ∨
    StorageProgram.actualRecovery (writes.map Prod.fst) s.storage markerPersisted =
      some (Persistence.applyWrites before writes) := by
  dsimp only
  rw [global_storage_trace_projection]
  simpa [initial, StorageProgram.initialWithBackupOrder] using
    StorageProgram.complete_source_transaction_recovers_old_or_new before writes
      (writes.map Prod.fst) (fun _ member => member) (events.filterMap storageEvent) markerPersisted

theorem cleanup_release_requires_finished_work (s : System) (maxId batch id : Nat)
    (owner : Architecture.Owner) (waiting : s.cleanup.terminal = false) :
    step maxId batch s (.releaseCleaned id owner) = s := by
  simp [step, waiting]

theorem cleanup_release_keeps_resource_identity (s : System) (maxId batch id : Nat)
    (owner : Architecture.Owner) (other : id ≠ s.cleanupResource ∨ owner ≠ s.cleanupOwner) :
    step maxId batch s (.releaseCleaned id owner) = s := by
  rcases other with other | other <;> simp [step, other]

theorem raw_resource_event_cannot_release (s : System) (maxId batch id : Nat)
    (owner : Architecture.Owner) :
    step maxId batch s (.resource id (.completeCleanup owner)) = s := rfl

end Cordis.Composition
