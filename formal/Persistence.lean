import Std

/-!
Deployment publication in `cordis-runtime/src/storage.rs`.

An image maps a pathname identity to exact contents (or absence). `prepare`
represents completion of the backup-directory sync, rename to undo, and parent
directory sync. Before that boundary, target names retain the previous image.
After it, individual names can persist independently. A successful data-directory
sync precedes moving the decision marker; a second successful directory sync is
the acknowledgement boundary. Recovery replays the saved names individually.

The filesystem contract is explicit: successful file sync preserves complete
contents, rename atomically changes one name, successful directory sync persists
its entries, and unchanged names remain unchanged. The exclusive deployment lock
excludes external writers. These are environmental assumptions, not Lean axioms.
-/

namespace Cordis.Persistence

abbrev Image := Nat → Option Nat
abbrev Change := Nat × Option Nat

def put (image : Image) (key : Nat) (value : Option Nat) : Image :=
  fun other => if other = key then value else image other

def applyWrites (image : Image) : List Change → Image
  | [] => image
  | (key, value) :: rest => applyWrites (put image key value) rest

def Frame (before : Image) (keys : List Nat) (image : Image) : Prop :=
  ∀ key, key ∉ keys → image key = before key

theorem frame_initial (before : Image) (keys : List Nat) : Frame before keys before := by
  intro key unused
  rfl

theorem put_frame (before image : Image) (keys : List Nat) (key : Nat)
    (value : Option Nat) (h : Frame before keys image) (hk : key ∈ keys) :
    Frame before keys (put image key value) := by
  intro other outside
  have ne : other ≠ key := by intro eq; exact outside (eq ▸ hk)
  simp [put, ne, h other outside]

theorem persist_name_frame (before visible durable : Image) (keys : List Nat)
    (key : Nat) (hv : Frame before keys visible) (hd : Frame before keys durable) :
    Frame before keys (put durable key (visible key)) := by
  intro other outside
  by_cases same : other = key
  · subst other
    simp [put, hv key outside]
  · simp [put, same, hd other outside]

/-- Undo records are replayed in any order; repeated keys are harmless. -/
def restore (before : Image) : List Nat → Image → Image
  | [], image => image
  | key :: rest, image => restore before rest (put image key (before key))

theorem restore_at (before image : Image) (keys : List Nat) (key : Nat) :
    restore before keys image key = if key ∈ keys then before key else image key := by
  induction keys generalizing image with
  | nil => simp [restore]
  | cons head tail ih =>
    rw [restore, ih]
    by_cases htail : key ∈ tail <;> by_cases hhead : key = head <;>
      simp [htail, hhead, put]

theorem recovery_replays_complete_image (before image : Image) (keys : List Nat)
    (h : Frame before keys image) : restore before keys image = before := by
  funext key
  rw [restore_at]
  split
  · rfl
  · exact h key ‹key ∉ keys›

theorem partial_recovery_preserves_frame (before image : Image)
    (allKeys restoredKeys : List Nat) (h : Frame before allKeys image) :
    Frame before allKeys (restore before restoredKeys image) := by
  intro key outside
  rw [restore_at]
  split
  · rfl
  · exact h key outside

theorem interrupted_recovery_can_restart (before image : Image)
    (allKeys restoredKeys : List Nat) (h : Frame before allKeys image) :
    restore before allKeys (restore before restoredKeys image) = before := by
  exact recovery_replays_complete_image before _ allKeys
    (partial_recovery_preserves_frame before image allKeys restoredKeys h)

inductive Phase where
  | preparing | writing | dataSynced | decisionMoved | acknowledged
  deriving DecidableEq

structure State where
  phase : Phase
  visible : Image
  durable : Image
  remaining : List Change
  staged : Bool := false
  fileSynced : Bool := false

def initial (before : Image) (writes : List Change) : State :=
  ⟨.preparing, before, before, writes, false, false⟩

