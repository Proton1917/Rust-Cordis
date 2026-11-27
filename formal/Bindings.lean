import KernelRefinement

/-!
Ordered dependency resolution, committed service lookup, optimistic refresh
validation, and consumer retention. Function values stand for locked finite
tables; identifiers preserve equality. The compiled bridge checks the source
functions against executable cases, while the theorems quantify over arbitrary
lists and table contents.
-/

namespace Cordis.Bindings

structure Dependency where
  key : String
  required : Bool
  deriving DecidableEq

structure Implementation where
  uid : UInt64
  owner : Nat
  deriving DecidableEq

abbrev Table := Nat → Option Implementation

def uidAt (table : Table) (binding : Option Nat) : Option UInt64 :=
  binding.bind fun id => (table id).map Implementation.uid

def slot (table : Table) (pair : Option Nat × Dependency) : Option UInt64 :=
  (uidAt table pair.1).orElse fun _ => if pair.2.required then none else some 0

/-- A resolved slot selects its binding identity; an absent optional slot uses
    the zero sentinel. Required absence has no satisfying output identity. -/
def SlotMeaning (table : Table) (pair : Option Nat × Dependency) (uid : UInt64) : Prop :=
  uidAt table pair.1 = some uid ∨
    (uidAt table pair.1 = none ∧ pair.2.required = false ∧ uid = 0)

theorem slot_meaning (table : Table) (pair : Option Nat × Dependency) (uid : UInt64) :
    slot table pair = some uid ↔ SlotMeaning table pair uid := by
  cases found : uidAt table pair.1 <;> cases required : pair.2.required <;>
    simp [slot, SlotMeaning, found, required, eq_comm, Option.orElse]

def collect (table : Table) : List (Option Nat × Dependency) → Option (List UInt64)
  | [] => some []
  | pair :: rest => do
    let uid ← slot table pair
    let uids ← collect table rest
    pure (uid :: uids)

def Slotwise (table : Table) : List (Option Nat × Dependency) → List UInt64 → Prop
  | [], [] => True
  | pair :: pairs, uid :: uids => SlotMeaning table pair uid ∧ Slotwise table pairs uids
  | _, _ => False

theorem collect_meaning (table : Table) (pairs : List (Option Nat × Dependency)) (uids : List UInt64) :
    collect table pairs = some uids ↔ Slotwise table pairs uids := by
  induction pairs generalizing uids with
  | nil => cases uids <;> simp [collect, Slotwise]
  | cons pair rest ih =>
    cases uids with
    | nil =>
      cases hs : slot table pair <;> cases hc : collect table rest <;>
        simp [collect, Slotwise, hs, hc]
    | cons uid uids =>
      simp only [Slotwise]
      rw [← slot_meaning, ← ih]
      cases hs : slot table pair <;> cases hc : collect table rest <;> simp [collect, hs, hc]

theorem slotwise_preserves_length (table : Table) (pairs : List (Option Nat × Dependency))
    (uids : List UInt64) (meaning : Slotwise table pairs uids) : pairs.length = uids.length := by
  induction pairs generalizing uids with
  | nil => cases uids <;> simp_all [Slotwise]
  | cons pair pairs ih =>
    cases uids with
    | nil => simp [Slotwise] at meaning
    | cons uid uids => exact congrArg Nat.succ (ih uids meaning.2)

def epochIds (table : Table) (bindings : List (Option Nat)) (dependencies : List Dependency) :
    Option (List UInt64) :=
  if bindings.length != dependencies.length then none
  else collect table (bindings.zip dependencies)

/-- Soundness and completeness of the length check and ordered Option collect. -/
theorem epoch_resolution_spec (table : Table) (bindings : List (Option Nat))
    (dependencies : List Dependency) (uids : List UInt64) :
    epochIds table bindings dependencies = some uids ↔
      bindings.length = dependencies.length ∧
      Slotwise table (bindings.zip dependencies) uids := by
  unfold epochIds
  split <;> simp_all [collect_meaning]

