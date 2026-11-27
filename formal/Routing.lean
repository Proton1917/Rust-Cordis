import Std

/-!
Directory-owned export references, indexed removal, batch ownership and lifetime
retirement. A peer is one connection's ID namespace. `detached` counts references
moved out of the mutex-protected directory; `dropped` records their later drop.
External Arc clones are distinct references and are outside this ledger.

The batch field is a ghost inverse of the IDs held by each live `Batch`, not an
extra field in the Rust Entry. Capacity availability is a read of routes.len();
this model proves per-reference ownership and ID bounds, not HashMap complexity.
-/

namespace Cordis.Routing

structure Entry where
  owner : Option Nat := none
  lifetime : Option Nat := none
  batch : Option Nat := none
  deriving DecidableEq

def occupied : Option Entry → Nat
  | none => 0
  | some _ => 1

structure Peer where
  next : Nat := 1
  entries : Nat → Option Entry := fun _ => none
  detached : Nat → Nat := fun _ => 0
  dropped : Nat → Nat := fun _ => 0
  ended : Nat → Bool := fun _ => false
  alive : Bool := true

def allocated (next id : Nat) : Nat := if 0 < id ∧ id < next then 1 else 0

def referenceCount (s : Peer) (id : Nat) : Nat :=
  occupied (s.entries id) + s.detached id + s.dropped id

def PeerInvariant (s : Peer) : Prop :=
  0 < s.next ∧ ∀ id, referenceCount s id = allocated s.next id

theorem initial_peer_invariant : PeerInvariant {} := by
  constructor
  · decide
  · intro id
    simp only [referenceCount, occupied, Nat.zero_add, allocated]
    split <;> omega

def issue (s : Peer) (entry : Entry) : Peer :=
  { s with next := s.next + 1
           entries := fun id => if id = s.next then some entry else s.entries id }

def detach (s : Peer) (selected : Nat → Bool) : Peer :=
  { s with entries := fun id => if selected id then none else s.entries id
           detached := fun id => s.detached id + if selected id then occupied (s.entries id) else 0 }

def dropDetached (s : Peer) : Peer :=
  { s with detached := fun _ => 0
           dropped := fun id => s.dropped id + s.detached id }

def commit (s : Peer) (batch : Nat) : Peer :=
  { s with entries := fun id => (s.entries id).map fun entry =>
      if entry.batch = some batch then { entry with batch := none } else entry }

def belongsToBatch (s : Peer) (batch id : Nat) : Bool :=
  (s.entries id).any fun entry => entry.batch == some batch

def belongsToLifetime (s : Peer) (lifetime id : Nat) : Bool :=
  (s.entries id).any fun entry => entry.lifetime == some lifetime

def belongsToOwner (s : Peer) (owner id : Nat) : Bool :=
  (s.entries id).any fun entry => entry.owner == some owner

def generationAllowed (s : Peer) (entry : Entry) : Bool :=
  match entry.lifetime with
  | none => true
  | some lifetime => !s.ended lifetime

theorem issue_preserves (s : Peer) (entry : Entry) (h : PeerInvariant s) :
    PeerInvariant (issue s entry) := by
  refine ⟨by simp [issue], ?_⟩
  intro id
  have old := h.2 id
  by_cases fresh : id = s.next
  · subst id
    simp [referenceCount, allocated] at old
    simp [referenceCount, issue, occupied, allocated, h.1]
    omega
  · have bounds : (0 < id ∧ id < s.next + 1) ↔ (0 < id ∧ id < s.next) := by omega
    simpa [referenceCount, issue, fresh, allocated, bounds] using old

theorem detach_preserves_count (s : Peer) (selected : Nat → Bool) (id : Nat) :
    referenceCount (detach s selected) id = referenceCount s id := by
  cases hs : selected id <;> simp [referenceCount, detach, hs, occupied] <;> omega

theorem detach_preserves (s : Peer) (selected : Nat → Bool) (h : PeerInvariant s) :
    PeerInvariant (detach s selected) := by
  exact ⟨h.1, fun id => (detach_preserves_count s selected id).trans (h.2 id)⟩

