import WireSemantics

/-!
CBOR byte grammar and UP value policy. `Parses` is an inductive byte grammar,
separate from the ciborium implementation. Text retains UTF-8 bytes; `Fits`
requires that they decode as UTF-8. Float values retain IEEE binary64 bits,
including signed zero and NaN payloads. Definite-length encodings are generated;
indefinite input is represented by the additional grammar productions.
-/
namespace Cordis.CborSemantics
open WireSemantics

mutual
inductive Data where
  | null | boolean (value : Bool)
  | unsigned (value : Nat) | negative (complement : Nat)
  | floating (bits : Fin 18446744073709551616)
  | bytes (value : List Byte) | text (utf8 : List Byte)
  | array (values : Values) | map (fields : Fields)
  | tag (number : Nat) (value : Data)
inductive Values where
  | nil | cons (value : Data) (rest : Values)
inductive Fields where
  | nil | cons (key value : Data) (rest : Fields)
end

def valuesLength : Values → Nat
  | .nil => 0 | .cons _ rest => valuesLength rest + 1
def fieldsLength : Fields → Nat
  | .nil => 0 | .cons _ _ rest => fieldsLength rest + 1

def utf8Valid (bytes : List Byte) : Bool :=
  (String.fromUTF8? ⟨(bytes.map (fun byte => UInt8.ofNat byte.val)).toArray⟩).isSome

/-- The leading byte carries the major type; the following bytes are unchanged. -/
def argument (major n : Nat) : List Byte :=
  match integerEncode n with
  | [] => []
  | first :: rest => octet (major * 32 + first.val) :: rest

mutual
def encode : Data → List Byte
  | .null => [octet 246]
  | .boolean false => [octet 244]
  | .boolean true => [octet 245]
  | .unsigned n => argument 0 n
  | .negative n => argument 1 n
  | .floating bits => octet 251 :: word64 bits.val
  | .bytes bytes => argument 2 bytes.length ++ bytes
  | .text bytes => argument 3 bytes.length ++ bytes
  | .array values => argument 4 (valuesLength values) ++ encodeValues values
  | .map fields => argument 5 (fieldsLength fields) ++ encodeFields fields
  | .tag number value => argument 6 number ++ encode value
def encodeValues : Values → List Byte
  | .nil => []
  | .cons value rest => encode value ++ encodeValues rest
def encodeFields : Fields → List Byte
  | .nil => []
  | .cons key value rest => encode key ++ encode value ++ encodeFields rest
end

mutual
def Fits : Data → Prop
  | .null | .boolean _ | .floating _ => True
  | .unsigned n | .negative n => n < 18446744073709551616
  | .bytes bytes => bytes.length < 18446744073709551616
  | .text bytes => bytes.length < 18446744073709551616 ∧ utf8Valid bytes = true
  | .array values => valuesLength values < 18446744073709551616 ∧ ValuesFit values
  | .map fields => fieldsLength fields < 18446744073709551616 ∧ FieldsFit fields
  | .tag number value => number < 18446744073709551616 ∧ Fits value
def ValuesFit : Values → Prop
  | .nil => True
  | .cons value rest => Fits value ∧ ValuesFit rest
def FieldsFit : Fields → Prop
  | .nil => True
  | .cons key value rest => Fits key ∧ Fits value ∧ FieldsFit rest
end

structure FloatCodec where
  half : Nat → Nat
  single : Nat → Nat
  expandHalf : Nat → Nat
  expandSingle : Nat → Nat

inductive FloatEncoding where
  | half (bits : Nat) | single (bits : Nat) | double (bits : Nat)

def shorten (codec : FloatCodec) (bits : Nat) : FloatEncoding :=
  if codec.expandHalf (codec.half bits) = bits then .half (codec.half bits)
  else if codec.expandSingle (codec.single bits) = bits then .single (codec.single bits)
  else .double bits

def expand (codec : FloatCodec) : FloatEncoding → Nat
  | .half bits => codec.expandHalf bits
  | .single bits => codec.expandSingle bits
  | .double bits => bits

/-- ciborium::Value first chooses serialize_f32 only on an exact bit roundtrip. -/
def prepareFloat (codec : FloatCodec) (bits : Nat) : Nat :=
  if codec.expandSingle (codec.single bits) = bits then codec.expandSingle (codec.single bits) else bits

theorem prepare_float_preserves_bits (codec : FloatCodec) (bits : Nat) :
    prepareFloat codec bits = bits := by
  unfold prepareFloat
  split
  · assumption
  · rfl


def encodeFloat (codec : FloatCodec) (bits : Nat) : List Byte :=
  match shorten codec bits with
  | .half bits => octet 249 :: word16 bits
  | .single bits => octet 250 :: header bits
  | .double bits => octet 251 :: word64 bits

/-! A decoder accepts overlong argument widths as well as shortest widths.
Reserved additional-information values 28, 29 and 30 have no production. -/
inductive ArgumentEncoding (major n : Nat) : List Byte → Prop
  | canonical (bound : n < 18446744073709551616) : ArgumentEncoding major n (argument major n)
  | one (bound : n < 256) : ArgumentEncoding major n [octet (major * 32 + 24), octet n]
  | two (bound : n < 65536) : ArgumentEncoding major n (octet (major * 32 + 25) :: word16 n)
  | four (bound : n < 4294967296) : ArgumentEncoding major n (octet (major * 32 + 26) :: header n)
  | eight (bound : n < 18446744073709551616) : ArgumentEncoding major n (octet (major * 32 + 27) :: word64 n)

