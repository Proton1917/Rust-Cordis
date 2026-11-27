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
membership condition. `BindingGraph.lean` constructs the graph from the typed source facts described below. Concrete hashing, sorting, snapshot synchronization and `Arc` representation remain implementation contracts. The machine-readable bridge records these boundaries and the real
manager test entrypoint. The existing Python graph enumeration remains useful
as a small independent regression check; the Lean proof has no three-node bound.

## Constructing dependency edges

`BindingGraph.Snapshot` holds parent edges, consumers, current/incoming providers, replacement names and named-realm mappings. Providers from an incoming replacement suppress the current provider with the same instance identity. Each unresolved consumer accepts candidates only when the full major-version key matches, the offered minor version meets its requirement, and the provider inherits the consumer realm or belongs to its named namespace.

Committed bindings have three distinct representations: unresolved; a bound root service with no instance generation; and a bound instance generation. The latter creates one provider-to-consumer edge. A root binding seals candidate fallback while creating no instance edge. This distinction follows the complete production match branch.

`constructed_edges_exact` proves edge membership exactly matches those rules. `reachability_refines` and `selection_from_source_facts` connect the constructed edges to the arbitrary finite worklist proof. `selected_existing_instances_exact` includes the final catalog filter; `constructed_graph_terminates` preserves termination for all finite snapshots.

## Source predicate and complete builder execution

`check_binding_graph.py` extracts the candidate closure and parses three bound comparisons, boolean conjunction/disjunction and parentheses into `BindingGraph.Predicate`. A generated Lean certificate checks this source-derived syntax equals `candidateProgram`; `candidate_program_spec` then supplies its minor/realm specification for arbitrary parameters. The parser rejects unsupported syntax. The trusted boundary contains this restricted parser, source extraction, primitive comparisons, the u32 projection and the compiler; it is not a general Rust parser or memory model.

The checker also compiles the entire `Manager::select_instances` function body unchanged over stable snapshot APIs. Its 32,768 inputs cover current and fallback components, a missing fallback, parent chains, replacements, key/minor/realm compatibility, dynamic type failure, root and instance commitments, sibling plugin declarations and roots. Six source mutations alter minor direction, named-realm logic, replacement exclusion, committed-minor checking, parent direction and root-binding fallback; all are rejected. An AST operator mutation fails the Lean certificate, and unsupported predicate syntax fails parsing.

Context resolution, interface parsing and catalog locks supply the explicitly recorded environment facts. Input hashes are checked before and after execution; receipts use running/passed/failed states. Run `python3 formal/check_binding_graph.py --evidence target/binding-graph-certificate.json` to reproduce the bridge.
