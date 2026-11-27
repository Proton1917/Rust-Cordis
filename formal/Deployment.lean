import Std

/-!
The pending/selected worklist in `Manager::select_instances`. Adjacency is the
finite snapshot constructed from committed bindings, parent relations and
candidate providers. Its construction is a separate implementation obligation.
The model retains `dependents.remove`: every expansion consumes its adjacency
row. This provides a termination measure with no bound on graph size.
-/

namespace Cordis.Deployment

variable {α : Type} [DecidableEq α]

abbrev Adjacency (α : Type) := List (α × List α)

def takeNeighbors (edges : Adjacency α) (key : α) : List α × Adjacency α :=
  match edges with
  | [] => ([], [])
  | (name, next) :: rest =>
    if name = key then (next, rest)
    else
      let found := takeNeighbors rest key
      (found.1, (name, next) :: found.2)

def neighbors (edges : Adjacency α) (key : α) : List α := (takeNeighbors edges key).1

def edgeWeight : Adjacency α → Nat
  | [] => 0
  | (_, next) :: rest => next.length + edgeWeight rest

theorem take_neighbors_weight (edges : Adjacency α) (key : α) :
    (takeNeighbors edges key).1.length + edgeWeight (takeNeighbors edges key).2 =
      edgeWeight edges := by
  induction edges with
  | nil => rfl
  | cons edge rest ih =>
    rcases edge with ⟨name, next⟩
    by_cases same : name = key
    · simp [takeNeighbors, same, edgeWeight]
    · simp [takeNeighbors, same, edgeWeight]
      omega

theorem take_other_neighbors (edges : Adjacency α) (key other : α)
    (different : other ≠ key) :
    neighbors (takeNeighbors edges key).2 other = neighbors edges other := by
  induction edges with
  | nil => rfl
  | cons edge rest ih =>
    rcases edge with ⟨name, next⟩
    by_cases same : name = key
    · subst name
      simp [takeNeighbors, neighbors, Ne.symm different]
    · by_cases otherSame : name = other
      · simp [takeNeighbors, neighbors, same, otherSame, different]
      · simpa [takeNeighbors, neighbors, same, otherSame] using ih

structure State (α : Type) where
  selected : List α := []
  pending : List α
  remaining : Adjacency α

def initial (edges : Adjacency α) (roots : List α) : State α :=
  { pending := roots, remaining := edges }

def step (s : State α) : State α :=
  match s.pending with
  | [] => s
  | name :: rest =>
    if name ∈ s.selected then { s with pending := rest }
    else
      let found := takeNeighbors s.remaining name
      { selected := name :: s.selected, pending := found.1 ++ rest, remaining := found.2 }

def run : Nat → State α → State α
  | 0, s => s
  | fuel + 1, s => run fuel (step s)

def measure (s : State α) : Nat := s.pending.length + edgeWeight s.remaining

theorem nonempty_step_decreases (s : State α) (nonempty : s.pending ≠ []) :
    measure (step s) + 1 = measure s := by
  cases pending : s.pending with
  | nil => exact False.elim (nonempty pending)
  | cons name rest =>
    simp only [step, pending]
    split
    · simp [measure, pending]
      omega
    · have conservation := take_neighbors_weight s.remaining name
      simp [measure, pending]
      omega

theorem empty_worklist_stable (s : State α) (empty : s.pending = []) : step s = s := by
  simp [step, empty]

theorem run_empty_stable (fuel : Nat) (s : State α) (empty : s.pending = []) :
    run fuel s = s := by
  induction fuel with
  | zero => rfl
  | succ fuel ih => simpa [run, empty_worklist_stable s empty] using ih

/-- The source loop needs at most roots.length plus the sum of adjacency-list lengths.
The bound counts outgoing edge occurrences, including duplicates and self edges. -/
theorem finite_graph_terminates (fuel : Nat) (s : State α) (enough : measure s ≤ fuel) :
    (run fuel s).pending = [] := by
  induction fuel generalizing s with
  | zero =>
    have zeroLength : s.pending.length = 0 := by unfold measure at enough; omega
    simpa [run] using List.length_eq_zero_iff.mp zeroLength
  | succ fuel ih =>
    by_cases empty : s.pending = []
    · simpa [run_empty_stable _ s empty] using empty
    · have decrease := nonempty_step_decreases s empty
      exact ih (step s) (by omega)

