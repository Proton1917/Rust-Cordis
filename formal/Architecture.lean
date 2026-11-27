import Std

/-!
The four ownership rules as an executable contract calculus. Facts have a
designated authority; snapshots carry the authority's revision; a connection
has an authenticated principal supplied by the runtime, separately from
untrusted payload data. Resources move through owned, in-flight, cleanup and
returned states. These definitions specify the architecture contracts; their
application to concrete subsystem transitions is in Composition.lean.
-/

namespace Cordis.Architecture

inductive Owner where
  | kernel | runtime | supervisor | host | routing | transport | sdk | web
  | plugin (instanceId : Nat)
  deriving DecidableEq

inductive Fact where
  | generation (instanceId : Nat)
  | bindings (instanceId : Nat)
  | deployment | durableDecision
  | connection (id : Nat)
  | nativeImage (id : Nat)
  | route (connection object : Nat)
  | request (connection id : Nat)
  | sdkRequest (id : Nat)
  | httpRegistration (id : Nat)
  | business (instanceId key : Nat)
  deriving DecidableEq

def authority : Fact → Owner
  | .generation _ | .bindings _ => .kernel
  | .deployment | .durableDecision => .runtime
  | .connection _ => .supervisor
  | .nativeImage _ => .host
  | .route _ _ => .routing
  | .request _ _ => .transport
  | .sdkRequest _ => .sdk
  | .httpRegistration _ => .web
  | .business instanceId _ => .plugin instanceId

structure Version where
  interface : Nat
  major : Nat
  minor : Nat
  deriving DecidableEq

def Compatible (offered required : Version) : Prop :=
  offered.interface = required.interface ∧ offered.major = required.major ∧
    required.minor ≤ offered.minor

instance (offered required : Version) : Decidable (Compatible offered required) :=
  inferInstanceAs (Decidable (_ ∧ _ ∧ _))

theorem compatible_reflexive (v : Version) : Compatible v v := ⟨rfl, rfl, Nat.le_refl _⟩

theorem compatible_transitive (a b c : Version)
    (ab : Compatible a b) (bc : Compatible b c) : Compatible a c :=
  ⟨ab.1.trans bc.1, ab.2.1.trans bc.2.1, Nat.le_trans bc.2.2 ab.2.2⟩

structure Snapshot where
  fact : Fact
  revision : Nat
  value : Nat
  issuer : Owner
  offered : Version
  required : Version
  deriving DecidableEq

structure Facts where
  value : Fact → Nat
  revision : Fact → Nat
  delivered : List Snapshot := []

def initialFacts : Facts := ⟨fun _ => 0, fun _ => 0, []⟩

def writeFact (s : Facts) (actor : Owner) (fact : Fact) (value : Nat) : Facts :=
  if actor = authority fact then
    { s with value := fun other => if other = fact then value else s.value other
             revision := fun other => if other = fact then s.revision other + 1
               else s.revision other }
  else s

def publishFact (s : Facts) (actor : Owner) (fact : Fact)
    (offered required : Version) : Facts :=
  if actor = authority fact ∧ Compatible offered required then
    { s with delivered := s.delivered ++
      [⟨fact, s.revision fact, s.value fact, actor, offered, required⟩] }
  else s

def Provenance (s : Facts) : Prop :=
  ∀ snapshot ∈ s.delivered,
    snapshot.issuer = authority snapshot.fact ∧ Compatible snapshot.offered snapshot.required

def currentSnapshot (s : Facts) (snapshot : Snapshot) : Bool :=
  decide (snapshot ∈ s.delivered ∧ snapshot.revision = s.revision snapshot.fact)

theorem foreign_write_preserves_store (s : Facts) (actor : Owner) (fact : Fact)
    (value : Nat) (foreign : actor ≠ authority fact) : writeFact s actor fact value = s := by
  simp [writeFact, foreign]

theorem write_has_single_authority (s : Facts) (actor : Owner) (fact : Fact) (value : Nat)
    (changed : (writeFact s actor fact value).revision fact ≠ s.revision fact) :
    actor = authority fact := by
  by_cases owns : actor = authority fact
  · exact owns
  · simp [writeFact, owns] at changed

theorem write_frames_other_facts (s : Facts) (actor : Owner) (fact other : Fact)
    (value : Nat) (different : other ≠ fact) :
    (writeFact s actor fact value).value other = s.value other ∧
      (writeFact s actor fact value).revision other = s.revision other := by
  by_cases owns : actor = authority fact <;> simp [writeFact, owns, different]

