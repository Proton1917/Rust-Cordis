import Std

/-!
UP/1 envelope and transport grammar. Value is the CBOR data model at the
ciborium/cbor2/cbor-x library boundary. Float leaves retain a semantic token;
no floating arithmetic is used here. Byte framing is independent of that
library contract. The envelope grammar accepts arbitrary map field order.
-/
namespace Cordis.WireSemantics

abbrev Byte := Fin 256

inductive Value where
  | null
  | boolean (value : Bool)
  | integer (value : Int)
  | floatBits (bits : UInt64)
  | bytes (value : List Byte)
  | text (value : String)
  | array (values : List Value)
  | map (values : List (Value × Value))
  | tag (number : Nat) (value : Value)

inductive Message where
  | request (id : Nat) (method : String) (params : Value)
  | response (id : Nat) (value : Value)
  | failure (id : Nat) (code : Nat) (message : String) (detail : Value)
  | notification (method : String) (params : Value)

def object (fields : List (String × Value)) : Value :=
  .map (fields.map fun (key, value) => (.text key, value))

def lookup (name : String) : List (Value × Value) → Option Value
  | [] => none
  | (.text key, value) :: rest => if key = name then some value else lookup name rest
  | _ :: rest => lookup name rest

def countKey (name : String) : List (Value × Value) → Nat
  | [] => 0
  | (.text key, _) :: rest => (if key = name then 1 else 0) + countKey name rest
  | _ :: rest => countKey name rest

def exactFields (fields : List (Value × Value)) (names : List String) : Bool :=
  fields.length == names.length &&
    (names.all fun key => countKey key fields == 1)

def natural : Value → Option Nat
  | .integer (.ofNat n) => some n
  | _ => none

def textValue : Value → Option String
  | .text s => some s
  | _ => none

def validId (id : Nat) : Bool := 0 < id && id < 18446744073709551616

def validCode (code : Nat) : Bool := 0 < code && code < 4294967296

def shaped : Message → Bool
  | .request id method _ => validId id && !method.isEmpty
  | .response id _ => validId id
  | .failure id code _ _ => validId id && validCode code
  | .notification method _ => !method.isEmpty

def envelopeEncode : Message → Value
  | .request id method params => object [("request", object [
      ("id", .integer id), ("method", .text method), ("params", params)])]
  | .response id value => object [("response", object [
      ("id", .integer id), ("result", object [("ok", value)])])]
  | .failure id code message detail => object [("response", object [
      ("id", .integer id), ("result", object [("error", object [
        ("code", .integer code), ("message", .text message), ("detail", detail)])])])]
  | .notification method params => object [("notification", object [
      ("method", .text method), ("params", params)])]

def parseRequest (fields : List (Value × Value)) : Option Message := do
  if !exactFields fields ["id", "method", "params"] then none else do
    let id ← (lookup "id" fields).bind natural
    let method ← (lookup "method" fields).bind textValue
    let params ← lookup "params" fields
    some (.request id method params)

def parseNotification (fields : List (Value × Value)) : Option Message := do
  if !exactFields fields ["method", "params"] then none else do
    let method ← (lookup "method" fields).bind textValue
    let params ← lookup "params" fields
    some (.notification method params)

def parseResponse (fields : List (Value × Value)) : Option Message := do
  if !exactFields fields ["id", "result"] then none else do
    let id ← (lookup "id" fields).bind natural
    match ← lookup "result" fields with
    | .map [(.text "ok", value)] => some (.response id value)
    | .map [(.text "error", .map error)] =>
        if !exactFields error ["code", "message", "detail"] then none else do
          let code ← (lookup "code" error).bind natural
          let message ← (lookup "message" error).bind textValue
          let detail ← lookup "detail" error
          some (.failure id code message detail)
    | _ => none

def parseEnvelope : Value → Option Message
  | .map [(.text "request", .map fields)] => parseRequest fields
  | .map [(.text "response", .map fields)] => parseResponse fields
  | .map [(.text "notification", .map fields)] => parseNotification fields
  | _ => none

