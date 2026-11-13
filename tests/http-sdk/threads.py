import json
import shutil

from measure import runtime, save


def run(options):
    project = options.evidence / "threads"
    (project / "plugins").mkdir(parents=True)
    (project / "tmp").mkdir()
    shutil.copy2(options.tools / "libhttp_test_native.dylib", project / "plugins/libhttp_test_native.dylib")
    configuration = '[runtime]\ncheckpoint_ms=0\n[instance.provider]\nplugin="libhttp_test_native.dylib"\ncomponent="thread-provider"\ngroup="provider"\n'
    for mode in ("plain", "tokio"):
        configuration += f'[instance.{mode}]\nplugin="libhttp_test_native.dylib"\ncomponent="thread-probe"\ngroup={json.dumps(mode)}\nconfig={{mode={json.dumps(mode)},result={json.dumps(str(project / (mode + ".json")))}}}\n'
    (project / "cordis.toml").write_text(configuration)
    with runtime(project, options) as current:
        def ready():
            snapshot = current.status()
            if snapshot and snapshot["instance_errors"]:
                raise RuntimeError(json.dumps(snapshot))
            return all((project / (mode + ".json")).exists() for mode in ("plain", "tokio"))
        current.wait(ready, "自有线程调用、远端取消与控制操作完成", seconds=30)
        result = {mode: json.loads((project / (mode + ".json")).read_text()) for mode in ("plain", "tokio")}
        save(project / "observed-status.json", current.status())
    return result
