import json
from pathlib import Path
import ssl
import sys

sys.path.insert(0, str(Path(sys.argv[1]).resolve()))
import cbor2
import _cbor2

value = {"bytes": bytes([0, 127, 255]), "number": 42}
assert cbor2.loads(cbor2.dumps(value)) == value
print(json.dumps({"version": sys.version, "executable": sys.executable, "prefix": sys.prefix, "extension": _cbor2.__file__, "openssl": ssl.OPENSSL_VERSION}))
