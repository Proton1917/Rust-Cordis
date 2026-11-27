# Executor, native and routing ownership

These models describe ownership transfers in the implementation at repository
baseline `6d33702c8efb2025ddc9623f30e1df986b935f7d`. The bridge inventory in
`formal/bridges/execution.json` identifies the production paths, theorem names,
executable integration evidence and remaining assumptions. Lean checks the
transition-system proofs. Rust tests exercise the implementation mapping.

## Task submission and serial cleanup

`Execution.TaskState` counts one submitted task in five places: the caller, the
shared `Option<Task>` slot, the worker after its first poll, the owning effect's
suspended slot, or the completed state. `Kernel::submit` creates the shared slot;
the wrapper's first poll takes it. If submission panics, `Arc::try_unwrap` can
return the unpolled task only after the host has dropped the wrapper. A wrapper
retained by the host keeps its task, including when the host panics after accepting
it. The model separates recovered rejection and retained rejection.

`task_trace_preserves` proves that every finite event sequence starting from one
task preserves one token and the relation between first polls and worker/completed
ownership. Its corollaries establish one first take and exclusive custody. The
double-rejection trace follows `Effect::submit_cleanup`: return to caller, retry,
return to caller, suspend with the owning effect, then resume the same task.
Cancellation and disposal-observer drops preserve custody.

The serial-cleanup model extends the existing `Effects` claimant model. The
claimant transfers the reverse of the registered cleanup stack. During execution,
the original list remains exactly the concatenation of completed operations, the
optional current operation and pending operations. Starting an operation moves
one element between partitions; every callback result moves that operation into
the completed partition and preserves all remaining work. The first recorded
error remains fixed. If an arbitrary trace reaches the terminal state, all
operations have completed in LIFO order. Each callback result decreases the
remaining-work count, including failures.

`claimed_cleanup_completes_registered_stack` composes the claimant's proved stack
handoff with this asynchronous execution theorem, using the existing
`Effects.claimed_steps_are_lifo` result directly.

These are safety results. Finite completion relies on the documented host contract
to keep polling accepted tasks, plus termination of the supplied callbacks. A
new continuation created by `continue_cleanup` has its own task token. The model
does not claim one task object for the entire lifetime of an effect.

## Native callbacks and library leases

`NativeOwnership.Callback` separates cancellation requests, synchronous polling,
future destruction and permit return. Its transition invariant proves that a live
native future retains its permit, a synchronous poll has a live future, and both
destruction and return occur at most once. This represents `Admitted` field order:
the pinned future is declared before the permit and its destructor finishes first.
`NativeTask::drop` requests abort; the callback's capacity remains held until the
native task is actually destroyed.

`Usage.held` records the byte charge of every live permit. Acquisition performs
the same active-count, byte-limit and checked-addition guards as
`Admission::acquire`. Release removes an owned item in arbitrary order. The trace
invariant equates counters to the ledger and preserves configured limits and the
machine-word bound. In particular, any owned permit has enough count and bytes
to cover its subtraction.

`NativeOwnership.Lease` tracks count, retirement, descriptor-release result,
unload policy, library presence, cache-copy presence and a ghost close counter.
Any modeled trace preserves the implication that a closed library is retired,
has zero counted leases, allows unloading and has a successful descriptor release.
Repeated reap operations close at most once. A live counted lease prevents reap;
a retired zero-count lease rejects acquisition. Failed descriptor release and the
`never` policy retain the image. Quiet retirement releases the cache copy under
either image policy.

This lease model describes completed operations. The source reads `retired` from
inside the `count.fetch_update` closure, so two distinct atomic objects are
involved. Their Rust memory-model correspondence requires an additional
linearization argument. Legal ABI cloning holds an existing counted lease until
its new lease has been acquired. In the normal retirement path,
`NativeLibrary::retire` marks retirement before dropping the descriptor-owning
components. Rust ownership, foreign-code compliance, Tokio scheduling, dynamic
loading and operating-system unmapping remain outside the Lean language model.

## Routing references, rollback and indexes

`Routing.Peer` is one recipient's identifier namespace. Each issued identifier
creates one directory-owned export reference. The reference subsequently occupies
the entry map, a detached collection awaiting drop, or the dropped ledger. The
trace invariant proves one reference for every allocated identifier and zero
references outside the historical allocated range. It therefore covers any
combination of duplicate release, pending-batch rollback, source-owner reclaim,
generation sweep and recipient close without double return.

Release validates the whole supplied identifier list before detaching entries.
An invalid element leaves the state unchanged; repeated valid release is
idempotent. Batch commit clears the batch's rollback ownership, and a later
rollback of that committed batch preserves its exports. A pending batch removes
its own still-live entries. The batch annotation is ghost state derived from the
actual `Batch.ids` lists and `committed` flag.

`IndexedPeer` adds the owner and lifetime index views. Insert writes the new
entry's metadata into both indexes. Detach masks the selected key in the map and
both indexes. The index invariant states exact equivalence between each index
membership and the authoritative entry metadata. Induction proves index
consistency and reference conservation for arbitrary insertion/removal traces.

Retirement marking and sweeping are distinct events. This preserves the source's
ordering of `lifetime.finish()` before acquiring the directory mutex. Marking
blocks new exports for that generation; sweeping removes its existing entries.
The model permits the intermediate interval where retired entries are still in
the directory. `Retirement::during` and UP/1 cancellation determine the behavior
of calls overlapping that interval and are separate invocation contracts.

Detaching never advances the dropped ledger. A separate destruction event
represents `drop(discarded)` after the mutex guard is gone. The directory's tests
also execute destructors that assert the directory lock is available. The ledger
tracks directory-owned references; callers can hold additional `Arc` clones whose
lifetimes continue independently.

The model bounds new identifiers with the checked increment guard. After a
recipient closes, its historical frontier remains ghost state for accounting;
the concrete `Recipient` can be removed. The closed connection cannot export
again. Active-route capacity is an input read from `routes.len() < max_exports`;
these theorems do not establish the concrete HashMap length implementation or
the route-transfer serializer.

## Verification evidence

The three modules compile with Lean 4.19.0. Their final integrated audit and the
production integration commands are recorded by the repository verifier and CI.
The commands in the bridge inventory identify existing tests, including host
acceptance/rejection, asynchronous disposal, callback cancellation during native
poll, real dynamic-library retirement, routing rollback, index unlinking and
destructor execution outside the directory mutex. Each command's execution
status is recorded separately from the theorem inventory.
