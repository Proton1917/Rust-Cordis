import RevertibleEffects

/-!
Typed dependency tables and the paper's reactive observations (§3.2), using
finite specifications as in Rust `Inject`.  Type identity is retained in every
realm slot: isolation redirects a key inside that key's value family.
-/
namespace Cordis.ReactiveCoeffects

universe u v w

abbrev Store {K : Type u} (V : K → Type v) := (k : K) → Option (V k)
abbrev Specification (K : Type u) := List K

variable {K : Type u} {V : K → Type v} [DecidableEq K]

def empty : Store V := fun _ => none

def update (s : Store V) (key : K) (value : Option (V key)) : Store V :=
  fun other => if h : other = key then h.symm ▸ value else s other

@[simp] theorem update_same (s : Store V) (key : K) (value : Option (V key)) :
    update s key value key = value := by simp [update]

theorem update_other (s : Store V) (key other : K) (value : Option (V key))
    (different : other ≠ key) : update s key value other = s other := by
  simp [update, different]

theorem update_restores (s : Store V) (key : K) : update s key (s key) = s := by
  funext other
  by_cases same : other = key
  · subst other; simp
  · simp [update, same]

theorem update_update (s : Store V) (key : K) (first second : Option (V key)) :
    update (update s key first) key second = update s key second := by
  funext other
  by_cases same : other = key
  · subst other; simp
  · simp [update, same]

theorem independent_updates_commute (s : Store V) (first second : K)
    (left : Option (V first)) (right : Option (V second)) (different : first ≠ second) :
    update (update s first left) second right = update (update s second right) first left := by
  funext other
  by_cases hfirst : other = first
  · subst other; simp [update, different]
  · by_cases hsecond : other = second
    · subst other; simp [update, hfirst]
    · simp [update, hfirst, hsecond]

/-- A finite support witness states the exact domain, independently of storage
representation. Finite declarations provide decidable satisfaction. -/
def SupportedBy (s : Store V) (keys : List K) : Prop :=
  ∀ key, (s key).isSome = true ↔ key ∈ keys

omit [DecidableEq K] in
theorem empty_support : SupportedBy (empty : Store V) [] := by
  intro key; simp [empty]

theorem insertion_preserves_finite_support (s : Store V) (keys : List K)
    (finite : SupportedBy s keys) (key : K) (value : V key) :
    SupportedBy (update s key (some value)) (key :: keys) := by
  intro other
  by_cases same : other = key
  · subst other; simp
  · simp [update, same, finite other]

theorem removal_preserves_finite_support (s : Store V) (keys : List K)
    (finite : SupportedBy s keys) (key : K) :
    SupportedBy (update s key none) (keys.filter (fun other => other != key)) := by
  intro other
  by_cases same : other = key
  · subst other; simp
  · simp [update, same, finite other]

/-- Definition 12's finite-domain requirement is an explicit witness, rather
than a consequence of having a function-valued representation. -/
structure FiniteStore (V : K → Type v) where
  entries : Store V
  keys : List K
  supported : SupportedBy entries keys

def emptyFinite : FiniteStore V := ⟨empty, [], empty_support⟩

def insertFinite (s : FiniteStore V) (key : K) (value : V key) : FiniteStore V :=
  ⟨update s.entries key (some value), key :: s.keys,
   insertion_preserves_finite_support _ _ s.supported key value⟩

def removeFinite (s : FiniteStore V) (key : K) : FiniteStore V :=
  ⟨update s.entries key none, s.keys.filter (fun other => other != key),
   removal_preserves_finite_support _ _ s.supported key⟩

/-- Definition 13's presence precondition makes dependency access total. -/
def get (s : Store V) (key : K) (present : (s key).isSome = true) : V key :=
  (s key).get present

omit [DecidableEq K] in
theorem get_returns_registered_value (s : Store V) (key : K)
    (present : (s key).isSome = true) : s key = some (get s key present) := by
  cases value : s key <;> simp_all [get]

