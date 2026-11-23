import asyncio
import unittest

import cbor2
from cordis_sdk import CordisError, Runtime
from cordis_sdk.context import Callback, Context


def notification(method, params):
    body = cbor2.dumps({"notification": {"method": method, "params": params}})
    return len(body).to_bytes(4, "big") + body


def params(callback, context=1, reference=101, value=None):
    return {"object": callback, "context": context, "scope": 0,
            "payload": value, "refs": [reference]}


class Writer:
    def __init__(self):
        self.frames = []
        self.written = asyncio.Event()
        self.closed = False

    async def drain(self):
        return

    def write(self, value):
        self.frames.append(cbor2.loads(value[4:]))
        self.written.set()

    def close(self):
        self.closed = True


class CallbackAdmissionTest(unittest.IsolatedAsyncioTestCase):
    async def test_malformed_context_rejects_before_spawn_and_returns_refs(self):
        activated = []

        async def callback(_context, _value):
            activated.append(True)

        for context in ([], {}, True, 0, -1, 1 << 64, None):
            with self.subTest(context=context):
                runtime = Runtime("test", "1", [])
                identifier = runtime.offer(Callback(callback, Context(runtime, 1), event=True))
                value = params(identifier, context=context)
                if context is None:
                    value.pop("context")
                reader = asyncio.StreamReader()
                reader.feed_data(notification("callback", value))
                with self.assertRaises(CordisError) as rejected:
                    await runtime.connection._read(reader)
                self.assertEqual(rejected.exception.code, 2)
                self.assertEqual(runtime.callback_usage, (0, 0))
                self.assertEqual(runtime.events, {})
                self.assertEqual(dict(runtime.imports), {})
                release = runtime.connection.control_queue.get_nowait()
                self.assertEqual(release[:2], ("release", {"objects": [101]}))
                await asyncio.sleep(0)
                self.assertEqual(activated, [])
                runtime.close()

    async def test_burst_limit_and_cancel_before_first_poll_return_all_refs(self):
        activated = []

        async def slow(_context, value):
            activated.append(value)
            await asyncio.Future()

        runtime = Runtime("test", "1", [], max_active_callbacks=2)
        callback = runtime.offer(Callback(slow, Context(runtime, 1), event=True))
        frames = [notification("callback", params(callback, reference=101 + i, value=i))
                  for i in range(3)]
        reader = asyncio.StreamReader()
        reader.feed_data(b"".join(frames))
        with self.assertRaises(CordisError) as rejected:
            await runtime.connection._read(reader)
        self.assertEqual(rejected.exception.code, 8)
        self.assertEqual(runtime.callback_usage, (2, sum(len(f) - 4 for f in frames[:2])))
        self.assertEqual(set(runtime.imports), {101, 102})
        tasks = list(runtime.events[1])
        runtime.close()
        await asyncio.gather(*tasks, return_exceptions=True)
        await asyncio.sleep(0)
        self.assertEqual(activated, [])
        self.assertEqual(runtime.callback_usage, (0, 0))
        self.assertEqual(dict(runtime.imports), {})
        self.assertEqual(runtime.events, {})
        released = [runtime.connection.control_queue.get_nowait()[1]["objects"][0]
                    for _ in range(runtime.connection.control_queue.qsize())]
        self.assertCountEqual(released, [101, 102, 103])

    async def test_release_context_before_first_poll_finishes_its_permit(self):
        activated = []

        async def event(_context, _value):
            activated.append(True)

        runtime = Runtime("test", "1", [])
        callback = runtime.offer(Callback(event, Context(runtime, 1), event=True))
        reader = asyncio.StreamReader()
        reader.feed_data(notification("callback", params(callback)) +
                         notification("release_context", {"context": 1}))
        reader.feed_eof()
        await runtime.connection._read(reader)
        self.assertEqual(runtime.callback_usage[0], 1)
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        self.assertEqual(runtime.callback_usage, (0, 0))
        self.assertEqual(activated, [])
        self.assertEqual(dict(runtime.imports), {})

    async def test_wire_byte_boundary_is_measured_by_the_reader(self):
        runtime = Runtime("test", "1", [])
        callback = runtime.offer(Callback(lambda *_: None, Context(runtime, 1), event=True))
        frame = notification("callback", params(callback, value=b"x" * 512))
        for maximum, accepted in ((len(frame) - 4, True), (len(frame) - 5, False)):
            with self.subTest(maximum=maximum):
                target = Runtime("test", "1", [], max_callback_bytes=maximum)
                target.offer(Callback(lambda *_: None, Context(target, 1), event=True))
                reader = asyncio.StreamReader()
                reader.feed_data(frame)
                reader.feed_eof()
                if accepted:
                    await target.connection._read(reader)
                    self.assertEqual(target.callback_usage, (1, len(frame) - 4))
                else:
                    with self.assertRaises(CordisError) as rejected:
                        await target.connection._read(reader)
                    self.assertEqual(rejected.exception.code, 8)
                    self.assertEqual(target.callback_usage, (0, 0))
                tasks = [task for tasks in target.events.values() for task in tasks]
                target.close()
                await asyncio.gather(*tasks, return_exceptions=True)
                await asyncio.sleep(0)
                self.assertEqual(target.callback_usage, (0, 0))

    async def test_cancel_resistant_callback_retains_permit_until_completion(self):
        started = asyncio.Event()
        cancelled = asyncio.Event()
        finish = asyncio.Event()
        number = (1 << 64) - 1

        async def stubborn(context, _value):
            self.assertEqual(context.number, number)
            started.set()
            try:
                await asyncio.Future()
            except asyncio.CancelledError:
                cancelled.set()
                await finish.wait()

        runtime = Runtime("test", "1", [], max_active_callbacks=1)
        callback = runtime.offer(Callback(stubborn, Context(runtime, 1), event=True))
        value = params(callback, context=number)
        size = len(notification("callback", value)) - 4
        await runtime.notification("callback", value, size)
        task = next(iter(runtime.events[number]))
        await asyncio.wait_for(started.wait(), 1)
        await runtime.notification("release_context", {"context": number})
        await asyncio.wait_for(cancelled.wait(), 1)
        self.assertEqual(runtime.callback_usage, (1, size))
        self.assertIn(101, runtime.imports)
        with self.assertRaises(CordisError) as rejected:
            await runtime.notification("callback", params(callback, context=number, reference=102), size)
        self.assertEqual(rejected.exception.code, 8)
        self.assertNotIn(102, runtime.imports)
        finish.set()
        await task
        await asyncio.sleep(0)
        self.assertEqual(runtime.callback_usage, (0, 0))
        self.assertEqual(dict(runtime.imports), {})
        await runtime.notification("callback", params(callback, context=number, reference=103), size)
        tasks = list(runtime.events[number])
        runtime.close()
        await asyncio.gather(*tasks, return_exceptions=True)
        await asyncio.sleep(0)
        self.assertEqual(runtime.callback_usage, (0, 0))

    async def test_reverse_response_and_context_release_progress_at_capacity(self):
        runtime = Runtime("test", "1", [], max_active_callbacks=2)
        writer = Writer()
        runtime.connection.writer = writer
        activated = []
        received = asyncio.Event()
        second = asyncio.Event()

        async def event(_context, value):
            activated.append(value)
            if value == 0:
                reply = await runtime.connection.request("probe", {})
                try:
                    self.assertEqual(reply.value, 91)
                    received.set()
                finally:
                    reply.close()
            else:
                second.set()
                await asyncio.Future()

        callback = runtime.offer(Callback(event, Context(runtime, 1), event=True))
        reader = asyncio.StreamReader()
        reading = asyncio.create_task(runtime.connection._read(reader))
        reader.feed_data(notification("callback", params(callback, value=0)) +
                         notification("callback", params(callback, context=2, reference=102, value=1)))
        await asyncio.wait_for(writer.written.wait(), 1)
        await asyncio.wait_for(second.wait(), 1)
        self.assertEqual(runtime.callback_usage[0], 2)
        request = writer.frames[0]["request"]
        body = cbor2.dumps({"response": {"id": request["id"], "result": {"ok": 91}}})
        reader.feed_data(len(body).to_bytes(4, "big") + body)
        await asyncio.wait_for(received.wait(), 1)
        await asyncio.sleep(0)
        reader.feed_data(notification("release_context", {"context": 2}))
        reader.feed_eof()
        await asyncio.wait_for(reading, 1)
        await asyncio.sleep(0)
        await asyncio.sleep(0)
        self.assertEqual(activated, [0, 1])
        self.assertEqual(runtime.callback_usage, (0, 0))
        self.assertEqual(runtime.connection.pending, {})
        self.assertEqual(dict(runtime.imports), {})
        runtime.close()

    def test_callback_limits_require_positive_integers(self):
        for limits in ({"max_active_callbacks": 0}, {"max_callback_bytes": 0},
                       {"max_active_callbacks": True}, {"max_callback_bytes": 1.5}):
            with self.subTest(limits=limits), self.assertRaises(CordisError):
                Runtime("test", "1", [], **limits)


if __name__ == "__main__":
    unittest.main()
