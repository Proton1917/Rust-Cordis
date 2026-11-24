import asyncio
from pathlib import Path
from types import SimpleNamespace
import unittest

import cbor2
from cordis_sdk import CordisError, Runtime
from cordis_sdk.budget import check_encoded, decode, encode


class BudgetsTest(unittest.TestCase):
    def test_shared_vectors(self):
        vectors = Path(__file__).parents[1] / "up1/value-budgets.tsv"
        for line in vectors.read_text().splitlines():
            if line.startswith("#"):
                continue
            name, depth, collection, values, frame, accepted, encoded = line.split("\t")
            limits = SimpleNamespace(max_depth=int(depth), max_collection=int(collection),
                                     max_values=int(values), max_frame=int(frame))
            data = bytes.fromhex(encoded)
            with self.subTest(name=name):
                if accepted == "1":
                    message = decode(data, limits)
                    self.assertEqual(decode(encode(message, limits), limits), message)
                else:
                    with self.assertRaises(CordisError) as rejected:
                        decode(data, limits)
                    self.assertEqual(rejected.exception.code, 8)

    def test_declared_collection_rejected_before_decode(self):
        limits = Runtime("test", "1", []).connection
        # Complete envelope followed by an oversized declared array, without its elements.
        data = cbor2.dumps({"request": {"id": 1, "method": "probe", "params": None}})[:-1]
        data += b"\x9a\x00\x01\x00\x01"
        with self.assertRaises(CordisError) as rejected:
            check_encoded(data, limits)
        self.assertEqual(rejected.exception.detail["limit"], "max_collection")

    def test_mathbook_shape_rejects_before_encoding(self):
        limits = Runtime("test", "1", []).connection
        graph = [[0] * size for size in [65536, 65536, 65536, 65536, 1933]]
        with self.assertRaises(CordisError) as rejected:
            encode({"request": {"id": 1, "method": "probe", "params": graph}}, limits)
        self.assertEqual(rejected.exception.detail["limit"], "max_values")


class ConnectionBudgetTest(unittest.IsolatedAsyncioTestCase):
    async def test_local_rejection_returns_offers(self):
        runtime = Runtime("test", "1", [], max_collection=1)
        offered = runtime.offer(object())
        with self.assertRaises(CordisError) as rejected:
            await runtime.connection.notify("probe", [0, 1], [offered])
        self.assertEqual(rejected.exception.code, 8)
        self.assertNotIn(offered, runtime.exports)

    async def test_input_rejected_before_dispatch(self):
        runtime = Runtime("test", "1", [], max_collection=1)
        reader = asyncio.StreamReader()
        data = cbor2.dumps({"request": {"id": 1, "method": "hello", "params": [0, 1]}})
        reader.feed_data(len(data).to_bytes(4, "big") + data)
        reader.feed_eof()
        with self.assertRaises(CordisError) as rejected:
            await runtime.connection._read(reader)
        self.assertEqual(rejected.exception.code, 8)
        self.assertEqual(runtime.connection.running, {})


if __name__ == "__main__":
    unittest.main()
