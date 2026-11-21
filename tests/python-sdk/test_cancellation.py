import asyncio
import struct
import sys
import unittest

import cbor2


def frame(message):
    data = cbor2.dumps(message)
    return struct.pack(">I", len(data)) + data


async def receive(reader):
    length, = struct.unpack(">I", await reader.readexactly(4))
    return cbor2.loads(await reader.readexactly(length))


class CancellationTest(unittest.IsolatedAsyncioTestCase):
    async def test_cancel_before_dispatch_releases_slot_and_reference(self):
        plugin = await asyncio.create_subprocess_exec(
            sys.executable, "-B", "-c",
            "from cordis_sdk import run; run('worker', [], max_inflight=1)",
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        )
        try:
            async with asyncio.timeout(5):
                request = {"request": {"id": 1, "method": "invoke", "params": {"refs": [23]}}}
                cancel = {"notification": {"method": "cancel", "params": {"id": 1}}}
                plugin.stdin.write(frame(request) + frame(cancel))
                await plugin.stdin.drain()
                replies = [await receive(plugin.stdout) for _ in range(2)]
                response, = [item["response"] for item in replies if "response" in item]
                self.assertEqual(response["result"]["error"]["code"], 7)
                self.assertIn({"notification": {"method": "release", "params": {"objects": [23]}}}, replies)
                plugin.stdin.write(frame({"request": {"id": 2, "method": "hello", "params": {"protocol": 1}}}))
                await plugin.stdin.drain()
                hello = (await receive(plugin.stdout))["response"]
                self.assertEqual(hello["result"]["ok"]["plugins"][0]["name"], "worker")
        finally:
            plugin.stdin.close()
            try:
                await asyncio.wait_for(plugin.wait(), 5)
            except TimeoutError:
                plugin.kill()
                await plugin.wait()
        self.assertEqual(plugin.returncode, 0)


if __name__ == "__main__":
    unittest.main()
