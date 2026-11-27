# Deployment instance selection

`Deployment.lean` formalizes the worklist at the end of
`Manager::select_instances` in
`crates/cordis-supervisor/src/manager/transition.rs`. The input graph comes from
the manager's snapshot of committed dependency bindings, parent-child relations
and dependencies that candidate providers can satisfy. Edges point from a
provider or parent to a dependent instance.

The state records selected names, pending names and the remaining adjacency map.
Each step pops a pending name. An already selected name is discarded. A fresh
name enters the selected set, consumes its adjacency row and pushes its dependent
names. The abstract lists use pop order, so they correspond to reversing the
Rust `Vec` representation. The proof uses list membership and therefore applies
to every root order and adjacency order.

The invariant has five components: selected names are reachable, pending names
are reachable, every root remains selected or pending, every edge from a selected
name remains selected or pending, and every unselected name retains its original
adjacency row. `take_other_neighbors` proves the last component for each consumed
row. The step theorem preserves all five components and induction extends it to
an arbitrary execution length.

When the pending stack empties, the selected names contain all roots and are
closed under every graph edge. Reachability induction then proves completeness.
Together with soundness, `selected_iff_reachable` characterizes the result exactly.
`selection_is_minimal` proves that every other root-containing, edge-closed set
contains this result. The selected set also remains free of duplicates.

The termination measure is the pending-stack length plus the sum of all remaining
adjacency-list lengths. Popping a duplicate decreases the first term by one.
Expanding a fresh name removes its outgoing list from the map and adds that same
list to the pending stack, while consuming the popped name. Each nonterminal
step decreases the total by exactly one. The initial measure therefore bounds
the iteration count by the number of root occurrences plus the number of stored
edge occurrences. This bound applies to arbitrarily large finite graphs,
including cycles, duplicate roots and self edges.

The final Rust code sorts selected names and removes names absent from the
catalog snapshot. `returned_iff_reachable_and_present` proves the corresponding
membership condition. The graph-construction rules, concrete hashing, sorting,
snapshot synchronization and `Arc` representation remain Rust refinement
boundaries. The machine-readable bridge records these boundaries and the real
manager test entrypoint. The existing Python graph enumeration remains useful
as a small independent regression check; the Lean proof has no three-node bound.