def envelopeDecode (value : Value) : Option Message := do
  let message ← parseEnvelope value
  if shaped message then some message else none

theorem envelope_parse_encode (message : Message) :
    parseEnvelope (envelopeEncode message) = some message := by
  cases message <;> simp [parseEnvelope, envelopeEncode, object, parseRequest,
    parseResponse, parseNotification, exactFields, countKey, lookup, natural, textValue]

theorem envelope_roundtrip (message : Message) (h : shaped message = true) :
    envelopeDecode (envelopeEncode message) = some message := by
  simp [envelopeDecode, envelope_parse_encode, h]

theorem decoded_message_has_valid_shape (value : Value) (message : Message)
    (h : envelopeDecode value = some message) : shaped message = true := by
  unfold envelopeDecode at h
  cases hp : parseEnvelope value <;> simp [hp] at h
  rename_i result
  rcases h with ⟨hs, rfl⟩
  exact hs

theorem envelope_encoding_injective (left right : Message)
    (h : envelopeEncode left = envelopeEncode right) : left = right := by
  have : some left = some right := by
    rw [← envelope_parse_encode left, ← envelope_parse_encode right, h]
  exact Option.some.inj this

theorem zero_request_rejected (method : String) (params : Value) :
    envelopeDecode (envelopeEncode (.request 0 method params)) = none := by
  simp [envelopeDecode, envelope_parse_encode, shaped, validId]

theorem empty_method_rejected (id : Nat) (params : Value) :
    envelopeDecode (envelopeEncode (.request id "" params)) = none := by
  have empty : "".isEmpty = true := by decide
  simp [envelopeDecode, envelope_parse_encode, shaped, empty]

theorem zero_error_rejected (id : Nat) (message : String) (detail : Value) :
    envelopeDecode (envelopeEncode (.failure id 0 message detail)) = none := by
  simp [envelopeDecode, envelope_parse_encode, shaped, validCode]

/-! The transport prefix is a four-byte unsigned big-endian body length. -/
def octet (n : Nat) : Byte := ⟨n % 256, Nat.mod_lt _ (by decide)⟩

def header (n : Nat) : List Byte :=
  [octet (n / 16777216), octet (n / 65536), octet (n / 256), octet n]

def lengthWord (a b c d : Byte) : Nat :=
  a.val * 16777216 + b.val * 65536 + c.val * 256 + d.val

def readHeader : List Byte → Option (Nat × List Byte)
  | a :: b :: c :: d :: rest => some (lengthWord a b c d, rest)
  | _ => none

theorem header_word (n : Nat) (h : n < 4294967296) :
    lengthWord (octet (n / 16777216)) (octet (n / 65536))
      (octet (n / 256)) (octet n) = n := by
  simp [lengthWord, octet]
  omega

theorem header_length (n : Nat) : (header n).length = 4 := rfl

theorem read_header_roundtrip (n : Nat) (rest : List Byte) (h : n < 4294967296) :
    readHeader (header n ++ rest) = some (n, rest) := by
  simp [header, readHeader, header_word n h]

theorem parsed_length_is_word (a b c d : Byte) :
    lengthWord a b c d < 4294967296 := by
  have ha := a.isLt; have hb := b.isLt; have hc := c.isLt; have hd := d.isLt
  simp [lengthWord]
  omega

def frame (body : List Byte) : List Byte := header body.length ++ body

inductive FrameError where
  | headerIncomplete | bodyIncomplete | overLimit
  deriving DecidableEq

def readFrame (maximum : Nat) (stream : List Byte) :
    Except FrameError (List Byte × List Byte) :=
  match readHeader stream with
  | none => .error .headerIncomplete
  | some (length, rest) =>
      if length > maximum then .error .overLimit
      else if length > rest.length then .error .bodyIncomplete
      else .ok (rest.take length, rest.drop length)

theorem frame_length (body : List Byte) :
    (frame body).length = body.length + 4 := by
  simp only [frame, List.length_append, header_length]
  omega

