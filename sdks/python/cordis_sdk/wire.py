# DOC-16 §2–6：标准流帧、请求截止时间与发送所有权；连接错误交回运行入口。
import asyncio
import struct
import sys
from contextvars import ContextVar

import cbor2

from .errors import CordisError
from .budget import encode, decode

_invoke_deadline = ContextVar("invoke_deadline", default=None)


class Connection:
    """DOC-16 §2–6、§10：双向标准流连接，所有请求共享有界发送通道。"""

    def __init__(self, runtime, max_frame=16777216, max_inflight=64, timeout=30,
                 max_depth=64, max_collection=65536, max_values=262144):
        self.runtime = runtime
        self.max_frame = max_frame
        self.max_inflight = max_inflight
        self.timeout = timeout
        self.max_depth = max_depth
        self.max_collection = max_collection
        self.max_values = max_values
        if not (all(type(value) is int for value in
                    (max_frame, max_depth, max_collection, max_values, max_inflight))
                and 64 <= max_frame < 1 << 32 and 4 <= max_depth <= 128
                and max_collection > 0 and max_values >= 4 and max_inflight > 0):
            raise CordisError(2, "Invalid UP/1 connection limits")
        self.peer_budget = False
        self.pending = {}
        self.running = {}
        self.notifications = set()
        self.control_queue = asyncio.Queue(maxsize=64)
        self.next_id = 1
        self.writer = None
        self.closed = False
        self.write_lock = asyncio.Lock()
        self.capacity = asyncio.Semaphore(max_inflight)

    async def send(self, message, offers=(), *, budget_deadline=None):
        """DOC-16 §6：发送成功交付出口引用，编码或写入失败撤回本次出口。"""
        committed = False
        try:
            data = encode(message, self)
            length = len(data)
            if length > self.max_frame:
                raise CordisError(8, "消息超过帧长度上限")
            async with self.write_lock:
                if self.closed:
                    raise CordisError(4, "连接已关闭")
                await self.writer.drain()
                budget = b""
                if budget_deadline is not None:
                    remaining = budget_deadline - asyncio.get_running_loop().time()
                    if remaining <= 0:
                        raise TimeoutError
                    budget = cbor2.dumps(max(1, int(remaining * 1000)))
                    # 定长映射以 budget_ms 的单字节 CBOR null 占位结尾。
                    data = memoryview(data)[:-1]
                    length += len(budget) - 1
                    if length > self.max_frame:
                        raise CordisError(8, "消息超过帧长度上限")
                self.writer.write(b"".join((struct.pack(">I", length), data, budget)))
                committed = True
        finally:
            if not committed:
                self.runtime.release_exports(offers)

    async def request(self, method, params, offers=()):
        """DOC-16 §10：截止时间覆盖背压等待，取消通过控制通知交回核心。"""
        request_id = None
        delivered = False
        try:
            loop = asyncio.get_running_loop()
            deadline = loop.time() + self.timeout
            parent = _invoke_deadline.get()
            if parent is not None:
                deadline = min(parent, deadline)
            async with asyncio.timeout_at(deadline):
                async with self.capacity:
                    request_id = self.next_id
                    self.next_id += 1
                    future = loop.create_future()
                    self.pending[request_id] = future
                    budget_deadline = None
                    if method == "invoke" and self.peer_budget:
                        params = dict(params)
                        params.pop("budget_ms", None)
                        params["budget_ms"] = None
                        budget_deadline = deadline
                    await self.send(
                        {
                            "request": {
                                "id": request_id,
                                "method": method,
                                "params": params,
                            }
                        },
                        budget_deadline=budget_deadline,
                    )
                    delivered = True
                    return await future
        except (TimeoutError, asyncio.CancelledError) as problem:
            if delivered:
                self.notify_soon("cancel", {"id": request_id})
            if isinstance(problem, TimeoutError):
                raise CordisError(6, "请求超过截止时间") from problem
            raise
        finally:
            if request_id is not None:
                self.pending.pop(request_id, None)
            if not delivered:
                self.runtime.release_exports(offers)

    async def notify(self, method, params, offers=()):
        await self.send({"notification": {"method": method, "params": params}}, offers)

    def notify_soon(self, method, params, offers=()):
        """DOC-16 §6：析构控制消息写入失败时关闭连接并回收资源表。"""
        if self.closed:
            self.runtime.release_exports(offers)
            return
        try:
            self.control_queue.put_nowait((method, params, offers))
        except asyncio.QueueFull:
            self.runtime.release_exports(offers)
            self.closed = True
            self.writer.close()

    async def _control_messages(self):
        while True:
            method, params, offers = await self.control_queue.get()
            await self.notify(method, params, offers)

    def _notification_done(self, task):
        self.notifications.discard(task)
        if not task.cancelled() and task.exception() is not None:
            self.closed = True
            self.writer.close()

    async def serve(self):
        """DOC-16 §2、§10：EOF 结束连接，回收本进程持有的请求与对象。"""
        loop = asyncio.get_running_loop()
        reader = asyncio.StreamReader(limit=self.max_frame + 4)
        transport, _ = await loop.connect_read_pipe(
            lambda: asyncio.StreamReaderProtocol(reader), sys.stdin.buffer
        )
        output, protocol = await loop.connect_write_pipe(
            asyncio.streams.FlowControlMixin, sys.stdout.buffer
        )
        self.writer = asyncio.StreamWriter(output, protocol, None, loop)
        sender = asyncio.create_task(self._control_messages())
        self.notifications.add(sender)
        sender.add_done_callback(self._notification_done)
        try:
            await self._read(reader)
        finally:
            self.closed = True
            transport.close()
            for future in self.pending.values():
                if not future.done():
                    future.set_exception(CordisError(4, "核心连接已关闭"))
            tasks = [entry[1] for entry in self.running.values()] + list(
                self.notifications
            )
            for task in tasks:
                task.cancel()
            await asyncio.gather(*tasks, return_exceptions=True)
            while not self.control_queue.empty():
                _, _, offers = self.control_queue.get_nowait()
                self.runtime.release_exports(offers)
            self.runtime.close()
            output.close()

    async def _read(self, reader):
        while not self.closed:
            try:
                header = await reader.readexactly(4)
            except asyncio.IncompleteReadError as problem:
                if problem.partial:
                    raise CordisError(2, "帧头被截断") from problem
                return
            length = struct.unpack(">I", header)[0]
            if length > self.max_frame:
                raise CordisError(8, "输入帧超过长度上限")
            message = decode(await reader.readexactly(length), self)
            if not isinstance(message, dict) or len(message) != 1:
                raise CordisError(2, "消息包络或尾随数据错误")
            kind, body = next(iter(message.items()))
            if kind == "response":
                self._response(body)
            elif kind == "request":
                self._request(body)
            elif kind == "notification":
                await self.runtime.notification(body["method"], body["params"])
            else:
                raise CordisError(3, "消息类别错误")

    def _response(self, body):
        future = self.pending.get(body["id"])
        result = body["result"]
        if "error" in result:
            if future is not None and not future.done():
                future.set_exception(CordisError(**result["error"]))
        else:
            payload = self.runtime.payload(result["ok"])
            if future is None or future.done():
                payload.close()
            else:
                future.set_result(payload)

    def _request(self, body):
        request_id = body["id"]
        if request_id in self.running or request_id <= 0:
            raise CordisError(2, "请求编号重复或无效")
        if len(self.running) >= self.max_inflight:
            raise CordisError(8, "入站请求额度耗尽")
        payload = self.runtime.payload(body["params"])
        task = asyncio.create_task(self._execute(body, payload, asyncio.get_running_loop().time()))
        self.running[request_id] = (body["method"], task)
        task.add_done_callback(lambda finished: self._request_done(request_id, payload, finished))

    def _request_done(self, request_id, payload, task):
        payload.close()
        self.running.pop(request_id, None)
        if task.cancelled() and not self.closed:
            response = {"id": request_id, "result": {"error": CordisError(7, "调用已取消").wire()}}
            sender = asyncio.create_task(self.send({"response": response}))
            self.notifications.add(sender)
            sender.add_done_callback(self._notification_done)
        elif not task.cancelled() and task.exception() is not None:
            self.closed = True
            self.writer.close()

    async def _execute(self, body, payload, received):
        offers = []
        method = body["method"]
        try:
            deadline = None
            if method == "invoke" and "budget_ms" in payload.value:
                budget = payload.value["budget_ms"]
                if type(budget) is not int or not 0 <= budget < 1 << 64:
                    raise CordisError(2, "调用预算需要 u64 毫秒值")
                self.peer_budget = True
                deadline = received + min(self.timeout, budget / 1000)
            token = _invoke_deadline.set(deadline)
            try:
                async with asyncio.timeout_at(deadline) as timeout:
                    if deadline is not None and deadline <= asyncio.get_running_loop().time():
                        raise CordisError(6, "服务调用超过截止时间")
                    value, offers = await self.runtime.request(method, payload)
            except TimeoutError as problem:
                if timeout.expired():
                    raise CordisError(6, "服务调用超过截止时间") from problem
                raise
            finally:
                _invoke_deadline.reset(token)
            if method == "hello":
                value = {**value, "invoke_budget_v1": True}
            result = {"ok": value}
        except CordisError as problem:
            result = {"error": problem.wire()}
        except (ValueError, TypeError, KeyError) as problem:
            result = {"error": CordisError(2, str(problem)).wire()}
        except Exception as problem:
            result = {"error": CordisError(1, str(problem)).wire()}
        try:
            await self.send({"response": {"id": body["id"], "result": result}}, offers)
        except CordisError as problem:
            if problem.code != 8:
                raise
            await self.send({"response": {"id": body["id"], "result": {"error": problem.wire()}}})