theorem drop_preserves (s : Peer) (h : PeerInvariant s) : PeerInvariant (dropDetached s) := by
  refine ⟨h.1, ?_⟩
  intro id
  have old := h.2 id
  simpa [referenceCount, dropDetached, Nat.add_assoc, Nat.add_comm, Nat.add_left_comm] using old

theorem commit_preserves_count (s : Peer) (batch id : Nat) :
    referenceCount (commit s batch) id = referenceCount s id := by
  cases he : s.entries id with
  | none => simp [referenceCount, commit, he, occupied]
  | some entry =>
    by_cases hb : entry.batch = some batch <;>
      simp [referenceCount, commit, he, hb, occupied]

theorem commit_preserves (s : Peer) (batch : Nat) (h : PeerInvariant s) :
    PeerInvariant (commit s batch) :=
  ⟨h.1, fun id => (commit_preserves_count s batch id).trans (h.2 id)⟩

inductive Event where
  | exportRef (entry : Entry) (capacityAvailable : Bool)
  | release (ids : List Nat)
  | commitBatch (batch : Nat)
  | rollbackBatch (batch : Nat)
  | markRetired (lifetime : Nat)
  | sweepRetired (lifetime : Nat)
  | reclaimOwner (owner : Nat)
  | closeRecipient
  | destroyDetached
  deriving DecidableEq

def step (maxId : Nat) (s : Peer) : Event → Peer
  | .exportRef entry capacity =>
    if s.alive = true ∧ capacity = true ∧ generationAllowed s entry = true ∧ s.next < maxId
    then issue s entry else s
  | .release ids =>
    if ids.all (fun id => decide (0 < id ∧ id < s.next)) then
      detach s (fun id => ids.contains id)
    else s
  | .commitBatch batch => commit s batch
  | .rollbackBatch batch => detach s (belongsToBatch s batch)
  | .markRetired lifetime =>
    { s with ended := fun other => if other = lifetime then true else s.ended other }
  | .sweepRetired lifetime =>
    if s.ended lifetime then detach s (belongsToLifetime s lifetime) else s
  | .reclaimOwner owner => detach s (belongsToOwner s owner)
  | .closeRecipient => { (detach s (fun _ => true)) with alive := false }
  | .destroyDetached => dropDetached s

theorem step_preserves (maxId : Nat) (s : Peer) (event : Event) (h : PeerInvariant s) :
    PeerInvariant (step maxId s event) := by
  cases event with
  | exportRef entry capacity =>
    simp only [step]
    split
    · exact issue_preserves s entry h
    · exact h
  | release ids =>
    simp only [step]
    split
    · exact detach_preserves s _ h
    · exact h
  | commitBatch batch => exact commit_preserves s batch h
  | rollbackBatch batch => exact detach_preserves s _ h
  | markRetired lifetime => exact h
  | sweepRetired lifetime =>
    simp only [step]
    split
    · exact detach_preserves s _ h
    · exact h
  | reclaimOwner owner => exact detach_preserves s _ h
  | closeRecipient => exact detach_preserves s (fun _ => true) h
  | destroyDetached => exact drop_preserves s h

def run (maxId : Nat) (s : Peer) : List Event → Peer
  | [] => s
  | event :: rest => run maxId (step maxId s event) rest