def DurableFact (before after : Image) (s : State) : Prop :=
  match s.phase with
  | .preparing => s.durable = before
  | .writing => True
  | .dataSynced | .decisionMoved | .acknowledged => s.durable = after

def Invariant (before : Image) (writes : List Change) (s : State) : Prop :=
  applyWrites s.visible s.remaining = applyWrites before writes ∧
  (∀ change ∈ s.remaining, change.1 ∈ writes.map Prod.fst) ∧
  Frame before (writes.map Prod.fst) s.visible ∧
  Frame before (writes.map Prod.fst) s.durable ∧
  DurableFact before (applyWrites before writes) s

/-- Guards are control-flow facts from the implementation, rather than the
invariant being proved. Failed I/O leaves the last successful state available
to crash recovery; `fault` therefore makes no successful transition. -/
inductive Step : State → State → Prop where
  | prepare (s : State) (h : s.phase = .preparing) :
      Step s { s with phase := .writing }
  | stage (s : State) (key value : Nat) (rest : List Change)
      (h : s.phase = .writing) (todo : s.remaining = (key, some value) :: rest) :
      Step s { s with staged := true, fileSynced := false }
  | syncFile (s : State) (h : s.phase = .writing) (ready : s.staged = true) :
      Step s { s with fileSynced := true }
  | publish (s : State) (key value : Nat) (rest : List Change)
      (h : s.phase = .writing) (todo : s.remaining = (key, some value) :: rest)
      (synced : s.fileSynced = true) :
      Step s { s with visible := put s.visible key (some value), remaining := rest,
                      staged := false, fileSynced := false }
  | remove (s : State) (key : Nat) (rest : List Change)
      (h : s.phase = .writing) (todo : s.remaining = (key, none) :: rest) :
      Step s { s with visible := put s.visible key none, remaining := rest,
                      staged := false, fileSynced := false }
  | persistName (s : State) (key : Nat) (h : s.phase = .writing) :
      Step s { s with durable := put s.durable key (s.visible key) }
  | syncDataDirectory (s : State) (h : s.phase = .writing) (done : s.remaining = []) :
      Step s { s with phase := .dataSynced, durable := s.visible }
  | moveDecision (s : State) (h : s.phase = .dataSynced) :
      Step s { s with phase := .decisionMoved }
  | confirmDecision (s : State) (h : s.phase = .decisionMoved) :
      Step s { s with phase := .acknowledged }
  | fault (s : State) : Step s s

theorem initial_invariant (before : Image) (writes : List Change) :
    Invariant before writes (initial before writes) := by
  refine ⟨rfl, ?_, frame_initial _ _, frame_initial _ _, rfl⟩
  intro change member
  exact List.mem_map.mpr ⟨change, member, rfl⟩

theorem write_head_preserves (before : Image) (writes : List Change) (s : State)
    (key : Nat) (value : Option Nat) (rest : List Change)
    (h : Invariant before writes s) (phase : s.phase = .writing)
    (todo : s.remaining = (key, value) :: rest) :
    Invariant before writes
      { s with visible := put s.visible key value, remaining := rest,
               staged := false, fileSynced := false } := by
  rcases h with ⟨plan, covered, visible, durable, disk⟩
  have hk : key ∈ writes.map Prod.fst := covered (key, value) (by simp [todo])
  refine ⟨?_, ?_, put_frame before s.visible _ key value visible hk, durable, ?_⟩
  · simpa [todo, applyWrites] using plan
  · intro change member
    exact covered change (by simp [todo, member])
  · simp [DurableFact, phase]