/-! Each production consumes actual CBOR bytes and retains the unused suffix.
The integer head is the same base-256 encoding proved in WireSemantics. -/
mutual
inductive Parses (codec : FloatCodec) : List Byte → Data → List Byte → Prop
  | null (tail) : Parses codec (octet 246 :: tail) .null tail
  | undefined (tail) : Parses codec (octet 247 :: tail) .null tail
  | boolean (value : Bool) (tail) :
      Parses codec (octet (if value then 245 else 244) :: tail) (.boolean value) tail
  | unsigned (n tail) (bound : n < 18446744073709551616) :
      Parses codec (argument 0 n ++ tail) (.unsigned n) tail
  | negative (n tail) (bound : n < 18446744073709551616) :
      Parses codec (argument 1 n ++ tail) (.negative n) tail
  | floating (bits : Fin 18446744073709551616) (tail) :
      Parses codec (octet 251 :: (word64 bits.val ++ tail)) (.floating bits) tail
  | half (bits : Nat) (result : Fin 18446744073709551616) (tail)
      (word : bits < 65536) (same : codec.expandHalf bits = result.val) :
      Parses codec (octet 249 :: (word16 bits ++ tail)) (.floating result) tail
  | single (bits : Nat) (result : Fin 18446744073709551616) (tail)
      (word : bits < 4294967296) (same : codec.expandSingle bits = result.val) :
      Parses codec (octet 250 :: (header bits ++ tail)) (.floating result) tail
  | bytes (bytes tail) (bound : bytes.length < 18446744073709551616) :
      Parses codec (argument 2 bytes.length ++ bytes ++ tail) (.bytes bytes) tail
  | text (bytes tail) (bound : bytes.length < 18446744073709551616)
      (utf8 : utf8Valid bytes = true) :
      Parses codec (argument 3 bytes.length ++ bytes ++ tail) (.text bytes) tail
  | array (values body tail) (bound : valuesLength values < 18446744073709551616)
      (items : ParsesValues codec body values tail) :
      Parses codec (argument 4 (valuesLength values) ++ body) (.array values) tail
  | map (fields body tail) (bound : fieldsLength fields < 18446744073709551616)
      (items : ParsesFields codec body fields tail) :
      Parses codec (argument 5 (fieldsLength fields) ++ body) (.map fields) tail
  | tag (number value body tail) (bound : number < 18446744073709551616)
      (item : Parses codec body value tail) :
      Parses codec (argument 6 number ++ body) (.tag number value) tail
  | unsignedWidth (n head tail) (width : ArgumentEncoding 0 n head) :
      Parses codec (head ++ tail) (.unsigned n) tail
  | negativeWidth (n head tail) (width : ArgumentEncoding 1 n head) :
      Parses codec (head ++ tail) (.negative n) tail
  | bytesWidth (bytes head tail) (width : ArgumentEncoding 2 bytes.length head) :
      Parses codec (head ++ bytes ++ tail) (.bytes bytes) tail
  | textWidth (bytes head tail) (width : ArgumentEncoding 3 bytes.length head)
      (utf8 : utf8Valid bytes = true) :
      Parses codec (head ++ bytes ++ tail) (.text bytes) tail
  | arrayWidth (values head body tail) (width : ArgumentEncoding 4 (valuesLength values) head)
      (items : ParsesValues codec body values tail) :
      Parses codec (head ++ body) (.array values) tail
  | mapWidth (fields head body tail) (width : ArgumentEncoding 5 (fieldsLength fields) head)
      (items : ParsesFields codec body fields tail) :
      Parses codec (head ++ body) (.map fields) tail
  | tagWidth (number value head body tail) (width : ArgumentEncoding 6 number head)
      (item : Parses codec body value tail) :
      Parses codec (head ++ body) (.tag number value) tail
  | indefiniteArray (values body tail)
      (items : ParsesValues codec body values (octet 255 :: tail)) :
      Parses codec (octet 159 :: body) (.array values) tail
  | indefiniteMap (fields body tail)
      (items : ParsesFields codec body fields (octet 255 :: tail)) :
      Parses codec (octet 191 :: body) (.map fields) tail
  | indefiniteBytes (chunks : List (List Byte)) (tail)
      (sizes : ∀ chunk ∈ chunks, chunk.length < 18446744073709551616) :
      Parses codec (octet 95 :: (chunks.flatMap (fun bytes => argument 2 bytes.length ++ bytes) ++
        octet 255 :: tail)) (.bytes chunks.flatten) tail
  | indefiniteText (chunks : List (List Byte)) (tail)
      (sizes : ∀ chunk ∈ chunks, chunk.length < 18446744073709551616 ∧ utf8Valid chunk = true) :
      Parses codec (octet 127 :: (chunks.flatMap (fun bytes => argument 3 bytes.length ++ bytes) ++
        octet 255 :: tail)) (.text chunks.flatten) tail
inductive ParsesValues (codec : FloatCodec) : List Byte → Values → List Byte → Prop
  | nil (tail) : ParsesValues codec tail .nil tail
  | cons (value rest input middle tail)
      (item : Parses codec input value middle) (items : ParsesValues codec middle rest tail) :
      ParsesValues codec input (.cons value rest) tail
inductive ParsesFields (codec : FloatCodec) : List Byte → Fields → List Byte → Prop
  | nil (tail) : ParsesFields codec tail .nil tail
  | cons (key value rest input middle after tail)
      (keyParse : Parses codec input key middle) (valueParse : Parses codec middle value after)
      (items : ParsesFields codec after rest tail) : ParsesFields codec input (.cons key value rest) tail
end

variable {codec : FloatCodec}

mutual
theorem arbitrary_recursive_roundtrip (value : Data) (tail : List Byte) (fit : Fits value) :
    Parses codec (encode value ++ tail) value tail := by
  cases value with
  | null => exact .null tail
  | boolean value =>
    cases value with
    | false => exact .boolean false tail
    | true => exact .boolean true tail
  | unsigned n => exact .unsigned n tail fit
  | negative n => exact .negative n tail fit
  | floating bits => exact .floating bits tail
  | bytes bytes => exact .bytes bytes tail fit
  | text bytes => exact .text bytes tail fit.1 fit.2
  | array values =>
    simpa [encode, List.append_assoc] using
      Parses.array values (encodeValues values ++ tail) tail fit.1 (values_roundtrip values tail fit.2)
  | map fields =>
    simpa [encode, List.append_assoc] using
      Parses.map fields (encodeFields fields ++ tail) tail fit.1 (fields_roundtrip fields tail fit.2)
  | tag number value =>
    simpa [encode, List.append_assoc] using
      Parses.tag number value (encode value ++ tail) tail fit.1 (arbitrary_recursive_roundtrip value tail fit.2)
theorem values_roundtrip (values : Values) (tail : List Byte) (fit : ValuesFit values) :
    ParsesValues codec (encodeValues values ++ tail) values tail := by
  cases values with
  | nil => exact .nil tail
  | cons value rest =>
    simpa [encodeValues, List.append_assoc] using
      ParsesValues.cons value rest (encode value ++ (encodeValues rest ++ tail))
        (encodeValues rest ++ tail) tail
        (arbitrary_recursive_roundtrip value _ fit.1) (values_roundtrip rest tail fit.2)
