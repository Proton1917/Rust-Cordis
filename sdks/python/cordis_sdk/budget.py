"""DOC-16 §3: bounded CBOR structure before allocation and value graph before dispatch."""
import io

import cbor2

from .errors import CordisError

_DOMAINS = {
    ("request", "params"), ("notification", "params"),
    ("response", "result", "ok"), ("response", "result", "error", "detail"),
}
_PREFIXES = {path[:length] for path in _DOMAINS for length in range(len(path) + 1)}
_PLAIN_SCALARS = (type(None), bool, int, float, str, bytes, bytearray)
_NON_SEMANTIC_TAGS = frozenset((2, 3, 60000))


def exceeded(name, maximum):
    return CordisError(8, f"UP/1 value exceeds {name}", {"limit": name, "maximum": maximum})


def check_value(root, limits, depth=0):
    """Count keys, values and tags; retain at most one iterator per container depth.

    Returns True when a semantic/non-primitive Python object is encountered that
    cbor2 may expand into extra CBOR tags or shared references.
    """
    pending = [(iter((root,)), depth)]
    nodes = 0
    needs_wire_recheck = False
    while pending:
        iterator, depth = pending[-1]
        try:
            value = next(iterator)
        except StopIteration:
            pending.pop()
            continue
        nodes += 1
        if nodes > limits.max_values:
            raise exceeded("max_values", limits.max_values)
        children = None
        kind = type(value)
        if kind is cbor2.CBORTag:
            if value.tag not in _NON_SEMANTIC_TAGS:
                needs_wire_recheck = True
            children = iter((value.value,))
        elif kind is list or kind is tuple:
            if len(value) > limits.max_collection:
                raise exceeded("max_collection", limits.max_collection)
            children = iter(value)
        elif kind is dict:
            if len(value) > limits.max_collection:
                raise exceeded("max_collection", limits.max_collection)
            children = (part for pair in value.items() for part in pair)
        elif kind is int:
            if not -(1 << 64) <= value < 1 << 64:
                # cbor2 represents larger integers by a bignum tag and byte-string child.
                children = iter((b"",))
        elif kind not in _PLAIN_SCALARS:
            needs_wire_recheck = True
            if isinstance(value, cbor2.CBORTag):
                children = iter((value.value,))
            elif isinstance(value, (list, tuple)):
                if len(value) > limits.max_collection:
                    raise exceeded("max_collection", limits.max_collection)
                children = iter(value)
            elif isinstance(value, dict):
                if len(value) > limits.max_collection:
                    raise exceeded("max_collection", limits.max_collection)
                children = (part for pair in value.items() for part in pair)
        if children is not None:
            if depth >= limits.max_depth:
                raise exceeded("max_depth", limits.max_depth)
            pending.append((children, depth + 1))
    return needs_wire_recheck


def check_message(message, limits):
    needs_wire_recheck = type(message) is not dict
    if not needs_wire_recheck:
        # Check non-domain envelope fields (tiny fixed structure) for semantic types.
        stack = [(message, ())]
        while stack:
            node, path = stack.pop()
            if path in _DOMAINS:
                continue
            if type(node) is dict:
                for key, val in node.items():
                    if type(key) is not str:
                        needs_wire_recheck = True
                        break
                    child = path + (key,)
                    if child in _DOMAINS:
                        continue
                    if type(val) is dict and child in _PREFIXES:
                        stack.append((val, child))
                    elif type(val) not in _PLAIN_SCALARS:
                        needs_wire_recheck = True
                        break
            else:
                needs_wire_recheck = True
            if needs_wire_recheck:
                break
    for path in _DOMAINS:
        value = message
        for key in path:
            if not isinstance(value, dict) or key not in value:
                break
            value = value[key]
        else:
            if check_value(value, limits, len(path)):
                needs_wire_recheck = True
    return needs_wire_recheck


