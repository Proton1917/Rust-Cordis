import asyncio
import socket
import struct
import sys
import unittest

import cbor2
from cordis_sdk import CordisError, Runtime


def frame(message):
    data = cbor2.dumps(message)
    return struct.pack(">I", len(data)) + data


async def receive(reader):
    length, = struct.unpack(">I", await reader.readexactly(4))
    return cbor2.loads(await reader.readexactly(length))


class CancellationTest(unittest.IsolatedAsyncioTestCase):
    async def test_request_deadline_precedes_cancel_delivery_under_backpressure(self):
        client, peer = socket.socketpair()
        client.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 8192)
        _, writer = await asyncio.open_connection(sock=client)
        reader, peer_writer = await asyncio.open_connection(sock=peer, limit=1024)
        connection = Runtime("caller", "1.0.0", [], timeout=0.2).connection
        connection.writer = writer
        writer.transport.set_write_buffer_limits(high=8192)
        sender = asyncio.create_task(connection._control_messages())
        try:
            async with asyncio.timeout(3):
                request = asyncio.create_task(connection.request("hello", {"protocol": 1}))
                incoming = (await receive(reader))["request"]
                await connection.notify("log", {"message": "x" * (512 * 1024)})
                self.assertGreater(writer.transport.get_write_buffer_size(), 8192)
                with self.assertRaises(CordisError) as result:
                    await request
                self.assertEqual(result.exception.code, 6)
                self.assertEqual(connection.pending, {})
                self.assertEqual((await receive(reader))["notification"]["method"], "log")
                cancel = (await receive(reader))["notification"]
                self.assertEqual(cancel, {"method": "cancel", "params": {"id": incoming["id"]}})
        finally:
            sender.cancel()
            await asyncio.gather(sender, return_exceptions=True)
            writer.transport.abort()
            peer_writer.close()
            await asyncio.gather(writer.wait_closed(), peer_writer.wait_closed(), return_exceptions=True)

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