theorem write_advances_revision (s : Facts) (fact : Fact) (value : Nat) :
    (writeFact s (authority fact) fact value).revision fact = s.revision fact + 1 := by
  simp [writeFact]

theorem write_preserves_provenance (s : Facts) (actor : Owner) (fact : Fact)
    (value : Nat) (h : Provenance s) : Provenance (writeFact s actor fact value) := by
  by_cases owns : actor = authority fact <;> simpa [writeFact, owns, Provenance] using h

theorem publish_preserves_provenance (s : Facts) (actor : Owner) (fact : Fact)
    (offered required : Version) (h : Provenance s) :
    Provenance (publishFact s actor fact offered required) := by
  unfold publishFact
  split
  · rename_i accepted
    intro snapshot member
    simp only [List.mem_append, List.mem_singleton] at member
    rcases member with old | fresh
    · exact h snapshot old
    · subst snapshot
      exact accepted
  · exact h

theorem incompatible_boundary_rejects (s : Facts) (actor : Owner) (fact : Fact)
    (offered required : Version) (bad : ¬ Compatible offered required) :
    publishFact s actor fact offered required = s := by
  simp [publishFact, bad]

theorem publication_reads_authority_state (s : Facts) (fact : Fact)
    (offered required : Version) (compatible : Compatible offered required) :
    (publishFact s (authority fact) fact offered required).delivered = s.delivered ++
      [⟨fact, s.revision fact, s.value fact, authority fact, offered, required⟩] := by
  simp [publishFact, compatible]

inductive FactEvent where
  | write (actor : Owner) (fact : Fact) (value : Nat)
  | publish (actor : Owner) (fact : Fact) (offered required : Version)

def factStep (s : Facts) : FactEvent → Facts
  | .write actor fact value => writeFact s actor fact value
  | .publish actor fact offered required => publishFact s actor fact offered required

def factRun (s : Facts) : List FactEvent → Facts
  | [] => s
  | event :: rest => factRun (factStep s event) rest

