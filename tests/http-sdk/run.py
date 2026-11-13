import argparse
import http.client
import json
from pathlib import Path
import shutil
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "benchmarks/multilanguage"))
from measure import fixture, runtime, save


class Case:
    def __init__(self, options):
        self.options = options
        self.project = options.evidence
        self.prefixes = {"rust": "/api", "python": "/python", "typescript": "/typescript"}
        self.current = None
        self.address = None
        self.required = {"console", "http", "rust", "python", "typescript", "admin"}

    def prepare(self):
        plugins = self.project / "plugins"
        plugins.mkdir(parents=True)
        (self.project / "tmp").mkdir()
        for name in ("libcordis_http.dylib", "libhttp_echo.dylib", "libconsole_log.dylib"):
            shutil.copy2(self.options.tools / name, plugins / name)
        configuration = '[runtime]\ncheckpoint_ms=50\nconfirm_ms=20\napply_timeout_ms=120000\n'
        configuration += '[instance.console]\nplugin="libconsole_log.dylib"\ncomponent="console"\nconfig={level=3}\n'
        configuration += '[instance.http]\nplugin="libcordis_http.dylib"\ncomponent="http"\nforwarder=true\nconfig={max_body_bytes=104857600,route_timeout_ms=120000,public_origins=["https://app.example.test"]}\n'
        for language, prefix in self.prefixes.items():
            reference = "libhttp_echo.dylib" if language == "rust" else self.process_plugin(plugins, language)
            configuration += f'[instance.{language}]\nplugin={json.dumps(reference)}\ncomponent="http-echo"\ngroup={json.dumps(language)}\nconfig={{prefix={json.dumps(prefix)}}}\n'
        configuration += fixture(self.project, self.options.tools, "admin")
        (self.project / "cordis.toml").write_text(configuration)

    def process_plugin(self, plugins, language):
        directory = plugins / language
        directory.mkdir()
        source = self.options.workspace / "plugins" / ("http-echo-py" if language == "python" else "http-echo-ts")
        command = [sys.executable, "-B", str(source / "main.py")] if language == "python" else ["node", str(source / "dist/main.js")]
        name = "http-echo-py" if language == "python" else "http-echo-ts"
        (directory / "cordis-plugin.toml").write_text(f'name={json.dumps(name)}\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand={json.dumps(command)}\n')
        return language

    def ready(self):
        snapshot = self.current.status()
        if snapshot is None:
            return None
        if snapshot["instance_errors"] or snapshot["plugin_errors"]:
            raise RuntimeError(json.dumps(snapshot))
        states = {item["name"]: item["state"] for item in snapshot["instances"]}
        return snapshot if all(states.get(name) == "active" for name in self.required) else None

    def start(self, current):
        self.current = current
        self.current.wait(self.ready, "HTTP 与三种语言处理方激活")
        address = self.project / "state/data/http/listeners/http.json"
        self.current.wait(address.exists, "HTTP 地址已公布")
        self.address = json.loads(address.read_text())["bound"]
        self.current.wait(lambda: (self.project / "admin-commands").is_dir(), "管理客户端激活")

    def connection(self):
        host, port = self.address.rsplit(":", 1)
        return http.client.HTTPConnection(host, int(port), timeout=120)

    def request(self, method, path, body=None, headers=None, chunked=False):
        connection = self.connection()
        try:
            connection.request(method, path, body, headers or {}, encode_chunked=chunked)
            response = connection.getresponse()
            return response.status, list(response.getheaders()), response.read()
        finally:
            connection.close()

    def manage(self, method, arguments):
        return self.current.request("admin", "cordis.manage@1.1", method, arguments)[0]


def routes(case):
    observations = {}
    for language, prefix in case.prefixes.items():
        values = []
        for method in ("GET", "POST", "PUT", "DELETE"):
            path = prefix + "/check%20path?name=a%2Fb&n=3"
            data = bytes([0, 127, 255, 65])
            headers = {"Cookie": "session=route-cookie", "X-Test": "route-header", "Authorization": "Bearer route-token", "X-Forwarded-For": "203.0.113.7", "Connection": "X-Hop", "X-Hop": "discard"}
            status, received, body = case.request(method, path, data, headers)
            assert status == 200, (language, status, body)
            value = json.loads(body)
            assert (value["method"], value["path"], value["prefix"], value["query"], value["body"]) == (method, path.split('?')[0], prefix, "name=a%2Fb&n=3", list(data)), value
            fields = dict(value["headers"])
            for name in ("cookie", "x-test", "authorization", "x-forwarded-for"):
                assert fields[name] == next(value for key, value in headers.items() if key.lower() == name)
            assert "x-hop" not in fields and "connection" not in fields
            assert [value for key, value in received if key.lower() == "set-cookie"] == ["session=one; HttpOnly; Secure", "locale=zh; Secure"]
            assert "cordis" not in dict((key.lower(), value.lower()) for key, value in received).get("server", "")
            values.append({"method": method, "status": status, "echo": value, "response_headers": received})
        status, headers, _ = case.request("PATCH", prefix + "/check")
        assert status == 405 and dict(headers)["allow"] == "GET, POST, PUT, DELETE", (status, headers)
        inline = bytes(range(256)) * 4096
        status, _, body = case.request("POST", prefix + "/inline", inline)
        assert status == 200, (language, status, body[:200])
        assert bytes(json.loads(body)["body"]) == inline
        observations[language] = {"methods": values, "inline_bytes": len(inline), "inline_equal": True}
    assert case.request("GET", "/apix")[0] == 404
    for suffix, expected in [("error", 500), ("panic", 500), ("invalid-status", 502)]:
        status, headers, body = case.request("GET", "/api/" + suffix)
        assert status == expected and b"cordis" not in body.lower(), (status, body)
    return observations


def main():
    parser = argparse.ArgumentParser()
    for name in ("workspace", "tools", "dependencies", "evidence"):
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--scenario", required=True)
    options = parser.parse_args()
    for name in ("workspace", "tools", "dependencies", "evidence"):
        setattr(options, name, getattr(options, name).resolve())
    if options.scenario in ("bytes", "threads", "secrets", "web", "scaffold", "streams"):
        if options.scenario == "bytes":
            from aggregate import run
        elif options.scenario == "threads":
            from threads import run
        elif options.scenario == "secrets":
            from secrets import run
        elif options.scenario == "web":
            from web_cases import run
        elif options.scenario == "scaffold":
            from scaffold import run
        else:
            from stream_cases import run
        options.evidence.mkdir(parents=True, exist_ok=True)
        result = run(options)
        save(options.evidence / "result.json", {"scenario": options.scenario, "result": result, "exit": 0})
        return
    case = Case(options)
    case.prepare()
    with runtime(case.project, options) as current:
        case.start(current)
        if options.scenario == "routes":
            result = routes(case)
        elif options.scenario == "files":
            from files import run
            result = run(case)
        elif options.scenario == "lifecycle":
            from lifecycle import run
            result = run(case)
        elif options.scenario == "latency":
            from latency import run
            result = run(case)
        else:
            raise ValueError(options.scenario)
        save(case.project / "observed-status.json", current.status())
    save(case.project / "result.json", {"scenario": options.scenario, "result": result, "exit": 0})


if __name__ == "__main__":
    main()