theorem resolved_epoch_preserves_length (table : Table) (bindings : List (Option Nat))
    (dependencies : List Dependency) (uids : List UInt64)
    (resolved : epochIds table bindings dependencies = some uids) :
    uids.length = dependencies.length := by
  obtain ⟨lengths, meaning⟩ := (epoch_resolution_spec table bindings dependencies uids).mp resolved
  have same := slotwise_preserves_length table (bindings.zip dependencies) uids meaning
  simp [List.length_zip, lengths] at same
  exact same.symm

theorem required_slot_has_binding (table : Table) (binding : Option Nat) (dependency : Dependency)
    (uid : UInt64) (required : dependency.required = true)
    (meaning : SlotMeaning table (binding, dependency) uid) :
    ∃ id implementation, binding = some id ∧ table id = some implementation ∧ implementation.uid = uid := by
  have found : uidAt table binding = some uid := by
    rcases meaning with found | absent
    · exact found
    · simp [required] at absent
  cases binding with
  | none => simp [uidAt] at found
  | some id =>
    cases present : table id with
    | none => simp [uidAt, present] at found
    | some implementation =>
      exact ⟨id, implementation, rfl, present, by simpa [uidAt, present] using found⟩

theorem zero_identifies_optional_absence (table : Table) (pair : Option Nat × Dependency)
    (nonzero : ∀ uid, uidAt table pair.1 = some uid → uid ≠ 0) :
    slot table pair = some 0 ↔ uidAt table pair.1 = none ∧ pair.2.required = false := by
  rw [slot_meaning]
  unfold SlotMeaning
  constructor
  · intro h
    rcases h with found | absent
    · exact False.elim (nonzero 0 found rfl)
    · exact ⟨absent.1, absent.2.1⟩
  · intro h
    exact Or.inr ⟨h.1, h.2, rfl⟩

def commitPairs (dependencies : List Dependency) (bindings : List (Option Nat)) : List (String × Nat) :=
  (dependencies.zip bindings).filterMap fun (dependency, binding) =>
    binding.map fun id => (dependency.key, id)

theorem committed_membership (dependencies : List Dependency) (bindings : List (Option Nat))
    (key : String) (id : Nat) :
    (key, id) ∈ commitPairs dependencies bindings ↔
      ∃ dependency, (dependency, some id) ∈ dependencies.zip bindings ∧ dependency.key = key := by
  simp only [commitPairs, List.mem_filterMap]
  constructor
  · rintro ⟨⟨dependency, binding⟩, member, value⟩
    cases binding with
    | none => simp at value
    | some actual =>
      simp only [Option.map, Option.some.injEq, Prod.mk.injEq] at value
      exact ⟨dependency, by simpa [value.2] using member, value.1⟩
  · rintro ⟨dependency, member, keyEq⟩
    exact ⟨(dependency, some id), member, by simp [keyEq]⟩

structure Provision where
  id : Nat
  key : String
  realm : Nat
  deriving DecidableEq

/-- Matching isolation layers are ordered from the use site toward the root. -/
def realmFor (key : String) : List (String × Nat) → Nat
  | [] => 0
  | (isolated, realm) :: parents => if isolated == key then realm else realmFor key parents

theorem nearest_isolation_selects_realm (key : String) (realm : Nat) (parents : List (String × Nat)) :
    realmFor key ((key, realm) :: parents) = realm := by simp [realmFor]

theorem unrelated_isolation_preserves_realm (key other : String) (realm : Nat)
    (parents : List (String × Nat)) (distinct : other ≠ key) :
    realmFor key ((other, realm) :: parents) = realmFor key parents := by simp [realmFor, distinct]