theorem fields_roundtrip (fields : Fields) (tail : List Byte) (fit : FieldsFit fields) :
    ParsesFields codec (encodeFields fields ++ tail) fields tail := by
  cases fields with
  | nil => exact .nil tail
  | cons key value rest =>
    simpa [encodeFields, List.append_assoc] using
      ParsesFields.cons key value rest (encode key ++ (encode value ++ (encodeFields rest ++ tail)))
        (encode value ++ (encodeFields rest ++ tail)) (encodeFields rest ++ tail) tail
        (arbitrary_recursive_roundtrip key _ fit.1) (arbitrary_recursive_roundtrip value _ fit.2.1)
        (fields_roundtrip rest tail fit.2.2)
end

/-! Float shortening is guarded by decoded-bit equality in ciborium-ll.
The theorem is independent of every particular NaN payload and of the IEEE
conversion algorithm: only a lossless conversion may choose a narrow branch. -/
theorem every_float_bit_pattern_survives_shortening (codec : FloatCodec) (bits : Nat) :
    expand codec (shorten codec bits) = bits := by
  unfold shorten
  split
  · rename_i equal
    exact equal
  · split
    · rename_i equal
      exact equal
    · rfl

theorem signed_zero_stays_distinct (codec : FloatCodec) :
    expand codec (shorten codec 0) ≠ expand codec (shorten codec 9223372036854775808) := by
  simp [every_float_bit_pattern_survives_shortening]

theorem both_float_serialization_layers_preserve_bits (codec : FloatCodec) (bits : Nat) :
    expand codec (shorten codec (prepareFloat codec bits)) = bits := by
  rw [prepare_float_preserves_bits, every_float_bit_pattern_survives_shortening]

theorem actual_float_encoding_parses (codec : FloatCodec)
    (bits : Fin 18446744073709551616) (tail : List Byte)
    (halfWord : codec.half bits.val < 65536)
    (singleWord : codec.single bits.val < 4294967296) :
    Parses codec (encodeFloat codec bits.val ++ tail) (.floating bits) tail := by
  by_cases halfSame : codec.expandHalf (codec.half bits.val) = bits.val
  · simpa [encodeFloat, shorten, halfSame] using
      Parses.half (codec.half bits.val) bits tail halfWord halfSame
  · by_cases singleSame : codec.expandSingle (codec.single bits.val) = bits.val
    · simpa [encodeFloat, shorten, halfSame, singleSame] using
        Parses.single (codec.single bits.val) bits tail singleWord singleSame
    · simpa [encodeFloat, shorten, halfSame, singleSame] using
        (Parses.floating (codec := codec) bits tail)

mutual
def encodeWithFloats (codec : FloatCodec) : Data → List Byte
  | .array values => argument 4 (valuesLength values) ++ encodeValuesWithFloats codec values
  | .map fields => argument 5 (fieldsLength fields) ++ encodeFieldsWithFloats codec fields
  | .tag number value => argument 6 number ++ encodeWithFloats codec value
  | .floating bits => encodeFloat codec bits.val
  | value => encode value
def encodeValuesWithFloats (codec : FloatCodec) : Values → List Byte
  | .nil => []
  | .cons value rest => encodeWithFloats codec value ++ encodeValuesWithFloats codec rest
def encodeFieldsWithFloats (codec : FloatCodec) : Fields → List Byte
  | .nil => []
  | .cons key value rest =>
      encodeWithFloats codec key ++ encodeWithFloats codec value ++ encodeFieldsWithFloats codec rest
end

structure FloatWidths (codec : FloatCodec) : Prop where
  half : ∀ bits : Fin 18446744073709551616, codec.half bits.val < 65536
  single : ∀ bits : Fin 18446744073709551616, codec.single bits.val < 4294967296

mutual
theorem recursive_short_float_roundtrip (codec : FloatCodec) (widths : FloatWidths codec)
    (value : Data) (tail : List Byte) (fit : Fits value) :
    Parses codec (encodeWithFloats codec value ++ tail) value tail := by
  cases value with
  | null => exact .null tail
  | boolean value =>
    cases value with
    | false => exact .boolean false tail
    | true => exact .boolean true tail
  | unsigned n => exact .unsigned n tail fit
  | negative n => exact .negative n tail fit
  | floating bits => exact actual_float_encoding_parses codec bits tail (widths.half bits) (widths.single bits)
  | bytes bytes => exact .bytes bytes tail fit
  | text bytes => exact .text bytes tail fit.1 fit.2
  | array values =>
    simpa [encodeWithFloats, List.append_assoc] using
      Parses.array values (encodeValuesWithFloats codec values ++ tail) tail fit.1
        (values_short_float_roundtrip codec widths values tail fit.2)
  | map fields =>
    simpa [encodeWithFloats, List.append_assoc] using
      Parses.map fields (encodeFieldsWithFloats codec fields ++ tail) tail fit.1
        (fields_short_float_roundtrip codec widths fields tail fit.2)
  | tag number value =>
    simpa [encodeWithFloats, List.append_assoc] using
      Parses.tag number value (encodeWithFloats codec value ++ tail) tail fit.1
        (recursive_short_float_roundtrip codec widths value tail fit.2)
theorem values_short_float_roundtrip (codec : FloatCodec) (widths : FloatWidths codec)
    (values : Values) (tail : List Byte) (fit : ValuesFit values) :
    ParsesValues codec (encodeValuesWithFloats codec values ++ tail) values tail := by
  cases values with
  | nil => exact .nil tail
  | cons value rest =>
    simpa [encodeValuesWithFloats, List.append_assoc] using
      ParsesValues.cons value rest (encodeWithFloats codec value ++ (encodeValuesWithFloats codec rest ++ tail))
        (encodeValuesWithFloats codec rest ++ tail) tail
        (recursive_short_float_roundtrip codec widths value _ fit.1)
        (values_short_float_roundtrip codec widths rest tail fit.2)