theorem frame_roundtrip_with_tail (body tail : List Byte) (maximum : Nat)
    (word : body.length < 4294967296) (bounded : body.length ≤ maximum) :
    readFrame maximum (frame body ++ tail) = .ok (body, tail) := by
  simp only [frame, List.append_assoc]
  unfold readFrame
  rw [read_header_roundtrip _ _ word]
  simp [Nat.not_lt.mpr bounded]

theorem oversized_header_rejected_before_body (n maximum : Nat) (tail : List Byte)
    (word : n < 4294967296) (tooLarge : maximum < n) :
    readFrame maximum (header n ++ tail) = .error .overLimit := by
  unfold readFrame
  rw [read_header_roundtrip _ _ word]
  simp [tooLarge]

theorem incomplete_body_rejected (n maximum : Nat) (tail : List Byte)
    (word : n < 4294967296) (bounded : n ≤ maximum) (short : tail.length < n) :
    readFrame maximum (header n ++ tail) = .error .bodyIncomplete := by
  unfold readFrame
  rw [read_header_roundtrip _ _ word]
  simp [Nat.not_lt.mpr bounded, short]

def readFrames (maximum : Nat) : Nat → List Byte →
    Except FrameError (List (List Byte) × List Byte)
  | 0, stream => .ok ([], stream)
  | count + 1, stream => do
      let (body, rest) ← readFrame maximum stream
      let (bodies, suffix) ← readFrames maximum count rest
      pure (body :: bodies, suffix)

def frames (bodies : List (List Byte)) : List Byte := bodies.flatMap frame

theorem frames_roundtrip (bodies : List (List Byte)) (tail : List Byte) (maximum : Nat)
    (bounded : ∀ body ∈ bodies, body.length ≤ maximum ∧ body.length < 4294967296) :
    readFrames maximum bodies.length (frames bodies ++ tail) = .ok (bodies, tail) := by
  induction bodies with
  | nil => simp [frames, readFrames]
  | cons body bodies ih =>
    have hb := bounded body (by simp)
    have hr : ∀ b ∈ bodies, b.length ≤ maximum ∧ b.length < 4294967296 := by
      intro b h; exact bounded b (by simp [h])
    simp only [frames, List.flatMap_cons, List.length_cons, readFrames]
    rw [List.append_assoc, frame_roundtrip_with_tail body _ maximum hb.2 hb.1]
    change (do
      let (bs, suffix) ← readFrames maximum bodies.length (frames bodies ++ tail)
      pure (body :: bs, suffix)) = .ok (body :: bodies, tail)
    rw [ih hr]
    rfl

/-- Any partition of a byte stream reassembles to the same parse input. -/
def accumulate (buffer : List Byte) (chunks : List (List Byte)) : List Byte :=
  chunks.foldl (· ++ ·) buffer

theorem chunks_preserve_stream (buffer : List Byte) (chunks : List (List Byte)) :
    accumulate buffer chunks = buffer ++ chunks.flatten := by
  induction chunks generalizing buffer with
  | nil => simp [accumulate]
  | cons chunk chunks ih =>
    simp only [accumulate, List.foldl_cons]
    rw [show List.foldl (· ++ ·) (buffer ++ chunk) chunks =
      accumulate (buffer ++ chunk) chunks from rfl, ih]
    simp [List.append_assoc]

theorem chunked_frames_roundtrip (chunks : List (List Byte)) (bodies : List (List Byte))
    (maximum : Nat) (partition : chunks.flatten = frames bodies)
    (bounded : ∀ body ∈ bodies, body.length ≤ maximum ∧ body.length < 4294967296) :
    readFrames maximum bodies.length (accumulate [] chunks) = .ok (bodies, []) := by
  rw [chunks_preserve_stream, List.nil_append, partition]
  simpa using frames_roundtrip bodies [] maximum bounded

/-! The CBOR unsigned integer used by the negotiated invocation budget. -/
def integerWidth (n : Nat) : Nat :=
  if n ≤ 23 then 1 else if n ≤ 255 then 2 else if n ≤ 65535 then 3
  else if n ≤ 4294967295 then 5 else 9

def word16 (n : Nat) : List Byte := [octet (n / 256), octet n]

