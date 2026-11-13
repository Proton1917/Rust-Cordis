import io
from pathlib import Path

from .bindings import OwnedResource, Remote
from .errors import CordisError


MAX_CHUNK_BYTES = 4 * 1024 * 1024


def read_size(size):
    if type(size) is not int or not 0 < size <= MAX_CHUNK_BYTES:
        raise CordisError(2, "字节流读取长度需要为 1 至 4194304")
    return size


class ByteStream(OwnedResource):
    """ADR-0046 §3.8：读者拥有文件或字节迭代器，出口归零时释放读者。"""

    def __init__(self, reader):
        super().__init__()
        self.reader = reader

    async def read(self, max_bytes):
        size = read_size(max_bytes)
        if self.reader is None:
            return None
        data = self.reader.read(size)
        if data:
            return data
        self.close()
        return None

    def close(self):
        if self.reader is not None:
            self.reader.close()
            self.reader = None

    def __del__(self):
        self.close()


class Chunks(io.RawIOBase):
    def __init__(self, chunks):
        self.chunks = iter(chunks)
        self.pending = memoryview(b"")

    def read(self, size):
        while not self.pending:
            value = next(self.chunks, None)
            if value is None:
                return b""
            self.pending = memoryview(value)
        data = self.pending[:size].tobytes()
        self.pending = self.pending[len(data):]
        return data

    def close(self):
        self.pending = memoryview(b"")
        closer = getattr(self.chunks, "close", None)
        if closer is not None:
            closer()
        super().close()


def from_file(path):
    return ByteStream(Path(path).open("rb"))


def from_chunks(chunks):
    return ByteStream(Chunks(chunks))


async def copy_to(source, callback, chunk_bytes=1024 * 1024):
    """ADR-0046 §3.8：逐块交给文件或异步回调，退出时归还远端流。"""
    import inspect

    read_size(chunk_bytes)
    total = 0
    try:
        while (chunk := await source.read(chunk_bytes)) is not None:
            if not chunk or len(chunk) > chunk_bytes:
                raise CordisError(2, "字节流块长度超出读取契约")
            result = callback(chunk)
            if inspect.isawaitable(result):
                await result
            total += len(chunk)
        return total
    finally:
        remote = getattr(source, "_transport", None)
        if isinstance(remote, Remote):
            remote.close()


async def to_file(source, path, chunk_bytes=1024 * 1024):
    with Path(path).open("wb") as destination:
        return await copy_to(source, destination.write, chunk_bytes)