theorem fields_short_float_roundtrip (codec : FloatCodec) (widths : FloatWidths codec)
    (fields : Fields) (tail : List Byte) (fit : FieldsFit fields) :
    ParsesFields codec (encodeFieldsWithFloats codec fields ++ tail) fields tail := by
  cases fields with
  | nil => exact .nil tail
  | cons key value rest =>
    simpa [encodeFieldsWithFloats, List.append_assoc] using
      ParsesFields.cons key value rest
        (encodeWithFloats codec key ++ (encodeWithFloats codec value ++ (encodeFieldsWithFloats codec rest ++ tail)))
        (encodeWithFloats codec value ++ (encodeFieldsWithFloats codec rest ++ tail))
        (encodeFieldsWithFloats codec rest ++ tail) tail
        (recursive_short_float_roundtrip codec widths key _ fit.1)
        (recursive_short_float_roundtrip codec widths value _ fit.2.1)
        (fields_short_float_roundtrip codec widths rest tail fit.2.2)
end

/-! Repository policy over the decoded tree, including keys of maps. -/
def resourceTag (number : Nat) (value : Data) : Bool :=
  if number = 60000 then
    match value with | .unsigned n => 0 < n && n < 18446744073709551616 | _ => false
  else true

mutual
def resourceValid : Data → Bool
  | .tag number value => resourceTag number value && resourceValid value
  | .array values => valuesResourceValid values
  | .map fields => fieldsResourceValid fields
  | _ => true
def valuesResourceValid : Values → Bool
  | .nil => true | .cons value rest => resourceValid value && valuesResourceValid rest
def fieldsResourceValid : Fields → Bool
  | .nil => true
  | .cons key value rest => resourceValid key && resourceValid value && fieldsResourceValid rest
end

mutual
def persistent : Data → Bool
  | .tag number value => number != 60000 && persistent value
  | .array values => valuesPersistent values
  | .map fields => fieldsPersistent fields
  | _ => true
def valuesPersistent : Values → Bool
  | .nil => true | .cons value rest => persistent value && valuesPersistent rest
def fieldsPersistent : Fields → Bool
  | .nil => true | .cons key value rest => persistent key && persistent value && fieldsPersistent rest
end

theorem resource_requires_positive_u64 (number : Nat) :
    resourceTag 60000 (.unsigned number) = true ↔ 0 < number ∧ number < 18446744073709551616 := by
  simp [resourceTag]

theorem persistent_rejects_resource_at_map_key (id : Nat) (value : Data) (rest : Fields) :
    persistent (.map (.cons (.tag 60000 (.unsigned id)) value rest)) = false := by
  simp [persistent, fieldsPersistent]

theorem persistent_rejects_resource_under_array (id : Nat) (rest : Values) :
    persistent (.array (.cons (.tag 60000 (.unsigned id)) rest)) = false := by
  simp [persistent, valuesPersistent]

mutual
def nodes : Data → Nat
  | .array values => 1 + valuesNodes values
  | .map fields => 1 + fieldsNodes fields
  | .tag _ value => 1 + nodes value
  | _ => 1
def valuesNodes : Values → Nat
  | .nil => 0 | .cons value rest => nodes value + valuesNodes rest
def fieldsNodes : Fields → Nat
  | .nil => 0 | .cons key value rest => nodes key + nodes value + fieldsNodes rest
end

structure Limits where
  depth : Nat
  collection : Nat
  values : Nat

mutual
def containersWithin (limits : Limits) (depth : Nat) : Data → Bool
  | .array values => depth < limits.depth && valuesLength values ≤ limits.collection &&
      valuesWithin limits (depth + 1) values
  | .map fields => depth < limits.depth && fieldsLength fields ≤ limits.collection &&
      fieldsWithin limits (depth + 1) fields
  | .tag _ value => depth < limits.depth && containersWithin limits (depth + 1) value
  | _ => true
def valuesWithin (limits : Limits) (depth : Nat) : Values → Bool
  | .nil => true
  | .cons value rest => containersWithin limits depth value && valuesWithin limits depth rest
def fieldsWithin (limits : Limits) (depth : Nat) : Fields → Bool
  | .nil => true
  | .cons key value rest => containersWithin limits depth key && containersWithin limits depth value &&
      fieldsWithin limits depth rest
end

def validate (limits : Limits) (depth already : Nat) (value : Data) : Bool :=
  already + nodes value ≤ limits.values && containersWithin limits depth value && resourceValid value

theorem accepted_graph_nodes_are_bounded (limits : Limits) (depth already : Nat) (value : Data)
    (accepted : validate limits depth already value = true) :
    already + nodes value ≤ limits.values := by
  simp [validate] at accepted
  exact accepted.1.1

theorem root_collection_overflow_rejected (limits : Limits) (depth already : Nat) (values : Values)
    (overflow : limits.collection < valuesLength values) :
    validate limits depth already (.array values) = false := by
  simp [validate, containersWithin, show ¬valuesLength values ≤ limits.collection by omega]

theorem container_at_depth_limit_rejected (limits : Limits) (depth already : Nat) (value : Data)
    (exhausted : limits.depth ≤ depth) :
    validate limits depth already (.tag 42 value) = false := by
  simp [validate, containersWithin, show ¬depth < limits.depth by omega]

/-! ciborium's special bignum decode is separate from generic CBOR syntax.
The dependency interprets definite byte strings of at most 16 bytes for tags
2 and 3; a negative magnitude at least 2^127 is rejected by its i128 visitor.
Larger byte strings and other payload constructors remain ordinary tags. -/
def magnitude (bytes : List Byte) : Nat := bytes.foldl (fun n byte => n * 256 + byte.val) 0

inductive TagMeaning where
  | ordinary (number : Nat) (value : Data)
  | positiveBig (number : Nat)
  | negativeBig (complement : Nat)
  | rejectedNegativeOverflow

def interpretTag (number : Nat) (value : Data) : TagMeaning :=
  match value with
  | .bytes bytes =>
    if bytes.length ≤ 16 then
      if number = 2 then .positiveBig (magnitude bytes)
      else if number = 3 then
        if magnitude bytes < 170141183460469231731687303715884105728 then
          .negativeBig (magnitude bytes) else .rejectedNegativeOverflow
      else .ordinary number value
    else .ordinary number value
  | _ => .ordinary number value

theorem tag_two_is_a_number (bytes : List Byte) (bound : bytes.length ≤ 16) :
    interpretTag 2 (.bytes bytes) = .positiveBig (magnitude bytes) := by simp [interpretTag, bound]

theorem excessive_negative_bignum_is_rejected (bytes : List Byte) (bound : bytes.length ≤ 16)
    (overflow : 170141183460469231731687303715884105728 ≤ magnitude bytes) :
    interpretTag 3 (.bytes bytes) = .rejectedNegativeOverflow := by
  simp [interpretTag, bound, show ¬magnitude bytes < 170141183460469231731687303715884105728 by omega]