def word64 (n : Nat) : List Byte :=
  [octet (n / 72057594037927936), octet (n / 281474976710656),
   octet (n / 1099511627776), octet (n / 4294967296),
   octet (n / 16777216), octet (n / 65536), octet (n / 256), octet n]

def integerEncode (n : Nat) : List Byte :=
  if n ≤ 23 then [octet n]
  else if n ≤ 255 then octet 24 :: [octet n]
  else if n ≤ 65535 then octet 25 :: word16 n
  else if n ≤ 4294967295 then octet 26 :: header n
  else octet 27 :: word64 n

def readUnsigned : List Byte → Option (Nat × List Byte)
  | [] => none
  | tag :: rest =>
    if tag.val < 24 then some (tag.val, rest)
    else match tag.val, rest with
    | 24, a :: suffix => some (a.val, suffix)
    | 25, a :: b :: suffix => some (a.val * 256 + b.val, suffix)
    | 26, a :: b :: c :: d :: suffix => some (lengthWord a b c d, suffix)
    | 27, a :: b :: c :: d :: e :: f :: g :: h :: suffix =>
      some (a.val * 72057594037927936 + b.val * 281474976710656 +
        c.val * 1099511627776 + d.val * 4294967296 +
        e.val * 16777216 + f.val * 65536 + g.val * 256 + h.val, suffix)
    | _, _ => none

theorem word16_value (n : Nat) (bound : n < 65536) :
    (n / 256) % 256 * 256 + n % 256 = n := by omega

theorem word64_value (n : Nat) (bound : n < 18446744073709551616) :
    (n / 72057594037927936) % 256 * 72057594037927936 +
    (n / 281474976710656) % 256 * 281474976710656 +
    (n / 1099511627776) % 256 * 1099511627776 +
    (n / 4294967296) % 256 * 4294967296 +
    (n / 16777216) % 256 * 16777216 + (n / 65536) % 256 * 65536 +
    (n / 256) % 256 * 256 + n % 256 = n := by omega

theorem unsigned_integer_roundtrip (n : Nat) (tail : List Byte)
    (bound : n < 18446744073709551616) :
    readUnsigned (integerEncode n ++ tail) = some (n, tail) := by
  unfold integerEncode
  split
  · rename_i h
    have hn : n < 256 := by omega
    simp [readUnsigned, octet, Nat.mod_eq_of_lt hn, show n < 24 by omega]
  · split
    · rename_i h
      have hn : n < 256 := by omega
      simp [readUnsigned, octet, Nat.mod_eq_of_lt hn, show (24 : Byte).val = 24 from rfl, show (25 : Byte).val = 25 from rfl, show (27 : Byte).val = 27 from rfl]
    · split
      · rename_i h
        simp [readUnsigned, word16, octet, word16_value n (by omega), show (24 : Byte).val = 24 from rfl, show (25 : Byte).val = 25 from rfl, show (27 : Byte).val = 27 from rfl]
      · split
        · rename_i h
          simp only [List.cons_append, List.nil_append, readUnsigned]
          have hc : (octet 26).val = 26 := rfl
          simp only [hc]
          simp [header, lengthWord, octet]
          omega
        · simp [readUnsigned, word64, octet, word64_value n bound, show (24 : Byte).val = 24 from rfl, show (25 : Byte).val = 25 from rfl, show (27 : Byte).val = 27 from rfl]

theorem integer_encoded_width (n : Nat) : (integerEncode n).length = integerWidth n := by
  unfold integerEncode integerWidth
  split <;> (try split) <;> (try split) <;> (try split) <;>
    simp [word16, word64, header]

theorem decreasing_budget_never_grows_encoding (new old : Nat) (h : new ≤ old) :
    integerWidth new ≤ integerWidth old := by
  unfold integerWidth
  repeat' first | split | omega

def encodeByWidth (width n : Nat) : List Byte :=
  match width with
  | 1 => [octet n]
  | 2 => octet 24 :: [octet n]
  | 3 => octet 25 :: word16 n
  | 5 => octet 26 :: header n
  | _ => octet 27 :: word64 n