def Satisfied (s : Store V) (deps : Specification K) : Bool :=
  deps.all (fun key => (s key).isSome)

omit [DecidableEq K] in
theorem satisfaction_iff (s : Store V) (deps : Specification K) :
    Satisfied s deps = true ↔ ∀ key ∈ deps, (s key).isSome = true := by
  simp [Satisfied, List.all_eq_true]

omit [DecidableEq K] in
@[simp] theorem empty_specification_satisfied (s : Store V) : Satisfied s [] = true := rfl

omit [DecidableEq K] in
theorem satisfaction_append (s : Store V) (left right : Specification K) :
    Satisfied s (left ++ right) = (Satisfied s left && Satisfied s right) := by
  simp [Satisfied]

omit [DecidableEq K] in
theorem satisfied_dependency_present (s : Store V) (deps : Specification K)
    (satisfied : Satisfied s deps = true) (key : K) (member : key ∈ deps) :
    (s key).isSome = true := (satisfaction_iff s deps).mp satisfied key member

omit [DecidableEq K] in
theorem absent_dependency_blocks (s : Store V) (deps : Specification K)
    (key : K) (member : key ∈ deps) (absent : s key = none) :
    Satisfied s deps = false := by
  cases h : Satisfied s deps
  · rfl
  · have present := satisfied_dependency_present s deps h key member
    simp [absent] at present

/-- Totalization of Definition 13: occupied slots reject provisioning. -/
def provide (s : Store V) (key : K) (value : V key) : Option (Store V × (Store V → Store V)) :=
  if (s key).isSome then none
  else some (update s key (some value), fun later => update later key none)

theorem absent_provision_succeeds (s : Store V) (key : K) (value : V key)
    (absent : s key = none) :
    provide s key value = some (update s key (some value), fun later => update later key none) := by
  simp [provide, absent]

theorem occupied_provision_rejected (s : Store V) (key : K) (value prior : V key)
    (occupied : s key = some prior) : provide s key value = none := by
  simp [provide, occupied]

theorem provision_inverse_restores (s : Store V) (key : K) (value : V key)
    (absent : s key = none) :
    update (update s key (some value)) key none = s := by
  rw [update_update, ← absent, update_restores]

/-- A component may be removed while another component's distinct key stays
registered; this is a concrete spatial noninterference condition. -/
theorem independent_registration_removal (s : Store V) (first second : K)
    (left : V first) (right : V second) (different : first ≠ second)
    (absent : s first = none) :
    update (update (update s first (some left)) second (some right)) first none =
      update s second (some right) := by
  rw [← independent_updates_commute _ first second none (some right) different]
  rw [provision_inverse_restores s first left absent]

theorem successful_provision_is_strict (s : Store V) (key : K) (value : V key)
    (result : Store V × (Store V → Store V)) (success : provide s key value = some result) :
    result.2 result.1 = s := by
  unfold provide at success
  split at success
  · contradiction
  · rename_i free
    have absent : s key = none := by
      cases h : s key <;> simp_all
    cases Option.some.inj success
    exact provision_inverse_restores s key value absent

/-- A generic callback interface uses an inert rejected branch; success/failure
remains observable through `provide`. -/
def provisionEffect (key : K) (value : V key) : RevertibleEffects.Effect (Store V) := fun s =>
  match provide s key value with
  | none => (s, id)
  | some result => result

theorem provision_effect_strict (key : K) (value : V key) :
    RevertibleEffects.Strict (provisionEffect key value) := by
  intro s
  cases h : provide s key value with
  | none => simp [provisionEffect, h]
  | some result =>
    simpa [provisionEffect, h] using successful_provision_is_strict s key value result h