inductive Reachable (graph : α → List α) (roots : List α) : α → Prop where
  | root {node} : node ∈ roots → Reachable graph roots node
  | edge {source target} : Reachable graph roots source → target ∈ graph source → Reachable graph roots target

def Covered (s : State α) (node : α) : Prop := node ∈ s.selected ∨ node ∈ s.pending

def WorkInvariant (edges : Adjacency α) (roots : List α) (s : State α) : Prop :=
  (∀ node ∈ s.selected, Reachable (neighbors edges) roots node) ∧
  (∀ node ∈ s.pending, Reachable (neighbors edges) roots node) ∧
  (∀ node ∈ roots, Covered s node) ∧
  (∀ source ∈ s.selected, ∀ target ∈ neighbors edges source, Covered s target) ∧
  (∀ node, node ∉ s.selected → neighbors s.remaining node = neighbors edges node)

theorem initial_work_invariant (edges : Adjacency α) (roots : List α) :
    WorkInvariant edges roots (initial edges roots) := by
  simp only [WorkInvariant, initial, List.not_mem_nil, false_implies, implies_true, true_and]
  exact ⟨fun _ member => Reachable.root member,
    fun _ member => Or.inr member, trivial⟩

theorem covered_after_step (s : State α) (node : α) (h : Covered s node) :
    Covered (step s) node := by
  cases pending : s.pending with
  | nil => simpa [step, pending] using h
  | cons name rest =>
    rcases h with selected | queued
    · simp only [step, pending]
      split
      · exact Or.inl selected
      · exact Or.inl (List.mem_cons_of_mem _ selected)
    · rw [pending, List.mem_cons] at queued
      rcases queued with same | queued
      · subst node
        simp only [step, pending]
        split
        · exact Or.inl (by assumption)
        · exact Or.inl (List.mem_cons_self)
      · simp only [step, pending]
        split
        · exact Or.inr queued
        · exact Or.inr (List.mem_append_right _ queued)

theorem work_step_preserves (edges : Adjacency α) (roots : List α) (s : State α)
    (h : WorkInvariant edges roots s) : WorkInvariant edges roots (step s) := by
  rcases h with ⟨selectedReach, pendingReach, rootsCovered, closed, remaining⟩
  cases pending : s.pending with
  | nil =>
    simpa [step, pending, WorkInvariant] using
      (show WorkInvariant edges roots s from
        ⟨selectedReach, pendingReach, rootsCovered, closed, remaining⟩)
  | cons name rest =>
    have nameReach := pendingReach name (by simp [pending])
    by_cases seen : name ∈ s.selected
    · simp only [step, pending, if_pos seen, WorkInvariant]
      refine ⟨selectedReach, ?_, ?_, ?_, remaining⟩
      · intro node member
        exact pendingReach node (by simp [pending, member])
      · intro node member
        have kept := covered_after_step s node (rootsCovered node member)
        simpa [step, pending, seen] using kept
      · intro source member target edge
        have kept := covered_after_step s target (closed source member target edge)
        simpa [step, pending, seen] using kept
    · have expansion : (takeNeighbors s.remaining name).1 = neighbors edges name :=
        remaining name seen
      simp only [step, pending, if_neg seen, WorkInvariant]
      refine ⟨?_, ?_, ?_, ?_, ?_⟩
      · intro node member
        rcases List.mem_cons.mp member with same | old
        · subst node; exact nameReach
        · exact selectedReach node old
      · intro node member
        rcases List.mem_append.mp member with neighbor | old
        · exact Reachable.edge nameReach (expansion ▸ neighbor)
        · exact pendingReach node (by simp [pending, old])
      · intro node member
        have kept := covered_after_step s node (rootsCovered node member)
        simpa [step, pending, seen] using kept
      · intro source member target edge
        rcases List.mem_cons.mp member with same | old
        · subst source
          exact Or.inr (List.mem_append_left _ (expansion.symm ▸ edge))
        · have kept := covered_after_step s target (closed source old target edge)
          simpa [step, pending, seen] using kept
      · intro node unselected
        have different : node ≠ name := by
          intro same
          exact unselected (List.mem_cons.mpr (Or.inl same))
        have old : node ∉ s.selected := fun member => unselected (List.mem_cons_of_mem _ member)
        exact (take_other_neighbors s.remaining name node different).trans (remaining node old)