theorem unrelated_prefix_preserves_realm (key : String) (layers parents : List (String × Nat))
    (unrelated : ∀ layer ∈ layers, layer.1 ≠ key) :
    realmFor key (layers ++ parents) = realmFor key parents := by
  induction layers with
  | nil => rfl
  | cons layer rest ih =>
    have first := unrelated layer (by simp)
    have tail : ∀ item ∈ rest, item.1 ≠ key := fun item member => unrelated item (by simp [member])
    simpa [realmFor, first] using ih tail

structure Frame where
  owner : Nat
  provisions : List Provision
  /-- Isolations between the lookup cursor and this owner's Fiber/Root layer. -/
  isolations : List String
  committed : List (String × Nat)
  declared : List String
  deriving DecidableEq

inductive Located where
  | provision (id : Nat)
  | committed (owner id : Nat)
  | declaredAbsent (owner : Nat)
  deriving DecidableEq

def ownProvision (frame : Frame) (key : String) (realm : Nat) : Option Nat :=
  (frame.provisions.reverse.find? fun binding => binding.key == key && binding.realm == realm).map
    Provision.id

def committedBinding (frame : Frame) (key : String) : Option Nat :=
  (frame.committed.find? fun binding => binding.1 == key).map Prod.snd

/-- The Rust `locate` loop is traversed in owner segments. Provision search
    precedes isolation; committed and declared entries precede parent traversal. -/
def locate (key : String) (realm : Nat) : List Frame → Option Located
  | [] => none
  | frame :: parents =>
    match ownProvision frame key realm with
    | some id => some (.provision id)
    | none =>
      if key ∈ frame.isolations then none
      else match committedBinding frame key with
      | some id => some (.committed frame.owner id)
      | none => if key ∈ frame.declared then some (.declaredAbsent frame.owner)
        else locate key realm parents

theorem own_provision_has_priority (frame : Frame) (parents : List Frame) (key : String) (realm id : Nat)
    (provided : ownProvision frame key realm = some id) :
    locate key realm (frame :: parents) = some (.provision id) := by simp [locate, provided]

theorem latest_matching_provision_wins (frame : Frame) (key : String) (realm id : Nat) :
    ownProvision {frame with provisions := frame.provisions ++ [⟨id, key, realm⟩]} key realm = some id := by
  simp [ownProvision, List.reverse_append]

theorem isolation_seals_ancestors (frame : Frame) (left right : List Frame) (key : String) (realm : Nat)
    (barrier : key ∈ frame.isolations) :
    locate key realm (frame :: left) = locate key realm (frame :: right) := by
  cases found : ownProvision frame key realm <;> simp [locate, found, barrier]

theorem declared_absence_seals_ancestors (frame : Frame) (parents : List Frame)
    (key : String) (realm : Nat)
    (localAbsent : ownProvision frame key realm = none)
    (openView : key ∉ frame.isolations) (uncommitted : committedBinding frame key = none)
    (declared : key ∈ frame.declared) :
    locate key realm (frame :: parents) = some (.declaredAbsent frame.owner) := by
  simp [locate, localAbsent, openView, uncommitted, declared]

/-- A component's committed binding persists across replacement of the live
    realm store. Root lookup deliberately remains a live-store operation. -/
def getBinding (isRoot : Bool) (live : String → Nat → Option Nat)
    (frames : List Frame) (key : String) (realm : Nat) : Option Nat :=
  if isRoot then live key realm
  else (locate key realm frames).bind fun found => match found with
    | .provision id | .committed _ id => some id
    | .declaredAbsent _ => none

theorem committed_read_survives_live_replacement (frame : Frame) (parents : List Frame)
    (oldLive newLive : String → Nat → Option Nat) (key : String) (realm id : Nat)
    (localAbsent : ownProvision frame key realm = none)
    (openView : key ∉ frame.isolations) (fixed : committedBinding frame key = some id) :
    getBinding false oldLive (frame :: parents) key realm = some id ∧
      getBinding false newLive (frame :: parents) key realm = some id := by
  simp [getBinding, locate, localAbsent, openView, fixed]