theorem registration_trace_recovers (s : Store V)
    (registrations : List ((key : K) × V key)) :
    let callbacks := registrations.map (fun entry => provisionEffect entry.1 entry.2)
    RevertibleEffects.recoverJournal
      (RevertibleEffects.recordRun (s, id) callbacks) = (s, id) := by
  apply RevertibleEffects.completed_prefix_recovers
  intro e member
  obtain ⟨entry, _, equal⟩ := List.mem_map.mp member
  subst e
  exact provision_effect_strict entry.1 entry.2

inductive Notification where
  | activating | deactivating | neutral
  deriving DecidableEq, Repr

/-- Definition 15.  Scheduling and dependent-before-provider cleanup are
separate operational obligations, not assumed consequences of this classifier. -/
def notify (before after : Store V) (deps : Specification K) : Notification :=
  match Satisfied before deps, Satisfied after deps with
  | false, true => .activating
  | true, false => .deactivating
  | _, _ => .neutral

omit [DecidableEq K] in
theorem notify_activating_iff (before after : Store V) (deps : Specification K) :
    notify before after deps = .activating ↔
      Satisfied before deps = false ∧ Satisfied after deps = true := by
  cases hb : Satisfied before deps <;> cases ha : Satisfied after deps <;> simp [notify, hb, ha]

omit [DecidableEq K] in
theorem notify_deactivating_iff (before after : Store V) (deps : Specification K) :
    notify before after deps = .deactivating ↔
      Satisfied before deps = true ∧ Satisfied after deps = false := by
  cases hb : Satisfied before deps <;> cases ha : Satisfied after deps <;> simp [notify, hb, ha]

omit [DecidableEq K] in
theorem notify_neutral_iff (before after : Store V) (deps : Specification K) :
    notify before after deps = .neutral ↔ Satisfied before deps = Satisfied after deps := by
  cases hb : Satisfied before deps <;> cases ha : Satisfied after deps <;> simp [notify, hb, ha]

omit [DecidableEq K] in
theorem unchanged_dependencies_neutral (before after : Store V) (deps : Specification K)
    (same : ∀ key ∈ deps, (before key).isSome = (after key).isSome) :
    notify before after deps = .neutral := by
  apply (notify_neutral_iff _ _ _).mpr
  unfold Satisfied
  induction deps with
  | nil => rfl
  | cons key rest ih =>
    simp only [List.all_cons]
    rw [same key (by simp), ih (fun k hk => same k (by simp [hk]))]

theorem withdrawn_requirement_deactivates (before : Store V) (deps : Specification K)
    (key : K) (member : key ∈ deps) (satisfied : Satisfied before deps = true) :
    notify before (update before key none) deps = .deactivating := by
  apply (notify_deactivating_iff _ _ _).mpr
  exact ⟨satisfied, absent_dependency_blocks _ _ key member (by simp)⟩

/-- A dependency snapshot retains values/identities, beyond presence alone. -/
def SameEpoch (before after : Store V) (deps : Specification K) : Prop :=
  ∀ key ∈ deps, before key = after key

omit [DecidableEq K] in
theorem epoch_equality_preserves_satisfaction (before after : Store V) (deps : Specification K)
    (same : SameEpoch before after deps) : Satisfied before deps = Satisfied after deps := by
  apply (notify_neutral_iff _ _ _).mp
  apply unchanged_dependencies_neutral
  intro key member
  exact congrArg Option.isSome (same key member)

omit [DecidableEq K] in
theorem changed_binding_changes_epoch (before after : Store V) (deps : Specification K)
    (key : K) (member : key ∈ deps) (changed : before key ≠ after key) :
    ¬ SameEpoch before after deps := fun same => changed (same key member)

/-- Realm identity is separate from type identity; key remains in the slot. -/
abbrev RealmStore (V : K → Type v) := (key : K) → Nat → Option (V key)
structure View (K : Type u) where
  realm : K → Nat

abbrev IsolatedContext (V : K → Type v) := View K × RealmStore V

def resolve (s : IsolatedContext V) (key : K) : Option (V key) := s.2 key (s.1.realm key)