theorem step_preserves (before : Image) (writes : List Change) (s t : State)
    (h : Invariant before writes s) (step : Step s t) : Invariant before writes t := by
  cases step with
  | prepare phase =>
    rcases h with ⟨plan, covered, visible, durable, disk⟩
    exact ⟨plan, covered, visible, durable, trivial⟩
  | stage key value rest phase todo => exact h
  | syncFile phase ready => exact h
  | publish key value rest phase todo synced =>
    exact write_head_preserves before writes s key (some value) rest h phase todo
  | remove key rest phase todo =>
    exact write_head_preserves before writes s key none rest h phase todo
  | persistName key phase =>
    rcases h with ⟨plan, covered, visible, durable, disk⟩
    exact ⟨plan, covered, visible,
      persist_name_frame before s.visible s.durable _ key visible durable,
      by simp [DurableFact, phase]⟩
  | syncDataDirectory phase done =>
    rcases h with ⟨plan, covered, visible, durable, disk⟩
    exact ⟨plan, covered, visible, visible, by simpa [DurableFact, done, applyWrites] using plan⟩
  | moveDecision phase =>
    rcases h with ⟨plan, covered, visible, durable, disk⟩
    exact ⟨plan, covered, visible, durable, by simpa [DurableFact, phase] using disk⟩
  | confirmDecision phase =>
    rcases h with ⟨plan, covered, visible, durable, disk⟩
    exact ⟨plan, covered, visible, durable, by simpa [DurableFact, phase] using disk⟩
  | fault => exact h

inductive Reachable (before : Image) (writes : List Change) : State → Prop where
  | initial : Reachable before writes (initial before writes)
  | next {s t : State} : Reachable before writes s → Step s t → Reachable before writes t

theorem reachable_invariant (before : Image) (writes : List Change) (s : State)
    (h : Reachable before writes s) : Invariant before writes s := by
  induction h with
  | initial => exact initial_invariant before writes
  | next previous step ih => exact step_preserves before writes _ _ ih step

theorem preparing_preserves_original_contents (before : Image) (writes : List Change)
    (s : State) (reachable : Reachable before writes s) (preparing : s.phase = .preparing) :
    s.visible = before := by
  induction reachable with
  | initial => rfl
  | next previous transition ih =>
    cases transition <;> simp_all

/-- A crash between moving the decision marker and syncing its directory may
retain or lose that move. Both choices are legal; the data was already synced. -/
def recover (before : Image) (keys : List Nat) (s : State)
    (markerMovePersisted : Bool) : Image :=
  match s.phase with
  | .preparing | .acknowledged => s.durable
  | .writing | .dataSynced => restore before keys s.durable
  | .decisionMoved =>
      if markerMovePersisted then s.durable else restore before keys s.durable

theorem crash_recovery_is_atomic (before : Image) (writes : List Change)
    (s : State) (h : Reachable before writes s) (markerMovePersisted : Bool) :
    recover before (writes.map Prod.fst) s markerMovePersisted = before ∨
      recover before (writes.map Prod.fst) s markerMovePersisted = applyWrites before writes := by
  obtain ⟨plan, covered, visible, durable, disk⟩ := reachable_invariant before writes s h
  have restored := recovery_replays_complete_image before s.durable _ durable
  cases hp : s.phase <;> cases markerMovePersisted <;>
    simp_all [recover, DurableFact]

theorem acknowledged_deployment_survives_crash (before : Image) (writes : List Change)
    (s : State) (h : Reachable before writes s) (ack : s.phase = .acknowledged)
    (markerMovePersisted : Bool) :
    recover before (writes.map Prod.fst) s markerMovePersisted = applyWrites before writes := by
  have disk := (reachable_invariant before writes s h).2.2.2.2
  simpa [recover, DurableFact, ack] using disk

theorem unfinished_decision_restores_previous (before : Image) (writes : List Change)
    (s : State) (h : Reachable before writes s)
    (phase : s.phase = .writing ∨ s.phase = .dataSynced) (markerMovePersisted : Bool) :
    recover before (writes.map Prod.fst) s markerMovePersisted = before := by
  have restored := recovery_replays_complete_image before s.durable _
    (reachable_invariant before writes s h).2.2.2.1
  rcases phase with phase | phase <;> simpa [recover, phase] using restored

/-! ## Checkpoint cache publication

The cache mutex hides provisional entries until batch directory confirmation.
On a failed directory sync the implementation clears the entire cache. The
model records the contents whose directory entry was confirmed, including a
readable-but-uncached file which must pass file sync and directory sync again.
-/