theorem trace_preserves (maxId : Nat) (s : Peer) (events : List Event) (h : PeerInvariant s) :
    PeerInvariant (run maxId s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (step_preserves maxId s event h)

/-- Includes duplicate releases, batch rollback, owner reclaim and generation sweep. -/
theorem each_export_reference_returned_at_most_once (maxId : Nat) (events : List Event)
    (id : Nat) : (run maxId {} events).dropped id ≤ 1 := by
  have h := (trace_preserves maxId {} events initial_peer_invariant).2 id
  simp only [referenceCount, allocated] at h
  split at h <;> omega

theorem live_reference_is_owned_exclusively (maxId : Nat) (events : List Event)
    (id : Nat) (entry : Entry) (live : (run maxId {} events).entries id = some entry) :
    (run maxId {} events).detached id = 0 ∧ (run maxId {} events).dropped id = 0 := by
  have h := (trace_preserves maxId {} events initial_peer_invariant).2 id
  simp only [referenceCount, live, occupied, allocated] at h
  split at h <;> omega

theorem unallocated_reference_absent (maxId : Nat) (events : List Event) (id : Nat)
    (outside : id = 0 ∨ (run maxId {} events).next ≤ id) :
    (run maxId {} events).entries id = none := by
  have h := (trace_preserves maxId {} events initial_peer_invariant).2 id
  simp only [referenceCount, allocated] at h
  cases he : (run maxId {} events).entries id with
  | none => rfl
  | some entry =>
    simp only [he, occupied] at h
    split at h <;> omega

/-- Validation happens before any removal, including for a mixed valid/invalid batch. -/
theorem invalid_release_is_atomic (maxId : Nat) (s : Peer) (ids : List Nat)
    (bad : ∃ id ∈ ids, id = 0 ∨ s.next ≤ id) : step maxId s (.release ids) = s := by
  have invalid : ids.all (fun id => decide (0 < id ∧ id < s.next)) = false := by
    apply Bool.eq_false_iff.mpr
    intro all
    rcases bad with ⟨id, member, outside⟩
    have valid := (List.all_eq_true.mp all) id member
    simp only [decide_eq_true_eq] at valid
    omega
  simp only [step, invalid, Bool.false_eq_true, if_false]

theorem detach_is_idempotent (s : Peer) (selected : Nat → Bool) :
    detach (detach s selected) selected = detach s selected := by
  cases s with
  | mk next entries detached dropped ended alive =>
    simp only [detach, Peer.mk.injEq, and_true, true_and]
    constructor
    · funext id
      cases hs : selected id <;> simp [hs]
    · funext id
      cases hs : selected id <;> simp [hs, occupied]

theorem repeated_release_is_idempotent (maxId : Nat) (s : Peer) (ids : List Nat) :
    step maxId (step maxId s (.release ids)) (.release ids) = step maxId s (.release ids) := by
  simp only [step]
  split
  · simp only [detach]
    split
    · exact detach_is_idempotent s _
    · contradiction
  · rfl

theorem detach_defers_destructors (s : Peer) (selected : Nat → Bool) :
    (detach s selected).dropped = s.dropped := rfl

theorem committed_batch_survives_rollback (s : Peer) (batch : Nat) :
    detach (commit s batch) (belongsToBatch (commit s batch) batch) = commit s batch := by
  have noneSelected : belongsToBatch (commit s batch) batch = fun _ => false := by
    funext id
    cases he : s.entries id with
    | none => simp [belongsToBatch, commit, he]
    | some entry =>
      by_cases hb : entry.batch = some batch <;>
        simp [belongsToBatch, commit, he, hb]
  rw [noneSelected]
  cases commit s batch
  rfl

theorem rollback_removes_batch_exports (s : Peer) (batch id : Nat) (entry : Entry)
    (owned : s.entries id = some entry) (pending : entry.batch = some batch) :
    (detach s (belongsToBatch s batch)).entries id = none := by
  simp [detach, belongsToBatch, owned, pending]

theorem retired_generation_denies_new_exports (maxId : Nat) (s : Peer) (entry : Entry)
    (lifetime : Nat) (ended : s.ended lifetime = true) (owned : entry.lifetime = some lifetime)
    (capacity : Bool) : step maxId s (.exportRef entry capacity) = s := by
  simp [step, generationAllowed, owned, ended]

theorem retirement_sweep_removes_generation (maxId : Nat) (s : Peer)
    (lifetime id : Nat) (entry : Entry) (ended : s.ended lifetime = true)
    (owned : s.entries id = some entry) (generation : entry.lifetime = some lifetime) :
    (step maxId s (.sweepRetired lifetime)).entries id = none := by
  simp [step, ended, detach, belongsToLifetime, owned, generation]

theorem exhausted_id_space_denies_export (maxId : Nat) (s : Peer) (entry : Entry)
    (capacity : Bool) (full : maxId ≤ s.next) : step maxId s (.exportRef entry capacity) = s := by
  have hn : ¬ s.next < maxId := by omega
  simp [step, hn]

theorem next_is_monotone (maxId : Nat) (s : Peer) (event : Event) :
    s.next ≤ (step maxId s event).next := by
  cases event <;> simp only [step]
  all_goals repeat first | split
  all_goals simp [issue, detach, commit, dropDetached]

/-- The historical frontier is retained as ghost state after connection close.
The concrete directory may remove the Recipient then; that connection stays dead. -/
theorem closed_connection_cannot_export (maxId : Nat) (s : Peer) (entry : Entry)
    (capacity : Bool) (closed : s.alive = false) : step maxId s (.exportRef entry capacity) = s := by
  simp [step, closed]

def Directory := Nat → Peer

def updateRecipient (directory : Directory) (recipient : Nat) (peer : Peer) : Directory :=
  fun other => if other = recipient then peer else directory other

theorem recipient_namespace_isolation (directory : Directory) (recipient other : Nat)
    (peer : Peer) (different : other ≠ recipient) :
    updateRecipient directory recipient peer other = directory other := by
  simp [updateRecipient, different]

/-! Concrete index views. Rust HashSets encode these Boolean memberships.
The unlink implementation masks the selected key in both indexes while retaining
all other memberships; insert writes the metadata belonging to the new key. -/

structure IndexedPeer where
  peer : Peer
  owners : Nat → Nat → Bool
  lifetimes : Nat → Nat → Bool

def IndexInvariant (s : IndexedPeer) : Prop :=
  (∀ owner id, s.owners owner id = belongsToOwner s.peer owner id) ∧
  (∀ lifetime id, s.lifetimes lifetime id = belongsToLifetime s.peer lifetime id)

def indexedDetach (s : IndexedPeer) (selected : Nat → Bool) : IndexedPeer :=
  { peer := detach s.peer selected
    owners := fun owner id => if selected id then false else s.owners owner id
    lifetimes := fun lifetime id => if selected id then false else s.lifetimes lifetime id }

def indexedIssue (s : IndexedPeer) (entry : Entry) : IndexedPeer :=
  { peer := issue s.peer entry
    owners := fun owner id => if id = s.peer.next then entry.owner == some owner else s.owners owner id
    lifetimes := fun lifetime id =>
      if id = s.peer.next then entry.lifetime == some lifetime else s.lifetimes lifetime id }

theorem indexed_detach_preserves (s : IndexedPeer) (selected : Nat → Bool)
    (h : IndexInvariant s) : IndexInvariant (indexedDetach s selected) := by
  constructor
  · intro owner id
    have old := h.1 owner id
    cases hs : selected id <;> simp_all [indexedDetach, belongsToOwner, detach]
  · intro lifetime id
    have old := h.2 lifetime id
    cases hs : selected id <;> simp_all [indexedDetach, belongsToLifetime, detach]

theorem indexed_issue_preserves (s : IndexedPeer) (entry : Entry)
    (h : IndexInvariant s) : IndexInvariant (indexedIssue s entry) := by
  constructor
  · intro owner id
    have old := h.1 owner id
    by_cases fresh : id = s.peer.next <;>
      simp_all [indexedIssue, belongsToOwner, issue]
  · intro lifetime id
    have old := h.2 lifetime id
    by_cases fresh : id = s.peer.next <;>
      simp_all [indexedIssue, belongsToLifetime, issue]

inductive IndexTrace : IndexedPeer → IndexedPeer → Prop where
  | refl (s) : IndexTrace s s
  | insert {s t} (entry) : IndexTrace s t → IndexTrace s (indexedIssue t entry)
  | unlink {s t} (selected) : IndexTrace s t → IndexTrace s (indexedDetach t selected)

theorem indexed_trace_preserves (s t : IndexedPeer) (trace : IndexTrace s t)
    (h : IndexInvariant s) : IndexInvariant t := by
  induction trace with
  | refl => exact h
  | insert entry trace ih => exact indexed_issue_preserves _ entry ih
  | unlink selected trace ih => exact indexed_detach_preserves _ selected ih

theorem indexed_trace_preserves_ownership (s t : IndexedPeer) (trace : IndexTrace s t)
    (h : PeerInvariant s.peer) : PeerInvariant t.peer := by
  induction trace with
  | refl => exact h
  | insert entry trace ih => exact issue_preserves _ entry ih
  | unlink selected trace ih => exact detach_preserves _ selected ih

end Cordis.Routing