def redirect (view : View K) (key : K) (realm : Nat) : View K :=
  ⟨fun other => if other = key then realm else view.realm other⟩

def isolate (s : IsolatedContext V) (key : K) (realm : Nat) : IsolatedContext V :=
  (redirect s.1 key realm, s.2)

@[simp] theorem isolate_resolves_selected_realm (s : IsolatedContext V) (key : K)
    (realm : Nat) : resolve (isolate s key realm) key = s.2 key realm := by
  simp [resolve, isolate, redirect]

theorem isolation_keeps_other_keys (s : IsolatedContext V) (key other : K) (realm : Nat)
    (different : other ≠ key) : resolve (isolate s key realm) other = resolve s other := by
  simp [resolve, isolate, redirect, different]

theorem isolation_preserves_store (s : IsolatedContext V) (key : K) (realm : Nat) :
    (isolate s key realm).2 = s.2 := rfl

theorem isolation_shadowing (s : IsolatedContext V) (key : K) (first second : Nat) :
    isolate (isolate s key first) key second = isolate s key second := by
  apply Prod.ext
  · apply congrArg View.mk
    funext other
    by_cases same : other = key <;> simp [isolate, redirect, same]
  · rfl

/-- Overwriting a realm is reversible by retaining the previous resolution. -/
def isolationEffect (key : K) (realm : Nat) : RevertibleEffects.Effect (IsolatedContext V) :=
  fun s => (isolate s key realm, fun later => isolate later key (s.1.realm key))

theorem isolation_effect_strict (key : K) (realm : Nat) :
    RevertibleEffects.Strict (isolationEffect (V := V) key realm) := by
  intro s
  simp only [isolationEffect]
  rw [isolation_shadowing]
  apply Prod.ext
  · apply congrArg View.mk
    funext other
    by_cases same : other = key <;> simp [isolate, redirect, same]
  · rfl

/-- A derived scope preserves its parent value; dropping the derived value
recovers the parent even when that key already has an enclosing realm. -/
structure Derived (α : Type u) where
  parent : α
  current : α

def deriveIsolation (s : IsolatedContext V) (key : K) (realm : Nat) : Derived (IsolatedContext V) :=
  ⟨s, isolate s key realm⟩

theorem derived_isolation_recovers_parent (s : IsolatedContext V) (key : K) (realm : Nat) :
    (deriveIsolation s key realm).parent = s := rfl

def updateRealm (s : RealmStore V) (key : K) (realm : Nat) (value : Option (V key)) :
    RealmStore V :=
  fun other => if h : other = key then
    h.symm ▸ (fun r => if r = realm then value else s key r)
  else s other

@[simp] theorem update_realm_same (s : RealmStore V) (key : K) (realm : Nat)
    (value : Option (V key)) : updateRealm s key realm value key realm = value := by
  simp [updateRealm]

theorem update_realm_other (s : RealmStore V) (key : K) (realm other : Nat)
    (value : Option (V key)) (different : other ≠ realm) :
    updateRealm s key realm value key other = s key other := by
  simp [updateRealm, different]

theorem update_realm_restores (s : RealmStore V) (key : K) (realm : Nat) :
    updateRealm s key realm (s key realm) = s := by
  funext other r
  by_cases same : other = key
  · subst other
    by_cases hr : r = realm <;> simp [updateRealm, hr]
  · simp [updateRealm, same]

theorem update_realm_update (s : RealmStore V) (key : K) (realm : Nat)
    (first second : Option (V key)) :
    updateRealm (updateRealm s key realm first) key realm second =
      updateRealm s key realm second := by
  funext other r
  by_cases same : other = key
  · subst other
    by_cases hr : r = realm <;> simp [updateRealm, hr]
  · simp [updateRealm, same]

/-- Capture the acquired slot at registration, just as a Rust service Effect
owns the installed implementation identity and realm. -/
def revokeCaptured (s : IsolatedContext V) (key : K) (acquiredRealm : Nat) : IsolatedContext V :=
  (s.1, updateRealm s.2 key acquiredRealm none)

