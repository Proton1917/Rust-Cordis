"""DOC-16 §3: bounded CBOR structure before allocation and value graph before dispatch."""
import io

import cbor2

from .errors import CordisError

_DOMAINS = {
    ("request", "params"), ("notification", "params"),
    ("response", "result", "ok"), ("response", "result", "error", "detail"),
}
_PREFIXES = {path[:length] for path in _DOMAINS for length in range(len(path) + 1)}


def exceeded(name, maximum):
    return CordisError(8, f"UP/1 value exceeds {name}", {"limit": name, "maximum": maximum})


def check_value(root, limits, depth=0):
    """Count keys, values and tags; retain at most one iterator per container depth."""
    pending = [(iter((root,)), depth)]
    nodes = 0
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
        elif type(value) is int and not -(1 << 64) <= value < 1 << 64:
            # cbor2 represents larger integers by a bignum tag and byte-string child.
            children = iter((b"",))
        if children is not None:
            if depth >= limits.max_depth:
                raise exceeded("max_depth", limits.max_depth)
            pending.append((children, depth + 1))


def check_message(message, limits):
    for path in _DOMAINS:
        value = message
        for key in path:
            if not isinstance(value, dict) or key not in value:
                break
            value = value[key]
        else:
            check_value(value, limits, len(path))


def check_encoded(data, limits):
    """Scan wire structure with bounded depth before cbor2 creates collections.

    Envelope fields contribute depth. Collection and node budgets belong to each
    params/result/detail value, matching the Rust codec. String bytes are skipped
    in place; only short envelope keys are materialized.
    """
    if len(data) > limits.max_frame:
        raise exceeded("max_frame", limits.max_frame)
    position = 0
    nodes = 0

    def take(size):
        nonlocal position
        end = position + size
        if end > len(data):
            raise CordisError(2, "Truncated CBOR value")
        start = position
        position = end
        return start

    def value(depth, path=(), counted=False, key=False):
        nonlocal nodes
        if path in _DOMAINS and not counted:
            counted = True
            nodes = 0
        if counted:
            nodes += 1
            if nodes > limits.max_values:
                raise exceeded("max_values", limits.max_values)
        initial = data[take(1)]
        major, info = initial >> 5, initial & 31
        if info < 24:
            argument = info
        elif info < 28:
            size = 1 << (info - 24)
            start = take(size)
            argument = int.from_bytes(data[start:start + size], "big")
        elif info == 31 and major in (2, 3, 4, 5):
            argument = None
        else:
            raise CordisError(2, "Invalid CBOR additional information")
        if major in (4, 5, 6) and depth >= limits.max_depth:
            raise exceeded("max_depth", limits.max_depth)
        if major in (0, 1, 7):
            return None
        if major in (2, 3):
            if argument is not None:
                start = take(argument)
                if key and major == 3 and argument <= 64:
                    return bytes(data[start:start + argument]).decode("utf-8")
            else:
                # Indefinite strings are scalar values. Their chunks have the
                # same major type and definite length, as required by CBOR.
                parts = [] if key else None
                total = 0
                while position < len(data) and data[position] != 255:
                    start = position
                    if data[start] >> 5 != major or data[start] & 31 == 31:
                        raise CordisError(2, "Invalid indefinite CBOR string")
                    part = value(depth, None, False, key)
                    if parts is not None:
                        if part is None or total + len(part) > 64:
                            parts = None
                        else:
                            parts.append(part)
                            total += len(part)
                take(1)
                if parts is not None:
                    return "".join(parts)
            return None
        if major == 6:
            value(depth + 1, None, counted)
            return None
        if major not in (4, 5):
            raise CordisError(2, "Invalid CBOR major type")
        if counted and argument is not None and argument > limits.max_collection:
            raise exceeded("max_collection", limits.max_collection)
        size = 0
        while argument is None or size < argument:
            if argument is None and position < len(data) and data[position] == 255:
                take(1)
                return None
            size += 1
            if counted and size > limits.max_collection:
                raise exceeded("max_collection", limits.max_collection)
            if major == 4:
                value(depth + 1, None, counted)
            else:
                name = value(depth + 1, None, counted, path in _PREFIXES)
                child = path + (name,) if path in _PREFIXES and name is not None else None
                value(depth + 1, child, counted)
        return None

    value(0)
    if position != len(data):
        raise CordisError(2, "Trailing bytes after CBOR message")


class _LimitedBuffer(io.BytesIO):
    def __init__(self, maximum):
        super().__init__()
        self.maximum = maximum

    def write(self, data):
        if self.tell() + len(data) > self.maximum:
            raise exceeded("max_frame", self.maximum)
        return super().write(data)


def encode(message, limits):
    check_message(message, limits)
    output = _LimitedBuffer(limits.max_frame)
    cbor2.CBOREncoder(output).encode(message)
    data = output.getvalue()
    # Also covers semantic Python values encoded by cbor2 as tagged structures.
    check_encoded(data, limits)
    return data


def decode(data, limits):
    check_encoded(data, limits)
    message = cbor2.loads(data)
    # Semantic CBOR tags may produce shared Python objects; account their actual
    # graph before handing it to runtime code, including cycles and alias expansion.
    check_message(message, limits)
    return message
