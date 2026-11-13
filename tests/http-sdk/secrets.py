import json
from pathlib import Path
import plistlib
import shutil
import subprocess
import tarfile

from measure import runtime, save


VALUES = {"ALPHA": "m22-alpha-$literal-value", "BETA": "m22-beta-private-value"}


def prepare(options, label, mode):
    project = options.evidence / label
    (project / "plugins").mkdir(parents=True)
    (project / "tmp").mkdir()
    (project / "public").mkdir()
    secret = project / "public/credentials.env"
    secret.write_text("# local deployment\n" + "\n".join(f"{name}={value}" for name, value in VALUES.items()) + "\n")
    secret.chmod(mode)
    for name in ("libcordis_secrets.dylib", "libhttp_test_native.dylib", "libconsole_log.dylib"):
        shutil.copy2(options.tools / name, project / "plugins" / name)
    configuration = '[pack]\nname="secret-boundary"\nassets=["public"]\n[runtime]\ncheckpoint_ms=50\n[instance.console]\nplugin="libconsole_log.dylib"\ncomponent="console"\nconfig={level=3}\n'
    configuration += '[instance.secrets]\nplugin="libcordis_secrets.dylib"\ncomponent="secrets"\nconfig={file="public/credentials.env",grants={reader-a=["ALPHA"],reader-b=["BETA"]}}\n'
    for reader in ("reader-a", "reader-b", "unlisted"):
        child = f',child_result={json.dumps(str(project / "child.json"))}' if reader == "reader-a" else ""
        configuration += f'[instance.{reader}]\nplugin="libhttp_test_native.dylib"\ncomponent="secrets-probe"\ngroup={json.dumps(reader)}\nconfig={{names=["ALPHA","BETA"],result={json.dumps(str(project / (reader + ".json")))}{child}}}\n'
    (project / "cordis.toml").write_text(configuration)
    return project


def private_outputs(project):
    paths = [project / "stdout.log", project / "stderr.log", project / "state/status.json"]
    paths.extend((project / "state").rglob("*.state"))
    for path in paths:
        if path.exists():
            data = path.read_bytes()
            for value in VALUES.values():
                assert value.encode() not in data, path
    return [str(path.relative_to(project)) for path in paths if path.exists()]


def permissions(options):
    project = prepare(options, "permissions-0644", 0o644)
    with runtime(project, options) as current:
        def rejected():
            snapshot = current.status()
            if not snapshot:
                return None
            instance = next((entry for entry in snapshot["instances"] if entry["name"] == "secrets"), None)
            return snapshot if instance and instance["state"] == "failed" else None
        snapshot = current.wait(rejected, "0644 密钥文件激活失败", seconds=30)
        save(project / "rejected.json", snapshot)
        assert "0644" in json.dumps(snapshot), snapshot
    return {"mode": "0644", "rejected": True, "scanned": private_outputs(project)}


def grants(options):
    project = prepare(options, "grants-0600", 0o600)
    with runtime(project, options) as current:
        current.wait(lambda: all((project / (name + ".json")).exists() for name in ("reader-a", "reader-b", "unlisted", "child")), "授权、拒绝与子组件结果", seconds=30)
        reports = {name: json.loads((project / (name + ".json")).read_text()) for name in ("reader-a", "reader-b", "unlisted", "child")}
        for name, allowed in (("reader-a", "ALPHA"), ("reader-b", "BETA"), ("child", "ALPHA")):
            assert reports[name]["results"][allowed]["value"] == VALUES[allowed], reports[name]
            denied = "BETA" if allowed == "ALPHA" else "ALPHA"
            assert reports[name]["results"][denied]["code"] == 9
        assert all(value["code"] == 9 for value in reports["unlisted"]["results"].values())
        assert all(report["forged_identity_code"] == 9 for report in reports.values())
        save(project / "observed-status.json", current.status())
    scanned = private_outputs(project)
    return project, {"instances": reports, "scanned": scanned}


def install_environment(options, project):
    common = [str(options.tools / "cordis"), "service", "install", "--config", str(project / "cordis.toml"), "--name", "org.test.http-secrets"]
    denied = project / "denied.plist"
    rejected = subprocess.run([*common, "--output", str(denied), "--env", "PATH=/usr/bin:/bin", "--env", "API_TOKEN=private-env-value"], capture_output=True, text=True)
    assert rejected.returncode != 0 and not denied.exists()
    assert "private-env-value" not in rejected.stdout + rejected.stderr
    output = project / "environment.plist"
    accepted = subprocess.run([*common, "--output", str(output), "--env", "PATH=/usr/bin:/bin", "--inherit-env", "LANG"], capture_output=True, text=True, env={**__import__("os").environ, "LANG": "en_US.UTF-8"})
    assert accepted.returncode == 0, accepted.stderr
    data = plistlib.loads(output.read_bytes())
    assert data["EnvironmentVariables"] == {"LANG": "en_US.UTF-8", "PATH": "/usr/bin:/bin"}
    save(project / "service-results.json", {"denied_exit": rejected.returncode, "denied_error": rejected.stderr, "environment": data["EnvironmentVariables"]})
    return data["EnvironmentVariables"]


def package(options, project):
    command = [str(options.tools / "cordis-dev"), "pack", "--project", str(project), "--target", "aarch64-apple-darwin", "--out", "dist", "--runtime-dir", str(options.tools)]
    result = subprocess.run(command, capture_output=True, text=True)
    (project / "pack.log").write_text(result.stdout + result.stderr)
    assert result.returncode == 0, result.stderr
    archive = next((project / "dist").glob("*.tar.gz"))
    names = []
    with tarfile.open(archive, "r:gz") as package:
        for entry in package:
            names.append(entry.name)
            assert not entry.name.endswith("credentials.env"), entry.name
            if entry.isfile():
                stream = package.extractfile(entry)
                previous = b""
                while chunk := stream.read(1024 * 1024):
                    data = previous + chunk
                    assert all(value.encode() not in data for value in VALUES.values()), entry.name
                    previous = data[-64:]
    return {"archive": archive.name, "members": names, "secret_file_absent": True, "secret_values_absent": True}


def run(options):
    rejected = permissions(options)
    project, accepted = grants(options)
    return {"permissions": rejected, "grants": accepted, "environment": install_environment(options, project), "pack": package(options, project)}