theorem long_tag_bytes_remain_tagged (number : Nat) (bytes : List Byte) (large : 16 < bytes.length) :
    interpretTag number (.bytes bytes) = .ordinary number (.bytes bytes) := by
  simp [interpretTag, show ¬bytes.length ≤ 16 by omega]

/-- The two 64-bit limbs of a CBOR bignum; leading zero bytes are legal. -/
def word128 (n : Nat) : List Byte := word64 (n / 18446744073709551616) ++ word64 n

theorem word64_fold (initial n : Nat) :
    (word64 n).foldl (fun n byte => n * 256 + byte.val) initial =
      initial * 18446744073709551616 + n % 18446744073709551616 := by
  simp [word64, octet]
  omega

theorem word128_magnitude (n : Nat) (bound : n < 340282366920938463463374607431768211456) :
    magnitude (word128 n) = n := by
  simp only [magnitude, word128, List.foldl_append, word64_fold, Nat.zero_mul, Nat.zero_add]
  omega

/-- Integer semantics includes both the scalar major types and bignum tags.
The negative constructor's natural argument is the CBOR complement, so its
mathematical integer is `-1 - complement`. -/
inductive IntegerParses (codec : FloatCodec) : List Byte → Int → List Byte → Prop
  | unsigned (n input tail) (parsed : Parses codec input (.unsigned n) tail) :
      IntegerParses codec input (Int.ofNat n) tail
  | negative (n input tail) (parsed : Parses codec input (.negative n) tail) :
      IntegerParses codec input (-1 - Int.ofNat n) tail
  | positiveBig (bytes input tail)
      (parsed : Parses codec input (.tag 2 (.bytes bytes)) tail) (size : bytes.length ≤ 16) :
      IntegerParses codec input (Int.ofNat (magnitude bytes)) tail
  | negativeBig (bytes input tail)
      (parsed : Parses codec input (.tag 3 (.bytes bytes)) tail) (size : bytes.length ≤ 16)
      (range : magnitude bytes < 170141183460469231731687303715884105728) :
      IntegerParses codec input (-1 - Int.ofNat (magnitude bytes)) tail

def encodeUnsigned128 (n : Nat) : List Byte :=
  if n < 18446744073709551616 then encode (.unsigned n)
  else encode (.tag 2 (.bytes (word128 n)))

def encodeNegative128 (complement : Nat) : List Byte :=
  if complement < 18446744073709551616 then encode (.negative complement)
  else encode (.tag 3 (.bytes (word128 complement)))

theorem every_u128_has_integer_roundtrip (codec : FloatCodec) (n : Nat) (tail : List Byte)
    (bound : n < 340282366920938463463374607431768211456) :
    IntegerParses codec (encodeUnsigned128 n ++ tail) (Int.ofNat n) tail := by
  unfold encodeUnsigned128
  split
  · rename_i scalar
    exact .unsigned n _ tail (arbitrary_recursive_roundtrip _ tail scalar)
  · have fit : Fits (.tag 2 (.bytes (word128 n))) := by
      simp [Fits, word128, word64]
    have parsed := arbitrary_recursive_roundtrip (codec := codec) _ tail fit
    have h := IntegerParses.positiveBig (word128 n) _ tail parsed (by simp [word128, word64])
    simpa [word128_magnitude n bound] using h

theorem every_negative_i128_has_integer_roundtrip (codec : FloatCodec) (complement : Nat)
    (tail : List Byte) (bound : complement < 170141183460469231731687303715884105728) :
    IntegerParses codec (encodeNegative128 complement ++ tail) (-1 - Int.ofNat complement) tail := by
  unfold encodeNegative128
  split
  · rename_i scalar
    exact .negative complement _ tail (arbitrary_recursive_roundtrip _ tail scalar)
  · have wordBound : complement < 340282366920938463463374607431768211456 := by omega
    have fit : Fits (.tag 3 (.bytes (word128 complement))) := by simp [Fits, word128, word64]
    have parsed := arbitrary_recursive_roundtrip (codec := codec) _ tail fit
    have h := IntegerParses.negativeBig (word128 complement) _ tail parsed
      (by simp [word128, word64]) (by simpa [word128_magnitude complement wordBound] using bound)
    simpa [word128_magnitude complement wordBound] using h

inductive TagParses (codec : FloatCodec) : List Byte → TagMeaning → List Byte → Prop
  | interpreted (number : Nat) (value : Data) (input tail)
      (parsed : Parses codec input (.tag number value) tail) :
      TagParses codec input (interpretTag number value) tail

theorem encoded_tag_has_dependency_meaning (codec : FloatCodec) (number : Nat)
    (value : Data) (tail : List Byte) (fits : Fits (.tag number value)) :
    TagParses codec (encode (.tag number value) ++ tail) (interpretTag number value) tail :=
  .interpreted number value _ tail (arbitrary_recursive_roundtrip _ tail fits)

/-! Dependency normalization runs before the repository's resource visitor.
This function describes the definite-length bytes emitted by `encode` and
`encodeWithFloats`; raw indefinite tag-byte bodies retain their wire-form
distinction at the parser boundary described in the bridge contract. -/
def minimal128 (n : Nat) : List Byte := (word128 n).dropWhile (fun byte => byte.val == 0)

def normalizeTag (number : Nat) (value : Data) : Option Data :=
  match interpretTag number value with
  | .ordinary number value => some (.tag number value)
  | .positiveBig n =>
      if n < 18446744073709551616 then some (.unsigned n)
      else some (.tag 2 (.bytes (minimal128 n)))
  | .negativeBig n =>
      if n < 18446744073709551616 then some (.negative n)
      else some (.tag 3 (.bytes (minimal128 n)))
  | .rejectedNegativeOverflow => none

mutual
def normalize : Data → Option Data
  | .tag number value => do
      let decoded ← normalize value
      normalizeTag number decoded
  | .array values => do return .array (← normalizeValues values)
  | .map fields => do return .map (← normalizeFields fields)
  | value => some value
def normalizeValues : Values → Option Values
  | .nil => some .nil
  | .cons value rest => do return .cons (← normalize value) (← normalizeValues rest)
def normalizeFields : Fields → Option Fields
  | .nil => some .nil
  | .cons key value rest => do
      return .cons (← normalize key) (← normalize value) (← normalizeFields rest)
end

