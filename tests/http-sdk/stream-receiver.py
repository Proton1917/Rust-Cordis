import json
from pathlib import Path

from cordis_sdk import Component, run
from cordis_sdk.streams import to_file
import stream_binding as binding


async def apply(context, config):
    files = await context.get("test.stream-files@1.0", binding, binding.FilesClient)
    source = await files.open()
    count = await to_file(source, config["output"])
    Path(config["result"]).write_text(json.dumps({"bytes": count, "chunk_limit": 1048576, "frame_limit": 4198400}))


run("stream-receiver-py", [Component("stream-probe", apply, inject={"test.stream-files@1.0": True})], max_frame=4198400)