theorem captured_revoke_ignores_later_redirection (s : IsolatedContext V)
    (key : K) (acquired newRealm : Nat) :
    (revokeCaptured (isolate s key newRealm) key acquired).2 =
      (revokeCaptured s key acquired).2 := rfl

theorem captured_revoke_preserves_other_realm (s : IsolatedContext V)
    (key : K) (acquired other : Nat) (different : other ≠ acquired) :
    (revokeCaptured s key acquired).2 key other = s.2 key other := by
  exact update_realm_other _ _ _ _ _ different

/-- The registered callback captures the acquired realm.  This definition also
makes rejection of an occupied slot inert, as `provisionEffect` does. -/
def isolatedProvisionEffect (key : K) (value : V key) :
    RevertibleEffects.Effect (IsolatedContext V) := fun s =>
  let acquired := s.1.realm key
  if (s.2 key acquired).isSome then (s, id)
  else ((s.1, updateRealm s.2 key acquired (some value)),
        fun later => revokeCaptured later key acquired)

theorem isolated_provision_effect_strict (key : K) (value : V key) :
    RevertibleEffects.Strict (isolatedProvisionEffect key value) := by
  intro s
  dsimp only [isolatedProvisionEffect]
  split
  · rfl
  · rename_i free
    have absent : s.2 key (s.1.realm key) = none := by
      cases h : s.2 key (s.1.realm key) <;> simp_all
    simp only [revokeCaptured]
    rw [update_realm_update, ← absent, update_realm_restores]

/-- Definition 17's written use of the later realm in a disposer changes its
target after redirection.  The acquired slot and the new slot can both exist. -/
theorem current_realm_disposer_counterexample :
    let s : IsolatedContext (fun _ : Unit => Nat) :=
      (⟨fun _ => 0⟩, fun _ realm => if realm = 0 then some 10 else some 20)
    let redirected := isolate s () 1
    let captured := revokeCaptured redirected () 0
    let current := revokeCaptured redirected () (redirected.1.realm ())
    captured.2 () 0 = none ∧ captured.2 () 1 = some 20 ∧
      current.2 () 0 = some 10 ∧ current.2 () 1 = none := by decide

/-- Metadata semantics are supplied per key, matching typed Rust metadata. -/
structure MetadataAlgebra (M : Type u) where
  empty : M
  merge : M → M → M
  associative : ∀ a b c, merge (merge a b) c = merge a (merge b c)
  left_identity : ∀ a, merge empty a = a
  right_identity : ∀ a, merge a empty = a

variable {M : K → Type w}

abbrev Metadata (M : K → Type w) := (key : K) → M key
abbrev Providers (M : K → Type w) (V : K → Type v) := (key : K) → Option (M key → V key)

structure Intercepted (M : K → Type w) (V : K → Type v) where
  metadata : Metadata M
  providers : Providers M V

/-- A finite Definition 18 specification carries the metadata used by each
declared dependency; satisfaction observes its underlying key set. -/
abbrev InterceptedSpecification (M : K → Type w) := List ((key : K) × M key)

def InterceptedSatisfied (s : Intercepted M V) (deps : InterceptedSpecification M) : Bool :=
  Satisfied s.providers (deps.map Sigma.fst)

omit [DecidableEq K] in
theorem intercepted_satisfaction_iff (s : Intercepted M V) (deps : InterceptedSpecification M) :
    InterceptedSatisfied s deps = true ↔
      ∀ entry ∈ deps, (s.providers entry.1).isSome = true := by
  simp [InterceptedSatisfied, Satisfied, List.all_eq_true]

def access (algebra : ∀ key, MetadataAlgebra (M key)) (s : Intercepted M V)
    (key : K) (declared : M key) : Option (V key) :=
  (s.providers key).map (fun provider => provider ((algebra key).merge declared (s.metadata key)))