def decodePolicy (wireTree : Data) : Option Data := do
  let decoded ← normalize wireTree
  if resourceValid decoded then some decoded else none

inductive DecodedParses (codec : FloatCodec) : List Byte → Option Data → List Byte → Prop
  | encoded (wireTree : Data) (tail)
      (parsed : Parses codec (encodeWithFloats codec wireTree ++ tail) wireTree tail) :
      DecodedParses codec (encodeWithFloats codec wireTree ++ tail) (decodePolicy wireTree) tail

theorem recursive_encoding_composes_with_normalization (codec : FloatCodec)
    (widths : FloatWidths codec) (wireTree : Data) (tail : List Byte) (fits : Fits wireTree) :
    DecodedParses codec (encodeWithFloats codec wireTree ++ tail) (decodePolicy wireTree) tail :=
  .encoded wireTree tail (recursive_short_float_roundtrip codec widths wireTree tail fits)

theorem normalization_precedes_resource_validation (value : Data) (id : Nat)
    (decoded : normalize value = some (.unsigned id))
    (positive : 0 < id) (bound : id < 18446744073709551616) :
    decodePolicy (.tag 60000 value) = some (.tag 60000 (.unsigned id)) := by
  simp [decodePolicy, normalize, decoded, normalizeTag, interpretTag,
    resourceValid, resourceTag, positive, bound]

theorem nested_numeric_resource_reference_is_accepted :
    decodePolicy (.tag 60000 (.tag 2 (.bytes [octet 1]))) =
      some (.tag 60000 (.unsigned 1)) := by rfl

theorem overflowing_negative_tag_rejects (bytes : List Byte) (size : bytes.length ≤ 16)
    (overflow : 170141183460469231731687303715884105728 ≤ magnitude bytes) :
    normalize (.tag 3 (.bytes bytes)) = none := by
  simp [normalize, normalizeTag, excessive_negative_bignum_is_rejected bytes size overflow]

theorem map_key_normalization_failure_propagates (key value : Data) (rest : Fields)
    (failure : normalize key = none) : normalize (.map (.cons key value rest)) = none := by
  simp [normalize, normalizeFields, failure]

theorem nested_array_normalization_failure_propagates (value : Data) (rest : Values)
    (failure : normalize value = none) : normalize (.array (.cons value rest)) = none := by
  simp [normalize, normalizeValues, failure]

theorem map_key_bignum_overflow_rejects (bytes : List Byte) (value : Data) (rest : Fields)
    (size : bytes.length ≤ 16)
    (overflow : 170141183460469231731687303715884105728 ≤ magnitude bytes) :
    decodePolicy (.map (.cons (.tag 3 (.bytes bytes)) value rest)) = none := by
  have failed := map_key_normalization_failure_propagates (.tag 3 (.bytes bytes)) value rest
    (overflowing_negative_tag_rejects bytes size overflow)
  simp [decodePolicy, failed]

/-! Canonical bignum lowering preserves integer meaning at every tree node. -/
theorem dropping_leading_zeroes_preserves_magnitude (bytes : List Byte) :
    magnitude (bytes.dropWhile (fun byte => byte.val == 0)) = magnitude bytes := by
  induction bytes with
  | nil => rfl
  | cons byte rest ih =>
    by_cases zero : byte.val = 0
    · simpa [List.dropWhile_cons, zero, magnitude] using ih
    · simp [List.dropWhile_cons, zero]

theorem minimal128_preserves_magnitude (n : Nat)
    (bound : n < 340282366920938463463374607431768211456) :
    magnitude (minimal128 n) = n := by
  rw [minimal128, dropping_leading_zeroes_preserves_magnitude, word128_magnitude n bound]

theorem minimal128_length (n : Nat) : (minimal128 n).length ≤ 16 := by
  have h := (List.dropWhile_sublist (l := word128 n) (fun byte => byte.val == 0)).length_le
  simpa [minimal128, word128, word64] using h

def lowerUnsigned (n : Nat) : Data :=
  if n < 18446744073709551616 then .unsigned n else .tag 2 (.bytes (minimal128 n))

def lowerNegative (complement : Nat) : Data :=
  if complement < 18446744073709551616 then .negative complement
  else .tag 3 (.bytes (minimal128 complement))

theorem lowered_unsigned_fits (n : Nat) : Fits (lowerUnsigned n) := by
  unfold lowerUnsigned
  split
  · assumption
  · have h := minimal128_length n
    simp [Fits]
    omega

theorem lowered_negative_fits (n : Nat) : Fits (lowerNegative n) := by
  unfold lowerNegative
  split
  · assumption
  · have h := minimal128_length n
    simp [Fits]
    omega

theorem lowered_unsigned_is_normalized (n : Nat)
    (bound : n < 340282366920938463463374607431768211456) :
    normalize (lowerUnsigned n) = some (lowerUnsigned n) := by
  unfold lowerUnsigned
  split
  · rfl
  · rename_i large
    simp [normalize, normalizeTag, interpretTag, minimal128_length,
      minimal128_preserves_magnitude n bound, large]

theorem lowered_negative_is_normalized (n : Nat)
    (bound : n < 170141183460469231731687303715884105728) :
    normalize (lowerNegative n) = some (lowerNegative n) := by
  have wide : n < 340282366920938463463374607431768211456 := by omega
  unfold lowerNegative
  split
  · rfl
  · rename_i large
    simp [normalize, normalizeTag, interpretTag, minimal128_length,
      minimal128_preserves_magnitude n wide, large, bound]

/-- A scalar or recognized numeric tag denotes one mathematical integer. -/
inductive DenotesInteger : Data → Int → Prop
  | unsigned (n : Nat) (bound : n < 18446744073709551616) :
      DenotesInteger (.unsigned n) (Int.ofNat n)
  | negative (n : Nat) (bound : n < 18446744073709551616) :
      DenotesInteger (.negative n) (Int.negSucc n)
  | positiveBig (bytes : List Byte) (bound : bytes.length ≤ 16) :
      DenotesInteger (.tag 2 (.bytes bytes)) (Int.ofNat (magnitude bytes))
  | negativeBig (bytes : List Byte) (bound : bytes.length ≤ 16)
      (range : magnitude bytes < 170141183460469231731687303715884105728) :
      DenotesInteger (.tag 3 (.bytes bytes)) (Int.negSucc (magnitude bytes))