theorem work_trace_preserves (edges : Adjacency α) (roots : List α)
    (fuel : Nat) (s : State α) (h : WorkInvariant edges roots s) :
    WorkInvariant edges roots (run fuel s) := by
  induction fuel generalizing s with
  | zero => exact h
  | succ fuel ih => exact ih (step s) (work_step_preserves edges roots s h)

theorem selection_sound (edges : Adjacency α) (roots : List α) (fuel : Nat)
    (node : α) (selected : node ∈ (run fuel (initial edges roots)).selected) :
    Reachable (neighbors edges) roots node :=
  (work_trace_preserves edges roots fuel _ (initial_work_invariant edges roots)).1 node selected

theorem selection_complete_when_finished (edges : Adjacency α) (roots : List α)
    (fuel : Nat) (finished : (run fuel (initial edges roots)).pending = [])
    (node : α) (reachable : Reachable (neighbors edges) roots node) :
    node ∈ (run fuel (initial edges roots)).selected := by
  have invariant := work_trace_preserves edges roots fuel _ (initial_work_invariant edges roots)
  have coveredSelected : ∀ node, Covered (run fuel (initial edges roots)) node →
      node ∈ (run fuel (initial edges roots)).selected := by
    intro node covered
    simpa [Covered, finished] using covered
  induction reachable with
  | root member => exact coveredSelected _ (invariant.2.2.1 _ member)
  | edge prior neighbor ih => exact coveredSelected _ (invariant.2.2.2.1 _ ih _ neighbor)

omit [DecidableEq α] in
theorem reachable_is_least_closed (graph : α → List α) (roots : List α) (closed : α → Prop)
    (containsRoots : ∀ node ∈ roots, closed node)
    (containsEdges : ∀ source, closed source → ∀ target ∈ graph source, closed target)
    (node : α) (reachable : Reachable graph roots node) : closed node := by
  induction reachable with
  | root member => exact containsRoots _ member
  | edge prior neighbor ih => exact containsEdges _ ih _ neighbor

def select (edges : Adjacency α) (roots : List α) : State α :=
  run (measure (initial edges roots)) (initial edges roots)

theorem selection_terminates (edges : Adjacency α) (roots : List α) :
    (select edges roots).pending = [] :=
  finite_graph_terminates _ _ (Nat.le_refl _)

/-- Arbitrary graph size, root multiplicity, cycles and disconnected vertices. -/
theorem selected_iff_reachable (edges : Adjacency α) (roots : List α) (node : α) :
    node ∈ (select edges roots).selected ↔ Reachable (neighbors edges) roots node := by
  constructor
  · exact selection_sound edges roots _ node
  · exact selection_complete_when_finished edges roots _ (selection_terminates edges roots) node

theorem selection_is_minimal (edges : Adjacency α) (roots : List α) (closed : α → Prop)
    (containsRoots : ∀ node ∈ roots, closed node)
    (containsEdges : ∀ source, closed source → ∀ target ∈ neighbors edges source, closed target)
    (node : α) (selected : node ∈ (select edges roots).selected) : closed node :=
  reachable_is_least_closed _ roots closed containsRoots containsEdges node
    ((selected_iff_reachable edges roots node).mp selected)

theorem step_preserves_nodup (s : State α) (h : s.selected.Nodup) : (step s).selected.Nodup := by
  cases pending : s.pending with
  | nil => simpa [step, pending] using h
  | cons name rest =>
    by_cases seen : name ∈ s.selected <;> simp [step, pending, seen, List.nodup_cons, h]

theorem run_preserves_nodup (fuel : Nat) (s : State α) (h : s.selected.Nodup) :
    (run fuel s).selected.Nodup := by
  induction fuel generalizing s with
  | zero => exact h
  | succ fuel ih => exact ih (step s) (step_preserves_nodup s h)

theorem selected_has_no_duplicates (edges : Adjacency α) (roots : List α) :
    (select edges roots).selected.Nodup :=
  run_preserves_nodup _ _ (by simp [initial])

def returned (edges : Adjacency α) (roots : List α) (inCatalog : α → Bool) : List α :=
  (select edges roots).selected.filter inCatalog

/-- The final Rust filter_map removes selected names absent from the snapshot.
Sorting changes output order but preserves this membership characterization. -/
theorem returned_iff_reachable_and_present (edges : Adjacency α) (roots : List α)
    (inCatalog : α → Bool) (node : α) :
    node ∈ returned edges roots inCatalog ↔
      Reachable (neighbors edges) roots node ∧ inCatalog node = true := by
  simp [returned, selected_iff_reachable]

end Cordis.Deployment
