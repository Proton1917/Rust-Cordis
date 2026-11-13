from dataclasses import dataclass
from .errors import CordisError


@dataclass(frozen=True)
class Caller:
    instance: str | None = None
    page_of: str | None = None


def caller(metadata):
    value = metadata.get("cordis.caller")
    if value is None:
        return None
    if not isinstance(value, dict) or set(value) not in ({"instance"}, {"page_of"}):
        raise CordisError(2, "调用身份需要 instance 或 page_of")
    if not isinstance(next(iter(value.values())), str):
        raise CordisError(2, "调用身份名称需要字符串")
    return Caller(**value)