structure Cache where
  visible : Nat
  fileDurable : Nat
  directoryDurable : Nat
  candidate : Option Nat := none
  reusable : Option Nat := none
  deriving DecidableEq

def Cache.Valid (s : Cache) : Prop :=
  ∀ value, s.reusable = some value → value = s.directoryDurable ∧ value = s.fileDurable

def beginCheckpoint (s : Cache) (value : Nat) : Cache :=
  { s with visible := value, candidate := none, reusable := none }

def syncCheckpointFile (s : Cache) : Cache :=
  { s with fileDurable := s.visible, candidate := some s.visible, reusable := none }

def confirmCheckpointDirectory (s : Cache) : Cache :=
  { s with directoryDurable := s.fileDurable, reusable := s.candidate }

def failCheckpointDirectory (s : Cache) : Cache :=
  { s with candidate := none, reusable := none }

def CandidateValid (s : Cache) : Prop :=
  ∀ value, s.candidate = some value → value = s.fileDurable

theorem begin_checkpoint_invalidates_cache (s : Cache) (value : Nat) :
    (beginCheckpoint s value).Valid ∧ CandidateValid (beginCheckpoint s value) := by
  simp [Cache.Valid, CandidateValid, beginCheckpoint]

theorem synced_file_records_candidate (s : Cache) :
    (syncCheckpointFile s).Valid ∧ CandidateValid (syncCheckpointFile s) := by
  simp [Cache.Valid, CandidateValid, syncCheckpointFile]

theorem confirmed_checkpoint_is_durable (s : Cache) (h : CandidateValid s) :
    (confirmCheckpointDirectory s).Valid := by
  intro value eq
  have recorded := h value eq
  exact ⟨recorded, recorded⟩

theorem failed_directory_sync_clears_reusable_fact (s : Cache) :
    (failCheckpointDirectory s).reusable = none ∧ (failCheckpointDirectory s).Valid := by
  simp [failCheckpointDirectory, Cache.Valid]

inductive CacheEvent where
  | beginWrite (value : Nat) | syncFile | syncDirectory | failDirectory | invalidate
  | rebind (visible fileDurable directoryDurable : Nat)
  deriving DecidableEq

def cacheStep (s : Cache) : CacheEvent → Cache
  | .beginWrite value => beginCheckpoint s value
  | .syncFile => syncCheckpointFile s
  | .syncDirectory => confirmCheckpointDirectory s
  | .failDirectory | .invalidate => failCheckpointDirectory s
  | .rebind visible fileDurable directoryDurable =>
      { visible := visible, fileDurable := fileDurable, directoryDurable := directoryDurable }

def cacheRun (s : Cache) : List CacheEvent → Cache
  | [] => s
  | e :: rest => cacheRun (cacheStep s e) rest

theorem cache_step_preserves (s : Cache) (event : CacheEvent)
    (h : s.Valid ∧ CandidateValid s) :
    (cacheStep s event).Valid ∧ CandidateValid (cacheStep s event) := by
  cases event with
  | beginWrite value => exact begin_checkpoint_invalidates_cache s value
  | syncFile => exact synced_file_records_candidate s
  | syncDirectory => exact ⟨confirmed_checkpoint_is_durable s h.2, h.2⟩
  | failDirectory => simp [cacheStep, failCheckpointDirectory, Cache.Valid, CandidateValid]
  | invalidate => simp [cacheStep, failCheckpointDirectory, Cache.Valid, CandidateValid]
  | rebind visible fileDurable directoryDurable =>
      simp [cacheStep, Cache.Valid, CandidateValid]

theorem cache_trace_preserves_durability (events : List CacheEvent) (s : Cache)
    (h : s.Valid ∧ CandidateValid s) :
    (cacheRun s events).Valid ∧ CandidateValid (cacheRun s events) := by
  induction events generalizing s with
  | nil => exact h
  | cons e rest ih => exact ih _ (cache_step_preserves s e h)

end Cordis.Persistence