def _read_head(data, position):
    if position >= len(data):
        raise CordisError(2, "Truncated CBOR value")
    initial = data[position]
    position += 1
    major, info = initial >> 5, initial & 31
    if info < 24:
        return major, info, position
    if info < 28:
        size = 1 << (info - 24)
        end = position + size
        if end > len(data):
            raise CordisError(2, "Truncated CBOR value")
        return major, int.from_bytes(data[position:end], "big"), end
    if info == 31 and major in (2, 3, 4, 5):
        return major, None, position
    raise CordisError(2, "Invalid CBOR additional information")


def _skip_string(data, position, major, argument, capture_key):
    if argument is not None:
        end = position + argument
        if end > len(data):
            raise CordisError(2, "Truncated CBOR value")
        text = (
            bytes(data[position:end]).decode("utf-8")
            if capture_key and major == 3 and argument <= 64
            else None
        )
        return text, end
    # Indefinite strings are scalar values. Their chunks have the same major
    # type and definite length, as required by CBOR.
    parts = [] if capture_key and major == 3 else None
    total = 0
    while position < len(data) and data[position] != 255:
        chunk_major, chunk_len, position = _read_head(data, position)
        if chunk_major != major or chunk_len is None:
            raise CordisError(2, "Invalid indefinite CBOR string")
        end = position + chunk_len
        if end > len(data):
            raise CordisError(2, "Truncated CBOR value")
        if parts is not None:
            if total + chunk_len > 64:
                parts = None
            else:
                parts.append(bytes(data[position:end]).decode("utf-8"))
                total += chunk_len
        position = end
    if position >= len(data):
        raise CordisError(2, "Truncated CBOR value")
    position += 1  # consume break byte 0xff
    return ("".join(parts) if parts is not None else None), position