theorem lowering_u128_preserves_integer (n : Nat)
    (bound : n < 340282366920938463463374607431768211456) :
    DenotesInteger (lowerUnsigned n) (Int.ofNat n) := by
  unfold lowerUnsigned
  split
  · exact .unsigned n (by assumption)
  · have h := DenotesInteger.positiveBig (minimal128 n) (minimal128_length n)
    simpa [minimal128_preserves_magnitude n bound] using h

theorem lowering_negative_i128_preserves_integer (n : Nat)
    (bound : n < 170141183460469231731687303715884105728) :
    DenotesInteger (lowerNegative n) (Int.negSucc n) := by
  have wide : n < 340282366920938463463374607431768211456 := by omega
  unfold lowerNegative
  split
  · exact .negative n (by assumption)
  · have h := DenotesInteger.negativeBig (minimal128 n) (minimal128_length n)
      (by simpa [minimal128_preserves_magnitude n wide] using bound)
    simpa [minimal128_preserves_magnitude n wide] using h

/-! Mathematical trees put an `Int` at an integer leaf. A tag is an ordinary
tag only when its encoded immediate body is outside the special bignum form.
Thus tag 2/3 around arrays, numbers, tags or byte strings longer than 16 bytes
remains available; a shorter byte body is represented by its integer meaning. -/
mutual
inductive MathValue where
  | null | boolean (value : Bool) | integer (value : Int)
  | floating (bits : Fin 18446744073709551616)
  | bytes (value : List Byte) | text (utf8 : List Byte)
  | array (values : MathValues) | map (fields : MathFields)
  | tag (number : Nat) (value : MathValue)
inductive MathValues where
  | nil | cons (value : MathValue) (rest : MathValues)
inductive MathFields where
  | nil | cons (key value : MathValue) (rest : MathFields)
end

def mathValuesLength : MathValues → Nat
  | .nil => 0 | .cons _ rest => mathValuesLength rest + 1
def mathFieldsLength : MathFields → Nat
  | .nil => 0 | .cons _ _ rest => mathFieldsLength rest + 1

def ordinaryTagAllowed (number : Nat) : MathValue → Prop
  | .bytes bytes => (number ≠ 2 ∧ number ≠ 3) ∨ 16 < bytes.length
  | _ => True

mutual
def MathFits : MathValue → Prop
  | .null | .boolean _ | .floating _ => True
  | .integer (.ofNat n) => n < 340282366920938463463374607431768211456
  | .integer (.negSucc n) => n < 170141183460469231731687303715884105728
  | .bytes bytes => bytes.length < 18446744073709551616
  | .text bytes => bytes.length < 18446744073709551616 ∧ utf8Valid bytes = true
  | .array values => mathValuesLength values < 18446744073709551616 ∧ MathValuesFit values
  | .map fields => mathFieldsLength fields < 18446744073709551616 ∧ MathFieldsFit fields
  | .tag number value => number < 18446744073709551616 ∧ ordinaryTagAllowed number value ∧ MathFits value
def MathValuesFit : MathValues → Prop
  | .nil => True | .cons value rest => MathFits value ∧ MathValuesFit rest
def MathFieldsFit : MathFields → Prop
  | .nil => True | .cons key value rest => MathFits key ∧ MathFits value ∧ MathFieldsFit rest
end

mutual
def lower : MathValue → Data
  | .null => .null | .boolean value => .boolean value
  | .integer (.ofNat n) => lowerUnsigned n
  | .integer (.negSucc n) => lowerNegative n
  | .floating bits => .floating bits
  | .bytes bytes => .bytes bytes | .text bytes => .text bytes
  | .array values => .array (lowerValues values)
  | .map fields => .map (lowerFields fields)
  | .tag number value => .tag number (lower value)
def lowerValues : MathValues → Values
  | .nil => .nil | .cons value rest => .cons (lower value) (lowerValues rest)
def lowerFields : MathFields → Fields
  | .nil => .nil | .cons key value rest => .cons (lower key) (lower value) (lowerFields rest)
end

theorem lowering_preserves_array_length (values : MathValues) :
    valuesLength (lowerValues values) = mathValuesLength values := by
  cases values with
  | nil => rfl
  | cons value rest =>
    simp [valuesLength, lowerValues, mathValuesLength, lowering_preserves_array_length rest]

theorem lowering_preserves_map_length (fields : MathFields) :
    fieldsLength (lowerFields fields) = mathFieldsLength fields := by
  cases fields with
  | nil => rfl
  | cons key value rest =>
    simp [fieldsLength, lowerFields, mathFieldsLength, lowering_preserves_map_length rest]

theorem ordinary_lowered_tag_is_unchanged (number : Nat) (value : MathValue)
    (ordinary : ordinaryTagAllowed number value) :
    normalizeTag number (lower value) = some (.tag number (lower value)) := by
  cases value with
  | integer value =>
    cases value <;> simp only [lower, lowerUnsigned, lowerNegative]
    all_goals split <;> rfl
  | bytes bytes =>
    rcases ordinary with different | long
    · simp [lower, normalizeTag, interpretTag, different.1, different.2]
    · simp [lower, normalizeTag, interpretTag, show ¬bytes.length ≤ 16 by omega]
  | null => rfl
  | boolean _ => rfl
  | floating _ => rfl
  | text _ => rfl
  | array _ => rfl
  | map _ => rfl
  | tag _ _ => rfl

mutual
theorem every_lowered_tree_fits (value : MathValue) (fit : MathFits value) : Fits (lower value) := by
  cases value with
  | null => trivial
  | boolean _ => trivial
  | floating _ => trivial
  | integer value =>
    cases value with
    | ofNat n => exact lowered_unsigned_fits n
    | negSucc n => exact lowered_negative_fits n
  | bytes bytes => exact fit
  | text bytes => exact fit
  | array values => exact ⟨by simpa [lowering_preserves_array_length] using fit.1,
      every_lowered_values_fit values fit.2⟩
  | map fields => exact ⟨by simpa [lowering_preserves_map_length] using fit.1,
      every_lowered_fields_fit fields fit.2⟩
  | tag number value => exact ⟨fit.1, every_lowered_tree_fits value fit.2.2⟩
theorem every_lowered_values_fit (values : MathValues) (fit : MathValuesFit values) :
    ValuesFit (lowerValues values) := by
  cases values with
  | nil => trivial
  | cons value rest => exact ⟨every_lowered_tree_fits value fit.1, every_lowered_values_fit rest fit.2⟩