theorem fact_trace_provenance (s : Facts) (events : List FactEvent)
    (h : Provenance s) : Provenance (factRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih =>
    apply ih
    cases event with
    | write actor fact value => exact write_preserves_provenance s actor fact value h
    | publish actor fact offered required =>
      exact publish_preserves_provenance s actor fact offered required h

theorem delivered_facts_have_authority (events : List FactEvent) :
    Provenance (factRun initialFacts events) :=
  fact_trace_provenance _ _ (by simp [Provenance, initialFacts])

def SnapshotValid (s : Facts) : Prop :=
  ∀ snapshot ∈ s.delivered,
    snapshot.revision ≤ s.revision snapshot.fact ∧
      (snapshot.revision = s.revision snapshot.fact → snapshot.value = s.value snapshot.fact)

theorem write_preserves_snapshot_validity (s : Facts) (actor : Owner) (fact : Fact)
    (value : Nat) (h : SnapshotValid s) : SnapshotValid (writeFact s actor fact value) := by
  by_cases owns : actor = authority fact
  · intro snapshot member
    have old := h snapshot (by simpa [writeFact, owns] using member)
    by_cases same : snapshot.fact = fact
    · simp only [writeFact, if_pos owns, if_pos same]
      constructor
      · simpa [same] using Nat.le_trans old.1 (Nat.le_succ (s.revision snapshot.fact))
      · intro impossible
        have bound := old.1
        rw [same] at bound
        omega
    · simpa [writeFact, owns, same] using old
  · simpa [writeFact, owns] using h

theorem publish_preserves_snapshot_validity (s : Facts) (actor : Owner) (fact : Fact)
    (offered required : Version) (h : SnapshotValid s) :
    SnapshotValid (publishFact s actor fact offered required) := by
  unfold publishFact
  split
  · intro snapshot member
    simp only [List.mem_append, List.mem_singleton] at member
    rcases member with old | fresh
    · exact h snapshot old
    · subst snapshot
      exact ⟨Nat.le_refl _, fun _ => rfl⟩
  · exact h

theorem fact_trace_preserves_snapshots (s : Facts) (events : List FactEvent)
    (h : SnapshotValid s) : SnapshotValid (factRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih =>
    apply ih
    cases event with
    | write actor fact value => exact write_preserves_snapshot_validity s actor fact value h
    | publish actor fact offered required =>
      exact publish_preserves_snapshot_validity s actor fact offered required h

theorem current_snapshot_matches_authoritative_value (events : List FactEvent)
    (snapshot : Snapshot) (current : currentSnapshot (factRun initialFacts events) snapshot = true) :
    snapshot.value = (factRun initialFacts events).value snapshot.fact := by
  have valid := fact_trace_preserves_snapshots initialFacts events
    (by simp [SnapshotValid, initialFacts])
  have accepted : snapshot ∈ (factRun initialFacts events).delivered ∧
      snapshot.revision = (factRun initialFacts events).revision snapshot.fact := by
    simpa [currentSnapshot] using current
  exact (valid snapshot accepted.1).2 accepted.2

/-! Resources are addressed independently from owners. A transfer has a unique
source, destination and request identity. Cancelling after submission transfers
cleanup custody back to the original owner; completion moves custody to the
receiver. A stale request id cannot reclaim a newer transfer. -/

inductive Custody where
  | absent
  | owned (owner : Owner)
  | inFlight (sender receiver : Owner) (request : Nat)
  | cleaning (owner : Owner)
  | returned
  deriving DecidableEq

structure Resource where
  custody : Custody := .absent
  issued : Nat := 0
  returned : Nat := 0
  /-- First unused transfer stamp; retained across ownership changes. -/
  nextRequest : Nat := 0
  deriving DecidableEq

def live : Custody → Nat
  | .absent | .returned => 0
  | .owned _ | .inFlight _ _ _ | .cleaning _ => 1

def Conserved (s : Resource) : Prop :=
  live s.custody + s.returned = s.issued ∧ s.issued ≤ 1 ∧
    (s.custody = .absent → s.issued = 0) ∧
    (s.custody = .returned → s.returned = 1)

inductive ResourceEvent where
  | acquire (owner : Owner)
  | send (sender receiver : Owner) (request : Nat)
  | receive (receiver : Owner) (request : Nat)
  | cancel (sender : Owner) (request : Nat)
  | beginCleanup (owner : Owner)
  | completeCleanup (owner : Owner)
  deriving DecidableEq

def resourceStep (s : Resource) (event : ResourceEvent) : Resource :=
  match s.custody, event with
  | .absent, .acquire owner => { s with custody := .owned owner, issued := 1, returned := 0 }
  | .owned owner, .send sender receiver request =>
    if sender = owner ∧ s.nextRequest ≤ request then
      { s with custody := .inFlight sender receiver request, nextRequest := request + 1 }
    else s
  | .inFlight sender receiver request, .receive actor observed =>
    if actor = receiver ∧ observed = request then { s with custody := .owned receiver } else s
  | .inFlight sender receiver request, .cancel actor observed =>
    if actor = sender ∧ observed = request then { s with custody := .cleaning sender } else s
  | .owned owner, .beginCleanup actor =>
    if actor = owner then { s with custody := .cleaning owner } else s
  | .cleaning owner, .completeCleanup actor =>
    if actor = owner then { s with custody := .returned, returned := s.returned + 1 } else s
  | _, _ => s

theorem resource_step_conserves (s : Resource) (event : ResourceEvent)
    (h : Conserved s) : Conserved (resourceStep s event) := by
  rcases h with ⟨balance, bound, absent, returned⟩
  cases hc : s.custody <;> cases event <;> simp only [resourceStep, hc]
  all_goals try split
  all_goals simp_all [Conserved, live]
  all_goals omega

def resourceRun (s : Resource) : List ResourceEvent → Resource
  | [] => s
  | event :: rest => resourceRun (resourceStep s event) rest

theorem resource_trace_conserves (s : Resource) (events : List ResourceEvent)
    (h : Conserved s) : Conserved (resourceRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons event rest ih => exact ih _ (resource_step_conserves s event h)

theorem transfer_stamp_never_decreases (s : Resource) (event : ResourceEvent) :
    s.nextRequest ≤ (resourceStep s event).nextRequest := by
  cases hc : s.custody <;> cases event <;> simp only [resourceStep, hc]
  all_goals try split
  all_goals simp_all
  all_goals omega

theorem transfer_history_keeps_stamp_frontier (s : Resource) (events : List ResourceEvent) :
    s.nextRequest ≤ (resourceRun s events).nextRequest := by
  induction events generalizing s with
  | nil => exact Nat.le_refl _
  | cons event rest ih =>
    exact Nat.le_trans (transfer_stamp_never_decreases s event) (ih _)

theorem used_transfer_stamp_cannot_be_sent_again (s : Resource)
    (sender receiver : Owner) (request : Nat) (used : request < s.nextRequest) :
    resourceStep s (.send sender receiver request) = s := by
  cases hc : s.custody <;> simp [resourceStep, hc, Nat.not_le.mpr used]

theorem fresh_transfer_rejects_earlier_callbacks (s : Resource)
    (sender receiver : Owner) (request observed : Nat)
    (owned : s.custody = .owned sender) (fresh : s.nextRequest ≤ request)
    (old : observed < s.nextRequest) :
    let sent := resourceStep s (.send sender receiver request)
    resourceStep sent (.receive receiver observed) = sent ∧
      resourceStep sent (.cancel sender observed) = sent := by
  have different : observed ≠ request := by omega
  simp [resourceStep, owned, fresh, different]

theorem old_callbacks_cannot_match_a_later_transfer (s : Resource)
    (events : List ResourceEvent) (sender receiver : Owner) (request observed : Nat)
    (owned : (resourceRun s events).custody = .owned sender)
    (fresh : (resourceRun s events).nextRequest ≤ request)
    (old : observed < s.nextRequest) :
    let sent := resourceStep (resourceRun s events) (.send sender receiver request)
    resourceStep sent (.receive receiver observed) = sent ∧
      resourceStep sent (.cancel sender observed) = sent := by
  exact fresh_transfer_rejects_earlier_callbacks _ sender receiver request observed owned fresh
    (Nat.lt_of_lt_of_le old (transfer_history_keeps_stamp_frontier s events))

theorem acquired_resource_has_unique_custody (events : List ResourceEvent) :
    live (resourceRun {} events).custody + (resourceRun {} events).returned =
      (resourceRun {} events).issued :=
  (resource_trace_conserves {} events (by simp [Conserved, live])).1

theorem resource_returned_at_most_once (events : List ResourceEvent) :
    (resourceRun {} events).returned ≤ 1 := by
  have h := resource_trace_conserves {} events (by simp [Conserved, live])
  have balance := h.1
  have bound := h.2.1
  omega

theorem completed_transfer_cannot_be_cancelled (s : Resource) (sender receiver : Owner)
    (request : Nat) (h : s.custody = .inFlight sender receiver request) :
    resourceStep (resourceStep s (.receive receiver request)) (.cancel sender request) =
      resourceStep s (.receive receiver request) := by
  simp [resourceStep, h]

theorem cancelled_transfer_cannot_be_delivered (s : Resource) (sender receiver : Owner)
    (request : Nat) (h : s.custody = .inFlight sender receiver request) :
    resourceStep (resourceStep s (.cancel sender request)) (.receive receiver request) =
      resourceStep s (.cancel sender request) := by
  simp [resourceStep, h]

theorem mismatched_transfer_preserves_custody (s : Resource) (sender receiver : Owner)
    (request observed : Nat) (h : s.custody = .inFlight sender receiver request)
    (stale : observed ≠ request) :
    resourceStep s (.receive receiver observed) = s ∧
      resourceStep s (.cancel sender observed) = s := by
  simp [resourceStep, h, stale]

theorem owned_cleanup_returns_resource (s : Resource) (owner : Owner)
    (h : s.custody = .owned owner) :
    (resourceRun s [.beginCleanup owner, .completeCleanup owner]).custody = .returned := by
  simp [resourceRun, resourceStep, h]

theorem cancelled_transfer_returns_resource (s : Resource) (sender receiver : Owner)
    (request : Nat) (h : s.custody = .inFlight sender receiver request) :
    (resourceRun s [.cancel sender request, .completeCleanup sender]).custody = .returned := by
  simp [resourceRun, resourceStep, h]

/-! An arbitrary number of resource ids shares the same event machinery. The
frame lemma is the separation rule used when composing independently owned
subsystems. No finite directory-size bound is imposed on this theorem. -/

abbrev Resources := Nat → Resource

def updateResource (resources : Resources) (id : Nat) (event : ResourceEvent) : Resources :=
  fun other => if other = id then resourceStep (resources id) event else resources other

def AllConserved (resources : Resources) : Prop := ∀ id, Conserved (resources id)

theorem update_resource_frames_others (resources : Resources) (id other : Nat)
    (event : ResourceEvent) (different : other ≠ id) :
    updateResource resources id event other = resources other := by
  simp [updateResource, different]

theorem update_resource_conserves_all (resources : Resources) (id : Nat)
    (event : ResourceEvent) (h : AllConserved resources) :
    AllConserved (updateResource resources id event) := by
  intro other
  by_cases same : other = id
  · subst other
    simpa [updateResource] using resource_step_conserves (resources id) event (h id)
  · simpa [updateResource, same] using h other

theorem distinct_resource_steps_commute (resources : Resources) (left right : Nat)
    (first second : ResourceEvent) (different : left ≠ right) :
    updateResource (updateResource resources left first) right second =
      updateResource (updateResource resources right second) left first := by
  funext id
  by_cases hl : id = left <;> by_cases hr : id = right
  · exact False.elim (different (hl.symm.trans hr))
  · subst id
    simp [updateResource, different, Ne.symm different]
  · subst id
    simp [updateResource, different, Ne.symm different]
  · simp [updateResource, hl, hr]

/-! The supervisor's two concrete provenance adapters. Non-text metadata keys
are retained. Every reserved text key is removed before core-owned fields are
appended, including duplicate attacker-supplied keys. Origin identity is taken
from the selected active generation and never from the request's payload. -/

inductive MetadataKey where
  | text (key : String)
  | other (identity : Nat)
  deriving DecidableEq

def reservedKey : MetadataKey → Bool
  | .text key => key.startsWith "cordis."
  | .other _ => false

abbrev Metadata := List (MetadataKey × Nat)

def scopedMetadata (untrusted trusted : Metadata) : Metadata :=
  untrusted.filter (fun field => !reservedKey field.1) ++ trusted

theorem trusted_metadata_has_authoritative_source (untrusted trusted : Metadata)
    (field : MetadataKey × Nat) (member : field ∈ scopedMetadata untrusted trusted)
    (reserved : reservedKey field.1 = true) : field ∈ trusted := by
  simp only [scopedMetadata, List.mem_append, List.mem_filter] at member
  rcases member with forged | authoritative
  · simp [reserved] at forged
  · exact authoritative

theorem business_metadata_is_preserved (untrusted trusted : Metadata)
    (field : MetadataKey × Nat) (member : field ∈ untrusted)
    (business : reservedKey field.1 = false) : field ∈ scopedMetadata untrusted trusted := by
  simp [scopedMetadata, member, business]

theorem every_trusted_field_is_delivered (untrusted trusted : Metadata)
    (field : MetadataKey × Nat) (member : field ∈ trusted) :
    field ∈ scopedMetadata untrusted trusted := by
  simp [scopedMetadata, member]

structure Generation where
  connection : Nat
  context : Nat
  instanceId : Nat
  group : Nat
  plugin : Nat
  active : Bool
  deriving DecidableEq

structure Origin where
  connection : Nat
  context : Nat
  instanceId : Nat
  group : Nat
  plugin : Nat
  deriving DecidableEq

def originOf (generation : Generation) : Origin :=
  ⟨generation.connection, generation.context, generation.instanceId,
    generation.group, generation.plugin⟩

def requestOrigin (generation : Generation) (connection context : Nat) : Option Origin :=
  if generation.connection = connection ∧ generation.context = context ∧ generation.active = true
  then some (originOf generation) else none

theorem successful_origin_is_core_owned (generation : Generation) (connection context : Nat)
    (origin : Origin) (success : requestOrigin generation connection context = some origin) :
    origin = originOf generation ∧ origin.connection = connection ∧ origin.context = context ∧
      generation.active = true := by
  unfold requestOrigin at success
  split at success
  · rename_i valid
    have same := Option.some.inj success
    subst origin
    exact ⟨rfl, valid.1, valid.2.1, valid.2.2⟩
  · contradiction

theorem foreign_connection_origin_rejected (generation : Generation) (connection context : Nat)
    (foreign : generation.connection ≠ connection) :
    requestOrigin generation connection context = none := by
  simp [requestOrigin, foreign]

theorem inactive_origin_rejected (generation : Generation) (connection context : Nat)
    (inactive : generation.active = false) :
    requestOrigin generation connection context = none := by
  simp [requestOrigin, inactive]

end Cordis.Architecture