def intercept (algebra : ∀ key, MetadataAlgebra (M key)) (s : Intercepted M V)
    (key : K) (value : M key) : Intercepted M V :=
  { s with metadata := fun other => if h : other = key then
      h.symm ▸ (algebra key).merge (s.metadata key) value else s.metadata other }

/-- Definition 19's saved metadata makes interception a strict effect even
when the merge operation itself has no inverse. -/
def interceptionEffect (algebra : ∀ key, MetadataAlgebra (M key))
    (key : K) (value : M key) : RevertibleEffects.Effect (Intercepted M V) := fun s =>
  (intercept algebra s key value, fun later =>
    { later with metadata := fun other => if h : other = key then
        h.symm ▸ s.metadata key else later.metadata other })

theorem interception_effect_strict (algebra : ∀ key, MetadataAlgebra (M key))
    (key : K) (value : M key) :
    RevertibleEffects.Strict (interceptionEffect (V := V) algebra key value) := by
  intro s
  cases s with
  | mk metadata providers =>
    simp only [interceptionEffect, intercept]
    congr 1
    funext other
    by_cases same : other = key
    · subst other; simp
    · simp [same]

/-- Registering a metadata-sensitive provider uses the same owned registration
effect as an ordinary typed dependency. -/
def providerEffect (key : K) (provider : M key → V key) :
    RevertibleEffects.Effect (Intercepted M V) := fun s =>
  let result := provisionEffect key provider s.providers
  ({ s with providers := result.1 },
    fun later => { later with providers := result.2 later.providers })

theorem provider_effect_strict (key : K) (provider : M key → V key) :
    RevertibleEffects.Strict (providerEffect key provider) := by
  intro s
  simp only [providerEffect]
  rw [provision_effect_strict key provider s.providers]

theorem intercept_preserves_providers (algebra : ∀ key, MetadataAlgebra (M key))
    (s : Intercepted M V) (key : K) (value : M key) :
    (intercept algebra s key value).providers = s.providers := rfl

omit [DecidableEq K] in
theorem access_merges_declared_then_context (algebra : ∀ key, MetadataAlgebra (M key))
    (s : Intercepted M V) (key : K) (declared : M key) (provider : M key → V key)
    (present : s.providers key = some provider) :
    access algebra s key declared =
      some (provider ((algebra key).merge declared (s.metadata key))) := by
  simp [access, present]

theorem intercept_same_key (algebra : ∀ key, MetadataAlgebra (M key))
    (s : Intercepted M V) (key : K) (first second : M key) :
    intercept algebra (intercept algebra s key first) key second =
      intercept algebra s key ((algebra key).merge first second) := by
  cases s with
  | mk metadata providers =>
    simp only [intercept]
    congr 1
    funext other
    by_cases same : other = key
    · subst other; simp [intercept, (algebra key).associative]
    · simp [intercept, same]

theorem interception_keeps_other_access (algebra : ∀ key, MetadataAlgebra (M key))
    (s : Intercepted M V) (key other : K) (value : M key) (declared : M other)
    (different : other ≠ key) :
    access algebra (intercept algebra s key value) other declared = access algebra s other declared := by
  simp [access, intercept, different]

/-- A concrete right-biased scalar metadata field.  Associativity alone does
not imply precedence; precedence is proved for this actual merge operation. -/
def override (left right : Option α) : Option α := right.or left

theorem override_associative (a b c : Option α) :
    override (override a b) c = override a (override b c) := by
  cases a <;> cases b <;> cases c <;> rfl

theorem context_scalar_takes_priority (declared : Option α) (context : α) :
    override declared (some context) = some context := rfl

def overrideAlgebra (α : Type u) : MetadataAlgebra (Option α) :=
  ⟨none, override, override_associative,
   fun a => by cases a <;> rfl, fun _ => rfl⟩

end Cordis.ReactiveCoeffects