theorem every_lowered_fields_fit (fields : MathFields) (fit : MathFieldsFit fields) :
    FieldsFit (lowerFields fields) := by
  cases fields with
  | nil => trivial
  | cons key value rest => exact ⟨every_lowered_tree_fits key fit.1,
      every_lowered_tree_fits value fit.2.1, every_lowered_fields_fit rest fit.2.2⟩
end

mutual
theorem every_lowered_tree_is_normalized (value : MathValue) (fit : MathFits value) :
    normalize (lower value) = some (lower value) := by
  cases value with
  | null => rfl
  | boolean _ => rfl
  | floating _ => rfl
  | integer value =>
    cases value with
    | ofNat n => exact lowered_unsigned_is_normalized n fit
    | negSucc n => exact lowered_negative_is_normalized n fit
  | bytes bytes => rfl
  | text bytes => rfl
  | array values => simp [lower, normalize, every_lowered_values_are_normalized values fit.2]
  | map fields => simp [lower, normalize, every_lowered_fields_are_normalized fields fit.2]
  | tag number value =>
    simp [lower, normalize, every_lowered_tree_is_normalized value fit.2.2,
      ordinary_lowered_tag_is_unchanged number value fit.2.1]
theorem every_lowered_values_are_normalized (values : MathValues) (fit : MathValuesFit values) :
    normalizeValues (lowerValues values) = some (lowerValues values) := by
  cases values with
  | nil => rfl
  | cons value rest => simp [lowerValues, normalizeValues,
      every_lowered_tree_is_normalized value fit.1, every_lowered_values_are_normalized rest fit.2]
theorem every_lowered_fields_are_normalized (fields : MathFields) (fit : MathFieldsFit fields) :
    normalizeFields (lowerFields fields) = some (lowerFields fields) := by
  cases fields with
  | nil => rfl
  | cons key value rest => simp [lowerFields, normalizeFields,
      every_lowered_tree_is_normalized key fit.1, every_lowered_tree_is_normalized value fit.2.1,
      every_lowered_fields_are_normalized rest fit.2.2]
end

/-! This relation interprets numeric tags as integers independently of lower.
Arrays, map keys, map values and ordinary tags lift the relation recursively. -/
mutual
inductive Denotes : Data → MathValue → Prop
  | null : Denotes .null .null
  | boolean (value : Bool) : Denotes (.boolean value) (.boolean value)
  | integer (encoded : Data) (value : Int) (meaning : DenotesInteger encoded value) :
      Denotes encoded (.integer value)
  | floating (bits) : Denotes (.floating bits) (.floating bits)
  | bytes (bytes) : Denotes (.bytes bytes) (.bytes bytes)
  | text (bytes) : Denotes (.text bytes) (.text bytes)
  | array (encoded values) (meaning : ValuesDenote encoded values) : Denotes (.array encoded) (.array values)
  | map (encoded fields) (meaning : FieldsDenote encoded fields) : Denotes (.map encoded) (.map fields)
  | tag (number encoded value) (ordinary : ordinaryTagAllowed number value) (meaning : Denotes encoded value) :
      Denotes (.tag number encoded) (.tag number value)
inductive ValuesDenote : Values → MathValues → Prop
  | nil : ValuesDenote .nil .nil
  | cons (encoded value rest values) (meaning : Denotes encoded value) (others : ValuesDenote rest values) :
      ValuesDenote (.cons encoded rest) (.cons value values)
inductive FieldsDenote : Fields → MathFields → Prop
  | nil : FieldsDenote .nil .nil
  | cons (encodedKey key encodedValue value rest fields)
      (keyMeaning : Denotes encodedKey key) (valueMeaning : Denotes encodedValue value)
      (others : FieldsDenote rest fields) :
      FieldsDenote (.cons encodedKey encodedValue rest) (.cons key value fields)
end

mutual
theorem every_lowered_tree_preserves_meaning (value : MathValue) (fit : MathFits value) :
    Denotes (lower value) value := by
  cases value with
  | null => exact .null
  | boolean value => exact .boolean value
  | floating bits => exact .floating bits
  | integer value =>
    cases value with
    | ofNat n => exact .integer _ _ (lowering_u128_preserves_integer n fit)
    | negSucc n => exact .integer _ _ (lowering_negative_i128_preserves_integer n fit)
  | bytes bytes => exact .bytes bytes
  | text bytes => exact .text bytes
  | array values => exact .array _ _ (every_lowered_values_preserve_meaning values fit.2)
  | map fields => exact .map _ _ (every_lowered_fields_preserve_meaning fields fit.2)
  | tag number value => exact .tag _ _ _ fit.2.1 (every_lowered_tree_preserves_meaning value fit.2.2)
theorem every_lowered_values_preserve_meaning (values : MathValues) (fit : MathValuesFit values) :
    ValuesDenote (lowerValues values) values := by
  cases values with
  | nil => exact .nil
  | cons value rest =>
    exact .cons _ _ _ _ (every_lowered_tree_preserves_meaning value fit.1)
      (every_lowered_values_preserve_meaning rest fit.2)
theorem every_lowered_fields_preserve_meaning (fields : MathFields) (fit : MathFieldsFit fields) :
    FieldsDenote (lowerFields fields) fields := by
  cases fields with
  | nil => exact .nil
  | cons key value rest =>
    exact .cons _ _ _ _ _ _
      (every_lowered_tree_preserves_meaning key fit.1) (every_lowered_tree_preserves_meaning value fit.2.1)
      (every_lowered_fields_preserve_meaning rest fit.2.2)
end

theorem recursive_integer_tree_roundtrip (codec : FloatCodec) (widths : FloatWidths codec)
    (value : MathValue) (tail : List Byte) (fit : MathFits value)
    (resources : resourceValid (lower value) = true) :
    DecodedParses codec (encodeWithFloats codec (lower value) ++ tail) (some (lower value)) tail ∧
      Denotes (lower value) value := by
  have parsed := recursive_encoding_composes_with_normalization codec widths (lower value) tail
    (every_lowered_tree_fits value fit)
  have normalized : decodePolicy (lower value) = some (lower value) := by
    simp [decodePolicy, every_lowered_tree_is_normalized value fit, resources]
  rw [normalized] at parsed
  exact ⟨parsed, every_lowered_tree_preserves_meaning value fit⟩

end Cordis.CborSemantics