def check_encoded(data, limits):
    """Iteratively scan wire structure with bounded depth before cbor2 allocates collections.

    Envelope fields contribute depth. Collection and node budgets belong to each
    params/result/detail value, matching the Rust codec. String bytes are skipped
    in place; only short envelope keys are materialized.

    Returns True when a semantic CBOR tag (outside bignum/object tags) is seen.
    """
    if len(data) > limits.max_frame:
        raise exceeded("max_frame", limits.max_frame)
    position = 0
    nodes = 0
    has_semantic_tag = False
    # Stack entries:
    # ("val", depth, path, counted)
    # ("arr", depth, remaining_or_None, size, counted)
    # ("map_key", depth, path, remaining_or_None, size, counted)
    stack = [("val", 0, (), False)]
    while stack:
        frame = stack.pop()
        kind = frame[0]
        if kind == "arr":
            _, depth, remaining, size, counted = frame
            if remaining is None:
                if position < len(data) and data[position] == 255:
                    position += 1
                    continue
                size += 1
                if counted and size > limits.max_collection:
                    raise exceeded("max_collection", limits.max_collection)
                stack.append(("arr", depth, None, size, counted))
                stack.append(("val", depth, None, counted))
            elif remaining > 0:
                size += 1
                if counted and size > limits.max_collection:
                    raise exceeded("max_collection", limits.max_collection)
                if remaining > 1:
                    stack.append(("arr", depth, remaining - 1, size, counted))
                stack.append(("val", depth, None, counted))
            continue
        if kind == "map_key":
            _, depth, path, remaining, size, counted = frame
            if remaining is None:
                if position < len(data) and data[position] == 255:
                    position += 1
                    continue
                next_remaining = None
            elif remaining > 0:
                next_remaining = remaining - 1
            else:
                continue
            size += 1
            if counted and size > limits.max_collection:
                raise exceeded("max_collection", limits.max_collection)
            if next_remaining is None or next_remaining > 0:
                stack.append(("map_key", depth, path, next_remaining, size, counted))
            track_path = path in _PREFIXES
            if track_path:
                # Parse key inline if it's a text string so we can compute child path.
                if counted:
                    nodes += 1
                    if nodes > limits.max_values:
                        raise exceeded("max_values", limits.max_values)
                key_major, key_arg, position = _read_head(data, position)
                if key_major in (2, 3):
                    key_text, position = _skip_string(data, position, key_major, key_arg, True)
                    child = path + (key_text,) if key_text is not None else None
                    stack.append(("val", depth, child, counted))
                elif key_major in (0, 1, 7):
                    stack.append(("val", depth, None, counted))
                else:
                    # Non-scalar map key: push value first, then parse complex key.
                    stack.append(("val", depth, None, counted))
                    if depth >= limits.max_depth:
                        raise exceeded("max_depth", limits.max_depth)
                    if key_major == 6:
                        if key_arg not in _NON_SEMANTIC_TAGS:
                            has_semantic_tag = True
                        stack.append(("val", depth + 1, None, counted))
                    elif key_major == 4:
                        if counted and key_arg is not None and key_arg > limits.max_collection:
                            raise exceeded("max_collection", limits.max_collection)
                        stack.append(("arr", depth + 1, key_arg, 0, counted))
                    elif key_major == 5:
                        if counted and key_arg is not None and key_arg > limits.max_collection:
                            raise exceeded("max_collection", limits.max_collection)
                        stack.append(("map_key", depth + 1, None, key_arg, 0, counted))
                    else:
                        raise CordisError(2, "Invalid CBOR major type")
            else:
                stack.append(("val", depth, None, counted))
                stack.append(("val", depth, None, counted))
            continue

        # kind == "val"
        _, depth, path, counted = frame
        if path in _DOMAINS and not counted:
            counted = True
            nodes = 0
        if counted:
            nodes += 1
            if nodes > limits.max_values:
                raise exceeded("max_values", limits.max_values)
        major, argument, position = _read_head(data, position)
        if major in (4, 5, 6) and depth >= limits.max_depth:
            raise exceeded("max_depth", limits.max_depth)
        if major in (0, 1, 7):
            continue
        if major in (2, 3):
            _, position = _skip_string(data, position, major, argument, False)
            continue
        if major == 6:
            if argument not in _NON_SEMANTIC_TAGS:
                has_semantic_tag = True
            stack.append(("val", depth + 1, None, counted))
            continue
        if major not in (4, 5):
            raise CordisError(2, "Invalid CBOR major type")
        if counted and argument is not None and argument > limits.max_collection:
            raise exceeded("max_collection", limits.max_collection)
        if major == 4:
            if argument is None or argument > 0:
                stack.append(("arr", depth + 1, argument, 0, counted))
        else:
            if argument is None or argument > 0:
                stack.append(("map_key", depth + 1, path, argument, 0, counted))

    if position != len(data):
        raise CordisError(2, "Trailing bytes after CBOR message")
    return has_semantic_tag


class _LimitedBuffer(io.BytesIO):
    def __init__(self, maximum):
        super().__init__()
        self.maximum = maximum

    def write(self, data):
        if self.tell() + len(data) > self.maximum:
            raise exceeded("max_frame", self.maximum)
        return super().write(data)


def encode(message, limits):
    needs_wire_recheck = check_message(message, limits)
    output = _LimitedBuffer(limits.max_frame)
    cbor2.CBOREncoder(output).encode(message)
    data = output.getvalue()
    # Semantic Python values (e.g. datetime, Decimal, set, shared refs) may be
    # encoded by cbor2 as tagged structures beyond what check_message counted.
    if needs_wire_recheck:
        check_encoded(data, limits)
    return data


def decode(data, limits):
    has_semantic_tag = check_encoded(data, limits)
    message = cbor2.loads(data)
    # Semantic CBOR tags may produce shared Python objects; account their actual
    # graph before handing it to runtime code, including cycles and alias expansion.
    if has_semantic_tag:
        check_message(message, limits)
    return message
