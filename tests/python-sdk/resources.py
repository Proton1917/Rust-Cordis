import asyncio
import json
from pathlib import Path
import signal
import subprocess
import sys
import time

from support import EVIDENCE, manifest, test_environment


def plugin():
    from cordis_sdk import Component, CordisError, run
    import resources_binding as binding

    class Item:
        def __init__(self, value, evidence):
            self.value = value
            self.evidence = evidence

        def read(self):
            return self.value

        def relay(self, value):
            return value

        async def wait(self):
            try:
                await asyncio.sleep(20)
                return self.value
            finally:
                Path(self.evidence, "cancelled.txt").write_text("cancelled\n")

    class Provider:
        def __init__(self, evidence):
            self.evidence = evidence

        def make(self, value):
            return Item(value, self.evidence)

        async def borrow_value(self, value):
            return await value.read()

        def mirror(self, value):
            return value

    async def provide(context, config):
        await context.provide("resources@1.0", Provider(config["evidence"]), binding)

        async def body(scope):
            async def cleanup(_context):
                Path(config["evidence"], "child-cleanup.txt").write_text("cleaned\n")

            await scope.defer(cleanup)

        await context.effect(body)

    async def consume(context, config):
        service = await context.get("resources@1.0", binding, binding.ResourcesClient)
        item = await service.make("owned")
        if config.get("transfer"):
            mirrored = await service.mirror(item)
            returned = await mirrored.relay(item)
            assert await returned.read() == "owned"
            received = []
            event_ready = asyncio.Event()

            async def listener(event_context, payload):
                received.append(
                    event_context.import_resource(
                        payload["item"],
                        "resources.item@1.0",
                        binding,
                        binding.ItemClient,
                    )
                )
                event_ready.set()

            await context.on("resources.notice@1.0", listener, binding=binding)
            await context.emit(
                "resources.notice@1.0",
                {
                    "item": context.service(
                        "resources.item@1.0", Item("event", config["evidence"]), binding
                    )
                },
            )
            await asyncio.wait_for(event_ready.wait(), 5)
            assert await received[0].read() == "event"
            received[0]._transport.close()
            Path(config["evidence"], "observed.json").write_text(
                json.dumps(
                    {
                        "resource_argument_and_result": True,
                        "event_resource_retained_after_callback": True,
                    }
                )
            )
            return
        assert await item.read() == "owned"
        assert await service.borrow_value(item) == "owned"
        mirrored = await service.mirror(item)
        assert await mirrored.read() == "owned"
        task = asyncio.create_task(item.wait())
        await asyncio.sleep(0.1)
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass
        assert await item.read() == "owned"
        try:
            await service.make("x" * context.runtime.connection.max_frame)
        except CordisError as problem:
            assert problem.code == 8
        else:
            raise AssertionError("超长请求需要返回 code 8")
        assert await mirrored.read() == "owned"
        item._transport.close()
        assert await mirrored.read() == "owned"
        mirrored._transport.close()
        Path(config["evidence"], "observed.json").write_text(
            json.dumps(
                {
                    "resource": True,
                    "borrow": True,
                    "mirror": True,
                    "cancel": True,
                    "oversize": True,
                    "child": True,
                }
            )
        )

    async def diagnose(context, config):
        try:
            await consume(context, config)
        except Exception as problem:
            Path(config["evidence"], "failure.json").write_text(
                json.dumps({"type": type(problem).__name__, "message": str(problem)})
            )
            raise

    async def parent(context, config):
        isolated = await context.isolate("resources@1.0")
        await isolated.plugin("child", config)
        await isolated.plugin("consumer", config)

    run(
        "python-resources",
        [
            Component("child", provide, provides=("resources@1.0",)),
            Component(
                "consumer",
                diagnose,
                inject={"resources@1.0": True},
                listens=("resources.notice@1.0",),
            ),
            Component("parent", parent),
        ],
    )


def execute():
    project = EVIDENCE / (
        "resources" + ("-" + sys.argv[1] if len(sys.argv) > 1 else "")
    )
    project.mkdir()
    manifest(
        project / "plugins/resource",
        "python-resources",
        [
            "conda",
            "run",
            "--no-capture-output",
            "-n",
            "base",
            "python",
            str(Path(__file__).resolve()),
            "plugin",
        ],
    )
    transfer = len(sys.argv) > 1 and sys.argv[1].startswith("transfer")
    (project / "cordis.toml").write_text(
        '[runtime]\ncheckpoint_ms=0\n[instance.parent]\nplugin="python-resources"\ncomponent="parent"\nconfig={evidence='
        + json.dumps(str(project))
        + ",transfer="
        + str(transfer).lower()
        + "}\n"
    )
    environment = test_environment()
    with (
        (project / "stdout.log").open("w") as output,
        (project / "stderr.log").open("w") as error,
    ):
        process = subprocess.Popen(
            [str(EVIDENCE / "bin/cordis"), "run"],
            cwd=project,
            env=environment,
            stdout=output,
            stderr=error,
        )
        try:
            deadline = time.monotonic() + 20
            while not (project / "observed.json").exists():
                if (project / "failure.json").exists():
                    raise RuntimeError((project / "failure.json").read_text())
                if process.poll() is not None or time.monotonic() >= deadline:
                    raise RuntimeError("资源场景结果缺失")
                time.sleep(0.1)
            process.send_signal(signal.SIGTERM)
            assert process.wait(timeout=15) == 0
            assert (project / "child-cleanup.txt").read_text() == "cleaned\n"
            if not transfer:
                assert (project / "cancelled.txt").read_text() == "cancelled\n"
            print((project / "observed.json").read_text())
        finally:
            if process.poll() is None:
                process.send_signal(signal.SIGTERM)
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "plugin":
        plugin()
    else:
        execute()
