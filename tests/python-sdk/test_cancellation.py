import asyncio
import socket
import struct
import sys
import unittest

import cbor2
from cordis_sdk import CordisError, Runtime
from cordis_sdk.bindings import Service
from cordis_sdk.context import evaluate


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

    async def test_invoke_budget_accounts_for_writer_backpressure(self):
        client, peer = socket.socketpair()
        client.setsockopt(socket.SOL_SOCKET, socket.SO_SNDBUF, 8192)
        _, writer = await asyncio.open_connection(sock=client)
        reader, peer_writer = await asyncio.open_connection(sock=peer, limit=1024)
        connection = Runtime("caller", "1.0.0", [], timeout=2).connection
        connection.peer_budget = True
        connection.writer = writer
        writer.transport.set_write_buffer_limits(high=8192)
        loop = asyncio.get_running_loop()
        started = loop.create_future()
        args = ["原始参数"]

        async def invoke():
            started.set_result(loop.time())
            return await connection.request("invoke", {
                "object": 7, "method": "leaf", "args": args, "refs": [],
            })

        try:
            async with asyncio.timeout(3):
                await connection.notify("log", {"message": "x" * (512 * 1024)})
                self.assertGreater(writer.transport.get_write_buffer_size(), 8192)
                request = asyncio.create_task(invoke())
                try:
                    started_at = await started
                    args[0] = "更新后的参数"
                    await asyncio.sleep(0.15)
                    remaining = int((connection.timeout - (loop.time() - started_at)) * 1000)

                    self.assertEqual((await receive(reader))["notification"]["method"], "log")
                    call = (await receive(reader))["request"]
                    self.assertIn(call["params"]["budget_ms"], range(1, remaining + 10))
                    self.assertEqual(call["params"]["args"], ["原始参数"])
                finally:
                    request.cancel()
                    await asyncio.gather(request, return_exceptions=True)
        finally:
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

    async def test_negotiated_deadline_reduces_the_nested_request_budget(self):
        plugin = await asyncio.create_subprocess_exec(
            sys.executable, "-B", __file__, "deadline-peer",
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        )
        try:
            async with asyncio.timeout(5):
                plugin.stdin.write(frame({"request": {"id": 1, "method": "hello", "params": {"protocol": 1}}}))
                await plugin.stdin.drain()
                hello = (await receive(plugin.stdout))["response"]
                self.assertIs(hello["result"]["ok"]["invoke_budget_v1"], True)

                params = {"object": 1, "method": "relay", "args": [], "refs": [], "budget_ms": 1000}
                plugin.stdin.write(frame({"request": {"id": 2, "method": "invoke", "params": params}}))
                await plugin.stdin.drain()
                nested = (await receive(plugin.stdout))["request"]
                self.assertEqual(nested["params"]["method"], "leaf")
                self.assertIn(nested["params"]["budget_ms"], range(1, 981))
                plugin.stdin.write(frame({"response": {"id": nested["id"], "result": {"ok": {"value": 91, "refs": []}}}}))
                await plugin.stdin.drain()
                result = (await receive(plugin.stdout))["response"]
                self.assertEqual((result["id"], result["result"]["ok"]["value"]), (2, 91))
        finally:
            plugin.stdin.close()
            try:
                await asyncio.wait_for(plugin.wait(), 5)
            except TimeoutError:
                plugin.kill()
                await plugin.wait()
        self.assertEqual(plugin.returncode, 0)

    async def test_expired_invoke_skips_synchronous_dispatch(self):
        plugin = await asyncio.create_subprocess_exec(
            sys.executable, "-B", __file__, "deadline-peer",
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
        )
        try:
            async with asyncio.timeout(5):
                results = []
                for request_id, budget in ((1, 0), (2, 1000)):
                    params = {
                        "object": 1, "method": "record", "args": [], "refs": [],
                        "budget_ms": budget,
                    }
                    plugin.stdin.write(frame({"request": {
                        "id": request_id, "method": "invoke", "params": params,
                    }}))
                    await plugin.stdin.drain()
                    response = (await receive(plugin.stdout))["response"]
                    results.append(response["result"])
                expired, current = results
                self.assertEqual(expired["error"]["code"], 6)
                self.assertEqual(current["ok"]["value"], 1)
        finally:
            plugin.stdin.close()
            try:
                await asyncio.wait_for(plugin.wait(), 5)
            except TimeoutError:
                plugin.kill()
                await plugin.wait()
        self.assertEqual(plugin.returncode, 0)


async def deadline_peer():
    runtime = Runtime("relay", "1.0.0", [])
    interface = "test.deadline@1.0"

    class Relay:
        def __init__(self):
            self.records = 0

        def record(self):
            self.records += 1
            return self.records

        async def relay(self):
            await asyncio.sleep(0.02)
            nested = await runtime.connection.request("invoke", {"object": 7, "method": "leaf", "args": [], "refs": []})
            try:
                return nested.value["value"]
            finally:
                nested.close()

    class Binding:
        SCHEMA = {"services": [{
            "interface": interface, "methods": [{"name": "relay"}, {"name": "record"}],
        }]}

        @staticmethod
        async def dispatch(provider, transport, name, method, args):
            return await evaluate(getattr(provider, method), *args)

    runtime.offer(Service(Relay(), Binding, interface))
    await runtime.connection.serve()


if __name__ == "__main__":
    if sys.argv[1:] == ["deadline-peer"]:
        asyncio.run(deadline_peer())
    else:
        unittest.main()