structure Snapshot where
  binding : Option Nat
  notificationRevision : Option UInt64
  active : Bool
  predicate : Option Nat
  /-- Present only when a consumer predicate captured this Arc identity. -/
  observedValue : Option Nat
  deriving DecidableEq

structure Current where
  binding : Option Nat
  notificationRevision : Option UInt64
  active : Bool
  predicate : Option Nat
  value : Option Nat
  deriving DecidableEq

def validSnapshot (expectedKey actualKey : String) (old : Snapshot) (now : Current) : Bool :=
  actualKey == expectedKey && now.binding == old.binding &&
    now.notificationRevision == old.notificationRevision && now.active == old.active &&
    now.predicate == old.predicate &&
    old.observedValue.all (fun value => now.value == some value)

def SnapshotAgreement (expectedKey actualKey : String) (old : Snapshot) (now : Current) : Prop :=
  actualKey = expectedKey ∧ now.binding = old.binding ∧
    now.notificationRevision = old.notificationRevision ∧ now.active = old.active ∧
    now.predicate = old.predicate ∧
    (∀ value, old.observedValue = some value → now.value = some value)

theorem snapshot_validation_spec (expectedKey actualKey : String) (old : Snapshot) (now : Current) :
    validSnapshot expectedKey actualKey old now = true ↔ SnapshotAgreement expectedKey actualKey old now := by
  cases value : old.observedValue <;> simp [validSnapshot, SnapshotAgreement, value, and_assoc]

def installChecked (previous : Option Nat) (expectedKey actualKey : String)
    (old : Snapshot) (now : Current) (available : Bool) : Option Nat :=
  if validSnapshot expectedKey actualKey old now then old.binding.filter (fun _ => available)
  else previous

theorem stale_notification_cannot_commit (previous : Option Nat) (expectedKey actualKey : String)
    (old : Snapshot) (now : Current) (available : Bool)
    (changed : now.notificationRevision ≠ old.notificationRevision) :
    installChecked previous expectedKey actualKey old now available = previous := by
  simp [installChecked, validSnapshot, changed]

theorem incremented_notification_invalidates_snapshot (previous : Option Nat)
    (expectedKey actualKey : String) (old : Snapshot) (now : Current) (available : Bool)
    (revision : UInt64) (room : revision.toNat + 1 < UInt64.size) :
    installChecked previous expectedKey actualKey {old with notificationRevision := some revision}
      {now with notificationRevision := some (revision + 1)} available = previous := by
  apply stale_notification_cannot_commit
  intro same
  exact KernelRefinement.revision_increment_changes_identity revision room (Option.some.inj same)

theorem successful_snapshot_install_has_all_facts (previous result : Option Nat)
    (expectedKey actualKey : String) (old : Snapshot) (now : Current) (available : Bool)
    (changed : installChecked previous expectedKey actualKey old now available = result)
    (different : result ≠ previous) :
    SnapshotAgreement expectedKey actualKey old now ∧ result = old.binding.filter (fun _ => available) := by
  unfold installChecked at changed
  split at changed
  · rename_i accepted
    exact ⟨(snapshot_validation_spec ..).mp accepted, changed.symm⟩
  · exact False.elim (different changed.symm)

abbrev Dependents := Nat → Nat → Prop

def retain (before : Dependents) (consumer : Nat) (committed : List (String × Nat))
    (owner : Nat → Nat) : Dependents :=
  fun binding dependent => before binding dependent ∨
    (dependent = consumer ∧ (∃ key, (key, binding) ∈ committed) ∧ owner binding ≠ consumer)

def retained (committed : List (String × Nat)) (consumer binding : Nat) (owner : Nat → Nat) : Bool :=
  committed.any (fun pair => pair.2 == binding) && owner binding != consumer

