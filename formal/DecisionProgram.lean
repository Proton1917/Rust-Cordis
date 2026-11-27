import Lifecycle

/-!
Evaluation of the emitted decision language with one-shot predicate ownership.
The source printer accepts only Certified trees. A successful evaluation returns
both its result and the ordered predicate calls. The interpreter rejects a
second read of an already consumed predicate, matching the FnOnce boundary.
The Rust parser/compiler and source printer are exercised independently by the
compiled bridge; this module specifies their decision-language contract.
-/

namespace Cordis.DecisionProgram

/-- A path may consume each argument at most once. Branches share the prefix,
    while mutually exclusive suffixes may use the same argument. -/
def Safe (used : List Nat) : Tree α n → Prop
  | .leaf _ => True
  | .branch field yes no =>
    field.val ∉ used ∧ Safe (field.val :: used) yes ∧ Safe (field.val :: used) no

instance safeDecidable (used : List Nat) (tree : Tree α n) : Decidable (Safe used tree) :=
  match tree with
  | .leaf _ => isTrue trivial
  | .branch field yes no =>
    letI := safeDecidable (field.val :: used) yes
    letI := safeDecidable (field.val :: used) no
    inferInstanceAs (Decidable (field.val ∉ used ∧
      Safe (field.val :: used) yes ∧ Safe (field.val :: used) no))

/-- Calls consume argument ownership before entering the selected block.
    A rejected repeated call has no result, so it cannot masquerade as a value. -/
def execute (input : Fin n → Bool) (used : List Nat) : Tree α n → Option (α × List Nat)
  | .leaf value => some (value, [])
  | .branch field yes no =>
    if field.val ∈ used then none
    else
      let next := if input field then execute input (field.val :: used) yes
        else execute input (field.val :: used) no
      next.map fun result =>
        (result.1, field.val :: result.2)

theorem execute_preserves_result_and_calls (tree : Tree α n) (input : Fin n → Bool)
    (used : List Nat) (safe : Safe used tree) :
    execute input used tree = some (tree.eval input, tree.reads input) := by
  induction tree generalizing used with
  | leaf value => rfl
  | branch field yes no ihYes ihNo =>
    obtain ⟨fresh, yesSafe, noSafe⟩ := safe
    cases h : input field <;>
      simp [execute, Tree.eval, Tree.reads, fresh, h, ihYes _ yesSafe, ihNo _ noSafe]

theorem calls_are_fresh (tree : Tree α n) (input : Fin n → Bool)
    (used : List Nat) (safe : Safe used tree) :
    ∀ field ∈ tree.reads input, field ∉ used := by
  induction tree generalizing used with
  | leaf value => simp [Tree.reads]
  | branch field yes no ihYes ihNo =>
    obtain ⟨fresh, yesSafe, noSafe⟩ := safe
    have yesFresh := ihYes (field.val :: used) yesSafe
    have noFresh := ihNo (field.val :: used) noSafe
    cases h : input field <;>
      simp only [Tree.reads, h, Bool.false_eq_true, ↓reduceIte, List.mem_cons] <;>
      intro x member <;> rcases member with equal | member
    · simpa [equal] using fresh
    · exact fun old => noFresh x member (List.mem_cons_of_mem _ old)
    · simpa [equal] using fresh
    · exact fun old => yesFresh x member (List.mem_cons_of_mem _ old)

theorem calls_have_no_duplicates (tree : Tree α n) (input : Fin n → Bool)
    (used : List Nat) (safe : Safe used tree) : (tree.reads input).Nodup := by
  induction tree generalizing used with
  | leaf value => simp [Tree.reads]
  | branch field yes no ihYes ihNo =>
    obtain ⟨fresh, yesSafe, noSafe⟩ := safe
    have yesFresh := calls_are_fresh yes input (field.val :: used) yesSafe
    have noFresh := calls_are_fresh no input (field.val :: used) noSafe
    cases h : input field <;>
      simp only [Tree.reads, h, Bool.false_eq_true, ↓reduceIte, List.nodup_cons]
    · exact ⟨fun member => noFresh _ member (by simp), ihNo _ noSafe⟩
    · exact ⟨fun member => yesFresh _ member (by simp), ihYes _ yesSafe⟩

structure Certified (α : Type) (n : Nat) where
  tree : Tree α n
  safe : Safe [] tree

theorem certified_executes (program : Certified α n) (input : Fin n → Bool) :
    execute input [] program.tree = some (program.tree.eval input, program.tree.reads input) :=
  execute_preserves_result_and_calls _ _ _ program.safe

theorem certified_calls_once (program : Certified α n) (input : Fin n → Bool) :
    (program.tree.reads input).Nodup := calls_have_no_duplicates _ _ _ program.safe

def idleProgram : Certified Phase 3 := ⟨idleTree, by decide⟩
def targetProgram : Certified Target 2 := ⟨targetTree, by decide⟩
def refreshProgram : Certified Action 7 := ⟨refreshTree, by decide⟩
def finishProgram : Certified Action 5 := ⟨finishTree, by decide⟩
def settledProgram : Certified Bool 4 := ⟨settledTree, by decide⟩
def currentProgram : Certified Bool 2 := ⟨currentTree, by decide⟩
def currentEpochProgram : Certified Bool 3 := ⟨currentEpochTree, by decide⟩
def cleanupProgram : Certified Bool 2 := ⟨cleanupTree, by decide⟩

theorem refresh_safe : Safe [] refreshTree := refreshProgram.safe
theorem finish_safe : Safe [] finishTree := finishProgram.safe
theorem current_epoch_safe : Safe [] currentEpochTree := currentEpochProgram.safe

/-- A malformed generator input with a repeated predicate is rejected by the
    same semantics, including when the earlier call returned true. -/
theorem repeated_call_rejected (field : Fin n) (value : α) (input : Fin n → Bool)
    (taken : input field = true) :
    execute input [] (.branch field (.branch field (.leaf value) (.leaf value)) (.leaf value)) = none := by
  simp [execute, taken]

end Cordis.DecisionProgram
