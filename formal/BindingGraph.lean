import Deployment

/-!
Dependency edges assembled by Manager::select_instances, before the already
proved worklist traversal. Interface keys include the major version. Provider
facts come from the current component or its incoming replacement; committed
bindings are distinguished from unresolved injections. All lists are arbitrary
finite snapshots. Object extraction under the catalog/context locks is a
separate source boundary, recorded by the executable bridge.
-/

namespace Cordis.BindingGraph

structure Consumer where
  owner : Nat
  key : String
  minor : Nat
  realm : Nat
  /-- none: unresolved; some none: a root service; some (some owner): an instance. -/
  committed : Option (Option Nat)
  deriving DecidableEq

structure Provider where
  owner : Nat
  key : String
  minor : Nat
  inherited : Nat
  space : Nat
  deriving DecidableEq

structure Snapshot where
  parents : List (Nat × Nat)
  consumers : List Consumer
  current : List Provider
  incoming : List Provider
  replaced : List Nat
  named : Nat → Option Nat

def providers (s : Snapshot) : List Provider :=
  s.incoming ++ s.current.filter (fun p => p.owner ∉ s.replaced)

def compatible (named : Nat → Option Nat) (p : Provider) (c : Consumer) : Bool :=
  p.key == c.key && decide (c.minor ≤ p.minor) &&
    (c.realm == p.inherited || named c.realm == some p.space)

def consumerEdges (s : Snapshot) (c : Consumer) : List (Nat × Nat) :=
  match c.committed with
  | some (some owner) => [(owner, c.owner)]
  | some none => []
  | none => ((providers s).filter (fun p => compatible s.named p c)).map
      (fun p => (p.owner, c.owner))

def pairs (s : Snapshot) : List (Nat × Nat) :=
  s.parents ++ s.consumers.flatMap (consumerEdges s)

def Edge (s : Snapshot) (source target : Nat) : Prop :=
  (source, target) ∈ s.parents ∨
  ∃ c ∈ s.consumers, c.owner = target ∧
    (c.committed = some (some source) ∨
     c.committed = none ∧ ∃ p ∈ providers s,
       p.owner = source ∧ p.key = c.key ∧ c.minor ≤ p.minor ∧
       (c.realm = p.inherited ∨ s.named c.realm = some p.space))

theorem provider_membership (s : Snapshot) (p : Provider) :
    p ∈ providers s ↔ p ∈ s.incoming ∨ p ∈ s.current ∧ p.owner ∉ s.replaced := by
  simp [providers]

theorem replaced_current_excluded (s : Snapshot) (p : Provider)
    (replaced : p.owner ∈ s.replaced) (absent : p ∉ s.incoming) :
    p ∉ providers s := by simp [provider_membership, replaced, absent]

theorem compatibility_spec (named : Nat → Option Nat) (p : Provider) (c : Consumer) :
    compatible named p c = true ↔ p.key = c.key ∧ c.minor ≤ p.minor ∧
      (c.realm = p.inherited ∨ named c.realm = some p.space) := by
  simp [compatible, and_assoc]

/-- Restricted source predicate language. The certificate generator parses the
actual Rust closure into this syntax; it never supplies a proposition as input. -/
inductive Predicate where
  | minorAtLeast | sameRealm | namedNamespace
  | and (left right : Predicate)
  | or (left right : Predicate)
  deriving DecidableEq, Repr

def Predicate.eval (named : Nat → Option Nat) (p : Provider) (c : Consumer) :
    Predicate → Bool
  | .minorAtLeast => decide (c.minor ≤ p.minor)
  | .sameRealm => c.realm == p.inherited
  | .namedNamespace => named c.realm == some p.space
  | .and left right => left.eval named p c && right.eval named p c
  | .or left right => left.eval named p c || right.eval named p c

def candidateProgram : Predicate :=
  .and .minorAtLeast (.or .sameRealm .namedNamespace)

theorem candidate_program_spec (named : Nat → Option Nat) (p : Provider) (c : Consumer) :
    candidateProgram.eval named p c = true ↔
      c.minor ≤ p.minor ∧ (c.realm = p.inherited ∨ named c.realm = some p.space) := by
  simp [candidateProgram, Predicate.eval]

theorem candidate_program_refines_compatibility (named : Nat → Option Nat)
    (p : Provider) (c : Consumer) :
    (p.key == c.key && candidateProgram.eval named p c) = compatible named p c := by
  simp [candidateProgram, Predicate.eval, compatible, Bool.and_assoc]

theorem consumer_edge_spec (s : Snapshot) (c : Consumer) (source target : Nat) :
    (source, target) ∈ consumerEdges s c ↔ c.owner = target ∧
      (c.committed = some (some source) ∨ c.committed = none ∧
        ∃ p ∈ providers s, p.owner = source ∧ p.key = c.key ∧
          c.minor ≤ p.minor ∧
          (c.realm = p.inherited ∨ s.named c.realm = some p.space)) := by
  cases h : c.committed with
  | some owner => cases owner <;> simp [consumerEdges, h, eq_comm, and_comm]
  | none =>
    simp only [consumerEdges, h, List.mem_map, List.mem_filter, Prod.mk.injEq,
      compatibility_spec, reduceCtorEq, false_or, true_and]
    constructor
    · rintro ⟨p, ⟨member, key, minor, realm⟩, owner, targetEq⟩
      exact ⟨targetEq, p, member, owner, key, minor, realm⟩
    · rintro ⟨targetEq, p, member, owner, key, minor, realm⟩
      exact ⟨p, ⟨member, key, minor, realm⟩, owner, targetEq⟩

