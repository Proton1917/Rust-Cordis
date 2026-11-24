"""Verify pure Web source confirmation with a real core and SDK registry process.

Build cordis, install the Python SDK dependencies, then run with CORDIS_BINARY
pointing at the built core. The registry fixture implements the HTTP asset
registration interface; asset identity is checked without launching a browser.
"""
from pathlib import Path
import json, os, signal, subprocess, sys, tempfile, time

REPO = Path(__file__).resolve().parents[2]
BINARY = Path(os.environ.get("CORDIS_BINARY", str(REPO / "target/debug/cordis")))
CASE_DIRECTORY = tempfile.TemporaryDirectory(prefix="cordis-web-source-")
CASE = Path(CASE_DIRECTORY.name)
OUT = CASE / "observations"
OUT.mkdir()
(CASE/'plugins/webmock').mkdir(parents=True)
(CASE/'plugins/panel/web').mkdir(parents=True)
mock='''from cordis_sdk import Component, run
import types
class Registry:
    def __init__(self): self.next = 0
    def register(self, fields):
        self.next += 1
        return self.next
    def activate(self, value): return None
    def unregister(self, value): return None
async def dispatch(provider, transport, interface, method, values):
    return getattr(provider, method)(*values)
binding=types.SimpleNamespace(SCHEMA={"services":[{"interface":"cordis.web@1.1","methods":[{"name":name} for name in ("register","activate","unregister")]}]},dispatch=dispatch)
async def apply(context, config):
    await context.provide("cordis.web@1.1",Registry(),binding)
run("webmock", [Component("registry",apply,provides=("cordis.web@1.1",))])
'''
(CASE/'plugins/webmock/main.py').write_text(mock)
(CASE/'plugins/webmock/cordis-plugin.toml').write_text('name="webmock"\nversion="0.1.0"\nsocket="process"\nprotocol=1\ncommand=['+json.dumps(sys.executable)+', "-B", "main.py"]\n')
(CASE/'plugins/panel/cordis-plugin.toml').write_text('name="panel"\nversion="0.1.0"\nsocket="web"\n[web]\nassets="web"\nmount="/panel"\nentry="index.js"\n')
(CASE/'plugins/panel/web/index.js').write_text('version_one\n')
(CASE/'cordis.toml').write_text('''format_version=2
[runtime]
checkpoint_ms=0
confirm_ms=0
lifecycle_timeout_ms=3000
invoke_timeout_ms=3000
[instance.registry]
plugin="webmock"
[instance.panel]
plugin="panel"
''')
env=os.environ.copy()
env['PYTHONPATH'] = os.pathsep.join([str(REPO/'sdks/python'), env.get('PYTHONPATH', '')])
results={'fixture':str(CASE),'scope':'Real core, real Python SDK process, real Web Deployment owner, fake HTTP registry service; no browser page involved','rounds':[], 'python':sys.version, 'platform':sys.platform}

def query(*args):
    r=subprocess.run([str(BINARY),'status','--config',str(CASE/'cordis.toml'),*args],capture_output=True,text=True,env=env,timeout=3)
    if r.returncode: raise RuntimeError(r.stderr)
    return json.loads(r.stdout)

def wait_active(p):
    end=time.monotonic()+12
    seen=None
    while time.monotonic()<end:
        if p.poll() is not None: raise RuntimeError('core exited '+str(p.returncode))
        try:
            seen=query()
            byname={i['name']:i for i in seen['instances']}
            if byname.get('panel',{}).get('state')=='active' and byname.get('registry',{}).get('state')=='active': return seen
        except (RuntimeError,KeyError,json.JSONDecodeError): pass
        time.sleep(.05)
    raise RuntimeError('activation timeout: '+repr(seen))

def round_(name):
    with (OUT/(name+'.stderr.log')).open('w') as log:
        p=subprocess.Popen([str(BINARY),'run','--config',str(CASE/'cordis.toml')],cwd=CASE,env=env,stdout=log,stderr=log)
        try:
            status=wait_active(p)
            end = time.monotonic() + 8
            while True:
                offline = query('--offline')
                if 'panel' in offline['instances']:
                    break
                if time.monotonic() >= end:
                    raise AssertionError('active Web owner was not confirmed: ' + repr(offline))
                time.sleep(.02)
            panel=next(i for i in status['instances'] if i['name']=='panel')
            ref=panel['reference']
            content=(CASE/'plugins'/ref/'web/index.js').read_text()
            row={'round':name,'panel_state':panel['state'],'panel_reference':ref,'panel_content':content.strip(),'status_names':[i['name'] for i in status['instances']],'offline_sources':offline}
            (OUT/(name+'.status.json')).write_text(json.dumps(status,indent=2))
            (OUT/(name+'.offline.json')).write_text(json.dumps(offline,indent=2))
            results['rounds'].append(row)
        finally:
            if p.poll() is None:
                p.send_signal(signal.SIGTERM)
                try:p.wait(timeout=8)
                except subprocess.TimeoutExpired:p.kill();p.wait()
            results['last_exit']=p.returncode
            if p.returncode:raise RuntimeError('core shutdown '+str(p.returncode))
try:
    round_('first')
    (CASE/'plugins/panel/web/index.js').write_text('version_two\n')
    round_('after_source_overwrite')
    first,second=results['rounds']
    assert first['panel_state']==second['panel_state']=='active'
    assert first['offline_sources']['instances']['panel']['installed_reference'] == first['panel_reference']
    assert first['panel_reference'] == second['panel_reference']
    assert first['panel_content'] == second['panel_content'] == 'version_one'
    results['passed'] = True
    (OUT/'result.json').write_text(json.dumps(results,indent=2))
    print(json.dumps({'passed': True, 'rounds': results['rounds'], 'last_exit': results['last_exit']}, indent=2))
except Exception as e:
    results['error']=repr(e)
    (OUT/'result.json').write_text(json.dumps(results,indent=2))
    raise

finally:
    CASE_DIRECTORY.cleanup()