theorem retained_membership_spec (committed : List (String × Nat)) (consumer binding : Nat)
    (owner : Nat → Nat) :
    retained committed consumer binding owner = true ↔
      retain (fun _ _ => False) consumer committed owner binding consumer := by
  simp only [retained, Bool.and_eq_true, List.any_eq_true, beq_iff_eq,
    bne_iff_ne, retain, false_or, true_and]
  constructor
  · rintro ⟨⟨⟨key, id⟩, member, same⟩, external⟩
    change id = binding at same
    exact ⟨⟨key, by simpa [same] using member⟩, external⟩
  · rintro ⟨⟨key, member⟩, external⟩
    exact ⟨⟨(key, binding), member, rfl⟩, external⟩

def release (before : Dependents) (consumer : Nat) (committed : List (String × Nat)) : Dependents :=
  fun binding dependent => before binding dependent ∧
    ¬(dependent = consumer ∧ ∃ key, (key, binding) ∈ committed)

def containsBinding (committed : List (String × Nat)) (binding : Nat) : Bool :=
  committed.any fun pair => pair.2 == binding

theorem contains_binding_spec (committed : List (String × Nat)) (binding : Nat) :
    containsBinding committed binding = true ↔ ∃ key, (key, binding) ∈ committed := by
  simp only [containsBinding, List.any_eq_true, beq_iff_eq]
  constructor
  · rintro ⟨⟨key, id⟩, member, same⟩
    change id = binding at same
    exact ⟨key, by simpa [same] using member⟩
  · rintro ⟨key, member⟩
    exact ⟨(key, binding), member, rfl⟩

def released (before : Nat → Nat → Bool) (consumer : Nat) (committed : List (String × Nat))
    (binding dependent : Nat) : Bool :=
  before binding dependent && !(dependent == consumer && containsBinding committed binding)

theorem released_membership_spec (before : Nat → Nat → Bool) (consumer : Nat)
    (committed : List (String × Nat)) (binding dependent : Nat) :
    released before consumer committed binding dependent = true ↔
      release (fun id who => before id who = true) consumer committed binding dependent := by
  simp only [released, release, Bool.and_eq_true, Bool.not_eq, ne_eq,
    beq_iff_eq, contains_binding_spec]

def ready (dependents : Dependents) (provisions : List Nat) : Prop :=
  ∀ binding ∈ provisions, ∀ consumer, ¬dependents binding consumer

theorem committed_consumer_retained (before : Dependents) (consumer binding : Nat)
    (committed : List (String × Nat)) (owner : Nat → Nat) (key : String)
    (member : (key, binding) ∈ committed) (external : owner binding ≠ consumer) :
    retain before consumer committed owner binding consumer :=
  Or.inr ⟨rfl, ⟨key, member⟩, external⟩

theorem provider_waits_for_committed_consumer (before : Dependents) (consumer binding : Nat)
    (committed : List (String × Nat)) (owner : Nat → Nat) (key : String) (provisions : List Nat)
    (member : (key, binding) ∈ committed) (external : owner binding ≠ consumer)
    (provided : binding ∈ provisions) :
    ¬ready (retain before consumer committed owner) provisions := by
  intro done
  exact done binding provided consumer
    (committed_consumer_retained before consumer binding committed owner key member external)

theorem releasing_consumer_preserves_others (before : Dependents) (consumer other binding : Nat)
    (committed : List (String × Nat)) (distinct : other ≠ consumer) :
    release before consumer committed binding other ↔ before binding other := by
  simp [release, distinct]

theorem releasing_committed_consumer_removes_membership (before : Dependents) (consumer binding : Nat)
    (committed : List (String × Nat)) (key : String) (member : (key, binding) ∈ committed) :
    ¬release before consumer committed binding consumer := by
  simp only [release, true_and]
  intro remaining
  exact remaining.2 ⟨key, member⟩

end Cordis.Bindings