theorem constructed_edges_exact (s : Snapshot) (source target : Nat) :
    (source, target) ∈ pairs s ↔ Edge s source target := by
  simp [pairs, Edge, consumer_edge_spec]

theorem committed_binding_seals_candidates (s t : Snapshot) (c : Consumer)
    (owner : Option Nat) (bound : c.committed = some owner) :
    consumerEdges s c = consumerEdges t c := by
  cases owner <;> simp [consumerEdges, bound]

theorem bound_root_service_has_no_instance_edge (s : Snapshot) (c : Consumer)
    (bound : c.committed = some none) : consumerEdges s c = [] := by
  simp [consumerEdges, bound]

theorem incompatible_key_has_no_candidate (named : Nat → Option Nat)
    (p : Provider) (c : Consumer) (key : p.key ≠ c.key) :
    compatible named p c = false := by simp [compatible, key]

theorem older_minor_has_no_candidate (named : Nat → Option Nat)
    (p : Provider) (c : Consumer) (minor : p.minor < c.minor) :
    compatible named p c = false := by simp [compatible, Nat.not_le.mpr minor]

def targets (es : List (Nat × Nat)) (source : Nat) : List Nat :=
  (es.filter (fun edge => edge.1 == source)).map Prod.snd

theorem target_membership (es : List (Nat × Nat)) (source target : Nat) :
    target ∈ targets es source ↔ (source, target) ∈ es := by
  simp only [targets, List.mem_map, List.mem_filter, beq_iff_eq]
  constructor
  · rintro ⟨⟨a, b⟩, ⟨member, rfl⟩, rfl⟩
    exact member
  · intro member
    exact ⟨(source, target), ⟨member, rfl⟩, rfl⟩

def rows (es : List (Nat × Nat)) (keys : List Nat) : Deployment.Adjacency Nat :=
  keys.map (fun key => (key, targets es key))

theorem rows_lookup (es : List (Nat × Nat)) (keys : List Nat) (key : Nat) :
    Deployment.neighbors (rows es keys) key = if key ∈ keys then targets es key else [] := by
  induction keys with
  | nil => simp [rows, Deployment.neighbors, Deployment.takeNeighbors]
  | cons first rest ih =>
    by_cases same : first = key
    · subst first
      simp [rows, Deployment.neighbors, Deployment.takeNeighbors]
    · simpa [rows, Deployment.neighbors, Deployment.takeNeighbors, same, Ne.symm same] using ih

def adjacency (es : List (Nat × Nat)) : Deployment.Adjacency Nat :=
  rows es (es.map Prod.fst)

theorem adjacency_membership (es : List (Nat × Nat)) (source target : Nat) :
    target ∈ Deployment.neighbors (adjacency es) source ↔ (source, target) ∈ es := by
  rw [adjacency, rows_lookup]
  by_cases member : source ∈ (es.map Prod.fst)
  · simp [member, target_membership]
  · simp only [member, if_false, List.not_mem_nil, false_iff]
    intro pair
    apply member
    simpa using (show source ∈ es.map Prod.fst from List.mem_map.mpr ⟨(source,target), pair, rfl⟩)

inductive Reachable (s : Snapshot) (roots : List Nat) : Nat → Prop where
  | root {node} : node ∈ roots → Reachable s roots node
  | edge {source target} : Reachable s roots source → Edge s source target → Reachable s roots target

theorem reachability_refines (s : Snapshot) (roots : List Nat) (node : Nat) :
    Deployment.Reachable (Deployment.neighbors (adjacency (pairs s))) roots node ↔
      Reachable s roots node := by
  constructor
  · intro h
    induction h with
    | root member => exact .root member
    | edge prior edge ih =>
      exact .edge ih ((constructed_edges_exact _ _ _).mp ((adjacency_membership _ _ _).mp edge))
  · intro h
    induction h with
    | root member => exact .root member
    | edge prior edge ih =>
      exact .edge ih ((adjacency_membership _ _ _).mpr ((constructed_edges_exact _ _ _).mpr edge))

theorem selection_from_source_facts (s : Snapshot) (roots : List Nat) (node : Nat) :
    node ∈ (Deployment.select (adjacency (pairs s)) roots).selected ↔ Reachable s roots node := by
  rw [Deployment.selected_iff_reachable, reachability_refines]

theorem selected_existing_instances_exact (s : Snapshot) (roots : List Nat)
    (catalog : Nat → Bool) (node : Nat) :
    node ∈ Deployment.returned (adjacency (pairs s)) roots catalog ↔
      Reachable s roots node ∧ catalog node = true := by
  rw [Deployment.returned_iff_reachable_and_present, reachability_refines]

theorem constructed_graph_terminates (s : Snapshot) (roots : List Nat) :
    (Deployment.select (adjacency (pairs s)) roots).pending = [] :=
  Deployment.selection_terminates _ _

end Cordis.BindingGraph