theorem encoding_selected_by_width (n : Nat) :
    encodeByWidth (integerWidth n) n = integerEncode n := by
  unfold integerWidth integerEncode
  repeat' first | split | rfl

/-- The frame writer truncates the old last budget before appending its new value. -/
def replaceBudget (old : List Byte) (offset value : Nat) : List Byte :=
  frame ((old.take offset).drop 4 ++ integerEncode value)

theorem replacing_budget_preserves_message_prefix (bodyPrefix : List Byte) (old fresh : Nat) :
    replaceBudget (frame (bodyPrefix ++ integerEncode old)) (4 + bodyPrefix.length) fresh =
      frame (bodyPrefix ++ integerEncode fresh) := by
  simp only [replaceBudget, frame]
  simp [header, Nat.add_comm 4, List.take_append]

theorem replacement_frame_roundtrip (bodyPrefix : List Byte) (old fresh maximum : Nat)
    (bound : (bodyPrefix ++ integerEncode fresh).length ≤ maximum)
    (word : (bodyPrefix ++ integerEncode fresh).length < 4294967296) :
    readFrame maximum
      (replaceBudget (frame (bodyPrefix ++ integerEncode old)) (4 + bodyPrefix.length) fresh) =
      .ok (bodyPrefix ++ integerEncode fresh, []) := by
  rw [replacing_budget_preserves_message_prefix]
  simpa using frame_roundtrip_with_tail (bodyPrefix ++ integerEncode fresh) [] maximum word bound

structure WidthArm where
  upper : Nat
  width : Nat

/-- Source-extracted match arms are interpreted in first-match order. -/
def evalWidth (fallback : Nat) : List WidthArm → Nat → Nat
  | [], _ => fallback
  | arm :: arms, n => if n ≤ arm.upper then arm.width else evalWidth fallback arms n

theorem source_width_program (n : Nat) :
    evalWidth 9 [⟨23, 1⟩, ⟨255, 2⟩, ⟨65535, 3⟩, ⟨4294967295, 5⟩] n = integerWidth n := rfl

/-! Protocol version and deadline capability are separate negotiated facts. -/
structure Negotiation where
  ready : Bool := false
  budget : Bool := false
  deriving DecidableEq

def hello (protocol : Nat) (advertisedBudget : Bool) : Option Negotiation :=
  if protocol = 1 then some ⟨true, advertisedBudget⟩ else none

theorem hello_accepts_exact_version (protocol : Nat) (budget : Bool) :
    (∃ result, hello protocol budget = some result) ↔ protocol = 1 := by
  simp [hello]

theorem capability_comes_from_accepted_hello (protocol : Nat) (advertised : Bool)
    (result : Negotiation) (h : hello protocol advertised = some result) :
    result.ready = true ∧ result.budget = advertised := by
  unfold hello at h
  split at h
  · cases h
    simp
  · contradiction

/-! `AtomicU64::fetch_update(checked_add(1))` returns the old frontier.
An exhausted frontier remains unchanged; terminal processing never rewinds it. -/
def allocateId (maximum frontier : Nat) : Option (Nat × Nat) :=
  if frontier < maximum then some (frontier, frontier + 1) else none

theorem allocated_id_advances_frontier (maximum frontier id next : Nat)
    (accepted : allocateId maximum frontier = some (id, next)) :
    id = frontier ∧ next = id + 1 ∧ id < maximum := by
  unfold allocateId at accepted
  split at accepted
  · cases accepted
    simp_all
  · contradiction

theorem exhausted_request_ids_reject (maximum : Nat) : allocateId maximum maximum = none := by
  simp [allocateId]

theorem consecutive_allocations_never_reuse (maximum frontier first middle second finalFrontier : Nat)
    (one : allocateId maximum frontier = some (first, middle))
    (two : allocateId maximum middle = some (second, finalFrontier)) : first < second := by
  have h₁ := allocated_id_advances_frontier _ _ _ _ one
  have h₂ := allocated_id_advances_frontier _ _ _ _ two
  omega

end Cordis.WireSemantics
