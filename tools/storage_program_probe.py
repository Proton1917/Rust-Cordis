#!/usr/bin/env python3
"""Execute unchanged storage publication/recovery bodies against real directories.

Every recorded std I/O call in six publication scenarios is individually failed
and interrupted. Recovery is itself interrupted at every recorded call and then
restarted. Lean generates the successful semantic I/O order from the exact write
and backup plans used by each execution. Hardware power loss is not simulated.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / 'formal'
sys.path.insert(0, str(FORMAL))
from check_kernel_refinement import braced as checked_braced
SUBJECT = ROOT / 'crates/cordis-runtime/src/storage.rs'
HARNESS = FORMAL / 'storage_program_probe.rs'
INPUTS = [SUBJECT, HARNESS, Path(__file__).resolve()] + [FORMAL / name for name in
    ['StorageProgram.lean', 'StorageControlFlow.lean', 'Persistence.lean', 'DirectoryInitialization.lean',
     'StorageVectors.lean', 'check_kernel_refinement.py', 'lakefile.toml', 'lake-manifest.json', 'lean-toolchain']]
FUNCTIONS = ['pub fn io_error(', 'pub fn read(path:', 'fn read_file(', 'pub fn write(path:',
             'fn publish(', 'fn sync_directory(', 'fn sync_directory_contents(',
             'fn prepare_directory_tree(', 'fn remove_file(', 'fn check_journal_format(',
             'fn deployment_lock_file(']
TYPES = ['pub(crate) enum CommitError', 'impl From<WireError> for CommitError',
         'pub(crate) struct Committed', "struct PreparedDeployment<'a>"]
METHODS = ['fn restore_deployment_files(', 'fn clear_staging(', 'fn move_decision_marker(',
           'fn lock_deployment(', 'pub(crate) fn commit_deployment<',
           "fn prepare_deployment<'a>(", 'fn commit_prepared(', 'fn reject_deployment(', 'fn sync_decision(']


def execute(command, *, cwd=ROOT, input=None, return_codes=(0,)):
    result = subprocess.run(command, cwd=cwd, input=input, text=True, capture_output=True, timeout=180)
    if result.returncode not in return_codes:
        raise RuntimeError(f'{command}:\n{result.stdout}\n{result.stderr}')
    return result.stdout


def extract(source, marker):
    return checked_braced(source, marker)


def sources():
    return {str(p.relative_to(ROOT)): hashlib.sha256(p.read_bytes()).hexdigest() for p in INPUTS}


def receipt(path, data):
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile('w', dir=path.parent, delete=False) as f:
        temporary = Path(f.name)
        json.dump(data, f, indent=2)
        f.write('\n')
        f.flush()
        os.fsync(f.fileno())
    os.replace(temporary, path)


def program():
    source = SUBJECT.read_text()
    blocks = {marker: extract(source, marker) for marker in [*FUNCTIONS, *TYPES, *METHODS]}
    constants = []
    for name in ['BASE', 'JOURNAL_FORMAT', 'JOURNAL_VERSION']:
        matches = [line for line in source.splitlines() if line.startswith(f'const {name}:')]
        if len(matches) != 1:
            raise ValueError(f'constant is not unique: {name}')
        constants.append(matches[0])
    types = [('#[derive(Debug)]\n' if marker in [TYPES[0], TYPES[2]] else '') + blocks[marker] for marker in TYPES]
    result = HARNESS.read_text().replace('/* PRODUCTION_FUNCTIONS */', '\n'.join([*constants, *types, *[blocks[m] for m in FUNCTIONS]]))
    result = result.replace('/* PRODUCTION_METHODS */', '\n'.join(blocks[m] for m in METHODS))
    return result, [{'marker': m, 'sha256': hashlib.sha256(b.encode()).hexdigest()} for m, b in blocks.items()]


def normalize_rust(fragment):
    """Token normalization for the explicitly supported method-body grammar.

    Quoted strings remain indivisible. Every non-whitespace/comment token is
    retained; an unknown expression, loop, branch or error policy is rejected.
    This is a fail-closed subset extractor, not a general Rust parser.
    """
    pattern = r'"(?:\\.|[^"\\])*"|//[^\n]*|/\*[\s\S]*?\*/|\s+|[^\s]'
    return ''.join(token for token in re.findall(pattern, fragment)
                   if not token.isspace() and not token.startswith(('//', '/*')))


def method_body(source, marker):
    block = extract(source, marker)
    return normalize_rust(block[block.index('{') + 1:-1])


def consume(fragment, expected, subject):
    expected = normalize_rust(expected)
    if not fragment.startswith(expected):
        raise ValueError(f'unsupported {subject} source structure near {fragment[:160]}')
    return fragment[len(expected):]


def ordered_nodes(fragment, patterns, subject):
    result = []
    normalized = [(normalize_rust(text), node) for text, node in patterns]
    while fragment:
        matches = [(text, node) for text, node in normalized if fragment.startswith(text)]
        if len(matches) != 1:
            raise ValueError(f'unsupported or ambiguous {subject} effect near {fragment[:160]}')
        text, node = matches[0]
        result.append(node)
        fragment = fragment[len(text):]
    return result


def source_template(source):
    """Extract the ordered effect/loop template and check all supported bindings."""
    if method_body(source, 'pub fn write(path:') != normalize_rust('publish(path, bytes, true)'):
        raise ValueError('write must select durable publication')
    entry = ordered_nodes(method_body(source, 'pub(crate) fn commit_deployment<'), [
        ('let prepared = PreparedDeployment::encode(base, overlay, states, sources)?;', 'encode'),
        ('let _lock = self.lock_deployment()?;', 'lock'),
        ('''if let Err(problem) = self.prepare_deployment(prepared.states.keys()) {
            return Err(self.reject_deployment(problem));
        }''', 'prepareKeys'),
        ('self.commit_prepared(&prepared)', 'commitSame'),
    ], 'commit_deployment same-prepared entry')
    helpers = {
        'fn lock_deployment(': '''
            let lock = deployment_lock_file(&self.directory).map_err(io_error)?;
            lock.lock().map_err(io_error)?;
            Ok(lock)
        ''',
        'fn deployment_lock_file(': '''
            #[cfg(unix)]
            {
                File::open(directory.join("consumed"))
            }
            #[cfg(windows)]
            {
                let consumed = directory.join("consumed");
                if !consumed.is_dir() {
                    return Err(std::io::Error::from(std::io::ErrorKind::NotFound));
                }
                fs::OpenOptions::new()
                    .read(true)
                    .write(true)
                    .create(true)
                    .truncate(false)
                    .open(consumed.join(".deployment.lock"))
            }
        ''',
        'fn remove_file(': '''match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(problem) if problem.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(problem) => Err(io_error(problem)),
        }''',
        'fn sync_directory(': '''
            #[cfg(test)] sync_test::before(path)?;
            sync_directory_contents(path)
        ''',
        'fn sync_directory_contents(': '''
            #[cfg(unix)]
            {
                File::open(path)
                    .and_then(|directory| directory.sync_all())
                    .map_err(io_error)
            }
            #[cfg(windows)]
            {
                let metadata = fs::metadata(path).map_err(io_error)?;
                if !metadata.is_dir() {
                    return Err(io_error(std::io::Error::new(
                        std::io::ErrorKind::NotADirectory,
                        format!("路径不是目录：{}", path.display()),
                    )));
                }
                Ok(())
            }
        ''',
    }
    for marker, expected in helpers.items():
        if method_body(source, marker) != normalize_rust(expected):
            raise ValueError(f'unsupported primitive source body: {marker}')
    publication = consume(method_body(source, 'fn publish('),
                          'let staged = path.with_added_extension("next");', 'publish path binding')
    publication = ordered_nodes(publication, [
        ('let mut file = File::create(&staged).map_err(io_error)?;', 'create'),
        ('file.write_all(bytes).map_err(io_error)?;', 'write'),
        ('if durable { file.sync_all().map_err(io_error)?; } drop(file);', 'sync'),
        ('fs::rename(staged, path).map_err(io_error)', 'rename'),
    ], 'publish')
    prepare = consume(method_body(source, "fn prepare_deployment<'a>("), '''
        self.restore_deployment_files()?;
        self.clear_staging()?;
        let staging = self.directory.join(".deployment-staging");
        fs::create_dir(&staging).map_err(io_error)?;
        write(&staging.join(JOURNAL_FORMAT), JOURNAL_VERSION)?;
        let preserve = |relative: &Path| -> Result<()> {
            let saved = staging.join(relative);
            match fs::hard_link(self.directory.join(relative), &saved) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    File::create(saved.with_added_extension("absent")).map_err(io_error)?;
                    Ok(())
                }
                Err(error) => Err(io_error(error)),
            }
        };
    ''', 'journal setup and preserve closure')
    prepare_nodes = ordered_nodes(prepare, [
        ('preserve(Path::new("instances.toml"))?;', ('backup', 'overlay')),
        ('preserve(Path::new("sources.json"))?;', ('backup', 'sources')),
        ('preserve(Path::new(BASE))?;', ('backup', 'base')),
        ('''for name in names {
            config::instance_name(name)?;
            preserve(Path::new(&format!("{name}.state")))?;
        }''', ('backup', 'states')),
        ('sync_directory(&staging)?;', ('barrier', 'sync')),
        ('fs::rename(&staging, self.directory.join(".deployment-undo")).map_err(io_error)?;', ('barrier', 'rename')),
        ('sync_directory(&self.directory)', ('barrier', 'confirm')),
    ], 'prepare_deployment')
    expected_backups = [('backup', name) for name in ['overlay', 'sources', 'base', 'states']]
    if prepare_nodes[:4] != expected_backups or any(kind != 'barrier' for kind, _ in prepare_nodes[4:]):
        raise ValueError('journal backup loop/field order or barrier separation changed')
    journal = ['backups'] + [node for _, node in prepare_nodes[4:]]
    commit = consume(method_body(source, 'fn commit_prepared('),
                     'self.checkpoints.lock().clear(); let result = (|| {', 'commit closure')
    suffix = normalize_rust('''
        })();
        if let Err(problem) = result { return Err(self.reject_deployment(problem)); }
        self.sync_decision()
    ''')
    if not commit.endswith(suffix):
        raise ValueError('commit must handle publication error before calling sync_decision')
    commit = commit[:-len(suffix)]
    commit_nodes = ordered_nodes(commit, [
        ('write(&self.directory.join(BASE), prepared.base.as_bytes())?;', ('group', 'base')),
        ('''for (name, bytes) in &prepared.states {
            let path = self.directory.join(format!("{name}.state"));
            match bytes {
                Some(bytes) => write(&path, bytes)?,
                None => remove_file(&path)?,
            }
        }''', ('group', 'states')),
        ('write(&self.directory.join("instances.toml"), prepared.overlay.as_bytes(),)?;', ('group', 'overlay')),
        ('write(&self.directory.join("sources.json"), prepared.sources)?;', ('group', 'sources')),
        ('sync_directory(&self.directory)?;', ('finish', 'syncData')),
        ('self.move_decision_marker()', ('finish', 'moveDecision')),
    ], 'commit_prepared')
    groups, finish = [], []
    for kind, node in commit_nodes:
        if kind == 'group':
            if finish:
                raise ValueError('file publication followed a transaction decision operation')
            groups.append(node)
        else:
            finish.append(node)
    expected_sync = '''
        sync_directory(&self.directory).map_err(CommitError::RecoveryRequired)?;
        Ok(Committed { cleanup_error: self.clear_staging().err(), })
    '''
    if method_body(source, 'fn sync_decision(') != normalize_rust(expected_sync):
        raise ValueError('decision acknowledgement/cleanup error policy changed')
    finish.append('syncDecision')
    expected_move = '''
        fs::rename(self.directory.join(".deployment-undo"),
                   self.directory.join(".deployment-staging"),).map_err(io_error)
    '''
    if method_body(source, 'fn move_decision_marker(') != normalize_rust(expected_move):
        raise ValueError('decision marker source/destination or error propagation changed')
    recovery = consume(method_body(source, 'fn restore_deployment_files('), '''
        self.checkpoints.lock().clear();
        let journal = self.directory.join(".deployment-undo");
        if journal.try_exists().map_err(io_error)? {
            check_journal_format(&journal)?;
            for folder in ["", "consumed"] {
                let destination = self.directory.join(folder);
                let entries = match fs::read_dir(journal.join(folder)) {
                    Ok(entries) => entries,
                    Err(e) if folder == "consumed" && e.kind() == std::io::ErrorKind::NotFound => { continue; }
                    Err(e) => return Err(io_error(e)),
                };
                for file in entries {
                    let file = file.map_err(io_error)?;
                    if file.file_type().map_err(io_error)?.is_dir() { continue; }
                    let name = file.file_name().into_string().map_err(|_| parameter("部署记录文件名无效"))?;
                    if folder.is_empty() && name == JOURNAL_FORMAT { continue; }
                    if let Some(absent) = name.strip_suffix(".absent") {
    ''', 'recovery journal/folder/name scan')
    separator = normalize_rust('} else {')
    if recovery.count(separator) != 1:
        raise ValueError('recovery must have exactly the absent/present branch')
    absent, present = recovery.split(separator)
    restore_absent = ordered_nodes(absent, [
        ('remove_file(&destination.join(absent))?;', 'removeTarget')
    ], 'absent journal entry')
    present = consume(present, '''
        let target = destination.join(name);
        let link = target.with_added_extension("recovering");
    ''', 'recovery original-byte and temporary-path binding')
    suffix = normalize_rust('''
                    }
                }
                sync_directory(&destination)?;
            }
            self.move_decision_marker()?;
            sync_directory(&self.directory)?;
        }
        Ok(())
    ''')
    if not present.endswith(suffix):
        raise ValueError('recovery folder/decision confirmation or error propagation changed')
    restore_present = ordered_nodes(present[:-len(suffix)], [
        ('remove_file(&link)?;', 'removeTemporary'),
        ('fs::hard_link(file.path(), &link).map_err(io_error)?;', 'linkJournal'),
        ('fs::rename(&link, target).map_err(io_error)?;', 'publishTemporary'),
    ], 'present journal entry')
    return {'publication': publication, 'journal': journal, 'groups': groups, 'finish': finish,
            'entry': entry, 'restorePresent': restore_present, 'restoreAbsent': restore_absent}


def compile_control_flow_certificate(lake, directory, source, name):
    template = source_template(source)
    fields = '\n'.join(f'  {field} := [{", ".join("." + value for value in values)}]'
                       for field, values in template.items())
    certificate = f'''import StorageControlFlow
open Cordis.StorageControlFlow Cordis.StorageProgram Cordis.Persistence
def extracted : Template := {{
{fields} }}
theorem extracted_is_canonical : extracted = canonical := by decide
theorem extracted_covers_all_write_and_backup_lists (before : Image)
    (writes : List Change) (order : List Nat) :
    ∃ t, Execution (initialWithBackupOrder before writes order)
      (compile extracted order writes) t ∧ t.cursor = .committed :=
  source_certificate_covers_arbitrary_loops extracted extracted_is_canonical before writes order
theorem entry_constructs_backup_coverage (p : Prepared) (name : Nat)
    (modified : name ∈ (entryRun {{prepared := p}} extracted.entry).writes.map Prod.fst) :
    name ∈ (entryRun {{prepared := p}} extracted.entry).backups :=
  source_entry_derives_backup_coverage extracted extracted_is_canonical p name modified
theorem recovery_restores_arbitrary_names (before image : Image)
    (journal : Nat → Option (Option Nat)) (names : List Nat)
    (complete : ∀ name ∈ names, journal name = some (before name)) :
    sourceReplay extracted journal names image = some (restore before names image) :=
  certified_recovery_restores_every_original_name extracted extracted_is_canonical before image journal names complete
theorem entry_and_recovery_compose (before : Image) (p : Prepared)
    (events : List Cordis.StorageProgram.Event) (markerPersisted : Bool) :
    let entry := entryRun {{prepared := p}} extracted.entry
    let machine := run (initialWithBackupOrder before entry.writes entry.backups) events
    sourceRecovery extracted (entry.writes.map Prod.fst) machine markerPersisted = some before ∨
      sourceRecovery extracted (entry.writes.map Prod.fst) machine markerPersisted =
        some (applyWrites before entry.writes) :=
  certified_entry_transaction_recovers_old_or_new extracted extracted_is_canonical before p events markerPersisted
'''
    path = directory / f'{name}-certificate.lean'
    path.write_text(certificate)
    execute([lake, 'env', 'lean', str(path)], cwd=FORMAL)
    return {'template': template, 'lean_source': certificate,
            'sha256': hashlib.sha256(certificate.encode()).hexdigest(),
            'status': 'kernel_checked',
            'scope': 'Fail-closed entry/publication/recovery grammar; same Prepared supplies all backup keys and writes; arbitrary valid journal names use the checked restore loop. Serialization, filesystem iteration-to-name representation and primitive I/O remain explicit extraction/OS contracts.'}


def parse(output):
    result, semantics, calls = None, [], []
    for line in output.splitlines():
        if line.startswith('RESULT\t'):
            if result is not None:
                raise ValueError('duplicate case receipt')
            _, mode, status, count, fired, plan = line.split('\t')
            result = {'mode': mode, 'status': status, 'calls': int(count), 'injected': bool(int(fired)), 'plan': plan}
        elif line.startswith('SEM\t'):
            _, index, event = line.split('\t')
            semantics.append((int(index), event))
        elif line.startswith('CALL\t'):
            _, index, phase, op, first, second = line.split('\t')
            calls.append({'index': int(index), 'phase': phase, 'op': op, 'first': first, 'second': second})
        else:
            raise ValueError(f'unrecognized probe line: {line}')
    if result is None or len(calls) != result['calls'] or [x['index'] for x in calls] != list(range(1, len(calls) + 1)):
        raise ValueError('incomplete or nonconsecutive source execution evidence')
    result['semantic_trace'] = semantics
    result['raw_calls'] = calls
    return result


def build(directory, rustc, source, name):
    path, binary = directory / f'{name}.rs', directory / name
    path.write_text(source)
    execute([rustc, '--edition=2024', '-Awarnings', str(path), '-o', str(binary)])
    return binary


def run_case(binary, directory, scenario, mode, fault):
    case = directory / f'{binary.name}-{scenario}-{mode}-{fault}'
    stopped = fault != 0 and mode in {'crash', 'recover-crash'}
    output = execute([str(binary), str(case), str(scenario), mode, str(fault)],
                     return_codes=(86,) if stopped else (0,))
    result = parse(output)
    if stopped:
        resume = 'resume-recovery' if mode == 'recover-crash' else 'resume'
        resumed = execute([str(binary), str(case), str(scenario), resume, '0'])
        if resumed != f'RESUMED\t{resume}\n':
            raise ValueError('fresh process recovery failed to produce its completion receipt')
        result['fresh_process_recovery'] = True
    if fault and not result['injected']:
        raise ValueError(f'injection boundary was not reached: {scenario} {mode} {fault}')
    if fault and mode in {'crash', 'recover-crash'} and result['status'] != 'crashed':
        raise ValueError('process interruption was ignored')
    if fault and mode == 'error' and result['status'].startswith('committed'):
        operation = result['raw_calls'][fault - 1]
        expected_cleanup = operation['phase'] == 'commit' and operation['op'] == 'remove-dir' and operation['first'] == '.deployment-staging'
        if not expected_cleanup or result['status'] != 'committed-cleanup-error':
            raise ValueError(f'I/O error was ignored before acknowledgement: {operation}')
    if fault and mode == 'recover' and result['status'] != 'error':
        raise ValueError('recovery I/O error was ignored')
    result.update(scenario=scenario, fault=fault)
    return result


def run_initialization(binary, directory, fault):
    case = directory / f'{binary.name}-initialization-{fault}' / 'nested' / 'store'
    output = execute([str(binary), str(case), '0', 'initialize', str(fault)])
    result = None
    paths = {'required': [], 'first': [], 'retry': []}
    for line in output.splitlines():
        parts = line.split('\t')
        if parts[0] == 'INITIALIZATION':
            if result is not None or len(parts) != 4:
                raise ValueError('duplicate or malformed initialization result')
            result = {'fault': int(parts[1]), 'ready': parts[2] == '1', 'required_count': int(parts[3])}
        elif parts[0] in {'INITREQUIRED', 'INITFIRST', 'INITRETRY'} and len(parts) == 2:
            paths[parts[0][4:].lower()].append(parts[1])
        else:
            raise ValueError(f'invalid initialization output: {line}')
    if result is None or result['fault'] != fault or len(paths['required']) != result['required_count']:
        raise ValueError('incomplete initialization evidence')
    if len(set(paths['required'])) != len(paths['required']):
        raise ValueError('initialization required paths are not unique')
    identities = {path: index for index, path in enumerate(paths['required'])}
    result['first'] = [identities[path] for path in paths['first']]
    result['retry'] = [identities[path] for path in paths['retry']]
    return result


def compare_initialization(lake, cases):
    count = cases[0]['required_count']
    if len(cases) != count + 1 or {case['fault'] for case in cases} != set(range(count + 1)):
        raise ValueError('initialization failure domain is incomplete')
    output = execute([lake, 'env', 'lean', '--run', 'StorageVectors.lean', f'init:{count}'], cwd=FORMAL)
    models = {}
    for line in output.splitlines():
        if line == f'INITPLAN\t{count}' or line == 'INITEND':
            continue
        parts = line.split('\t')
        if len(parts) != 7 or parts[0] != 'INITCASE':
            raise ValueError(f'invalid directory initialization model vector: {line}')
        fault = int(parts[1])
        if fault in models:
            raise ValueError('duplicate initialization model vector')
        models[fault] = {
            'first': [int(x) for x in parts[2].split(',') if x],
            'confirmed': int(parts[3]), 'ready': parts[4] == 'true',
            'retry': [int(x) for x in parts[5].split(',') if x], 'retry_ready': parts[6] == 'true',
        }
    if set(models) != set(range(count + 1)):
        raise ValueError('initialization model domain is incomplete')
    for case in cases:
        model = models[case['fault']]
        if case['required_count'] != count or any(case[key] != model[key] for key in ['first', 'retry', 'ready']) or not model['retry_ready']:
            raise ValueError('source directory confirmation/retry differs from Lean')
        if model['confirmed'] != len(case['first']) - int(case['fault'] != 0):
            raise ValueError('failed directory barrier was marked confirmed')
    return models


def lean_orders(lake, plans):
    output = execute([lake, 'env', 'lean', '--run', 'StorageVectors.lean', *sorted(plans)], cwd=FORMAL)
    orders, current = {}, None
    for line in output.splitlines():
        if line.startswith('PLAN\t'):
            current = line.split('\t', 1)[1]
            if current in orders:
                raise ValueError('duplicate Lean plan')
            orders[current] = []
        elif line.startswith('IO\t') and current:
            orders[current].append(line.split('\t', 1)[1])
        elif line == 'END':
            current = None
        else:
            raise ValueError(f'invalid Lean storage vector: {line}')
    if current is not None or set(orders) != set(plans) or any(not events for events in orders.values()):
        raise ValueError('incomplete Lean plan domain')
    return orders


def compare(result, orders):
    if result['mode'] in {'recover', 'recover-crash'}:
        return
    expected = orders[result['plan']]
    fault = result['fault']
    observed = [event for index, event in result['semantic_trace'] if not fault or index < fault]
    if observed != expected[:len(observed)]:
        raise ValueError(f'storage source I/O order differs from Lean at {result["scenario"]}/{result["mode"]}/{fault}:\n{observed}\n{expected}')
    if not fault and observed != expected:
        raise ValueError('successful storage invocation omitted a model I/O step')


def check(lake, rustc):
    before = sources()
    execute([lake, 'env', 'lean', '-o', '.lake/build/lib/lean/Persistence.olean', 'Persistence.lean'], cwd=FORMAL)
    execute([lake, 'env', 'lean', '-o', '.lake/build/lib/lean/StorageProgram.olean', 'StorageProgram.lean'], cwd=FORMAL)
    execute([lake, 'env', 'lean', '-o', '.lake/build/lib/lean/DirectoryInitialization.olean', 'DirectoryInitialization.lean'], cwd=FORMAL)
    execute([lake, 'env', 'lean', '-o', '.lake/build/lib/lean/StorageControlFlow.olean', 'StorageControlFlow.lean'], cwd=FORMAL)
    source, extracts = program()
    production = SUBJECT.read_text()
    cases = []
    negatives = {}
    with tempfile.TemporaryDirectory(prefix='cordis-storage-program-') as name:
        directory = Path(name)
        control_flow_certificate = compile_control_flow_certificate(lake, directory, production, 'production')
        binary = build(directory, rustc, source, 'production')
        initialization = [run_initialization(binary, directory, 0)]
        initialization.extend(run_initialization(binary, directory, fault)
                              for fault in range(1, initialization[0]['required_count'] + 1))
        initialization_models = compare_initialization(lake, initialization)
        for scenario in range(6):
            baseline = run_case(binary, directory, scenario, 'normal', 0)
            cases.append(baseline)
            for fault in range(1, baseline['calls'] + 1):
                for mode in ['error', 'crash']:
                    cases.append(run_case(binary, directory, scenario, mode, fault))
            recovery = run_case(binary, directory, scenario, 'recover', 0)
            cases.append(recovery)
            for fault in range(1, recovery['calls'] + 1):
                for mode in ['recover', 'recover-crash']:
                    cases.append(run_case(binary, directory, scenario, mode, fault))
        orders = lean_orders(lake, {case['plan'] for case in cases})
        for case in cases:
            compare(case, orders)
        changes = {
            'omitted_file_sync': ('file.sync_all().map_err(io_error)?;', 'let _ = &file;'),
            'decision_before_data_sync': ('sync_directory(&self.directory)?;\n            self.move_decision_marker()', 'self.move_decision_marker()?;\n            sync_directory(&self.directory)'),
            'ignored_write_error': ('file.write_all(bytes).map_err(io_error)?;', 'let _ = file.write_all(bytes);'),
            'entry_omits_state_backup_keys': ('self.prepare_deployment(prepared.states.keys())', 'self.prepare_deployment(std::iter::empty())'),
            'recovery_skips_legal_state_name': ('if let Some(absent) = name.strip_suffix(".absent") {', 'if name == "z.state" { continue; }\n                    if let Some(absent) = name.strip_suffix(".absent") {'),
        }
        for label, (old, new) in changes.items():
            if source.count(old) != 1 or production.count(old) != 1:
                raise ValueError(f'mutation target is not unique: {label}')
            mutant = build(directory, rustc, source.replace(old, new), label)
            certificate_rejected = None
            try:
                compile_control_flow_certificate(lake, directory, production.replace(old, new), label)
            except (RuntimeError, ValueError) as error:
                certificate_rejected = str(error)
            if certificate_rejected is None:
                raise RuntimeError(f'control-flow certificate accepted negative control: {label}')
            rejected = None
            try:
                scenario = 5 if label in {'entry_omits_state_backup_keys', 'recovery_skips_legal_state_name'} else 1
                if label == 'recovery_skips_legal_state_name':
                    run_case(mutant, directory, scenario, 'recover', 0)
                baseline = run_case(mutant, directory, scenario, 'normal', 0)
                model = lean_orders(lake, {baseline['plan']})
                compare(baseline, model)
                for fault in range(1, baseline['calls'] + 1):
                    case = run_case(mutant, directory, scenario, 'error', fault)
                    compare(case, model)
            except (RuntimeError, ValueError) as error:
                rejected = str(error)
            if rejected is None:
                raise RuntimeError(f'negative control survived: {label}')
            negatives[label] = {'status': 'rejected', 'reason': rejected[:1200],
                                'control_flow_certificate': {'status': 'rejected', 'reason': certificate_rejected[:1200]}}
        initialization_changes = {
            'initialization_skips_child': ('sync(&directory.join("consumed"))?;', ''),
            'initialization_ignores_barrier_error': ('sync(&directory.join("consumed"))?;', 'let _ = sync(&directory.join("consumed"));'),
        }
        for label, (old, new) in initialization_changes.items():
            if source.count(old) != 1:
                raise ValueError(f'mutation target is not unique: {label}')
            mutant = build(directory, rustc, source.replace(old, new), label)
            rejected = None
            try:
                run_initialization(mutant, directory, 0)
                run_initialization(mutant, directory, 1)
            except (RuntimeError, ValueError) as error:
                rejected = str(error)
            if rejected is None:
                raise RuntimeError(f'negative control survived: {label}')
            negatives[label] = {'status': 'rejected', 'reason': rejected[:1200]}
    after = sources()
    if before != after:
        raise RuntimeError('storage source/proof inputs changed during execution')
    compact = [{k: v for k, v in case.items() if k not in {'semantic_trace', 'raw_calls'}} for case in cases]
    return {'status': 'passed', 'cases': len(cases) + len(initialization), 'scenarios': 6,
            'compiled_input_cases': len(cases) + len(initialization),
            'storage_execution_cases': len(cases), 'directory_initialization_cases': len(initialization),
            'success_runs': sum(case['fault'] == 0 and case['mode'] == 'normal' for case in cases),
            'io_failure_runs': sum(case['mode'] == 'error' for case in cases),
            'process_interruption_runs': sum(case['mode'] == 'crash' for case in cases),
            'recovery_runs': sum(case['mode'] == 'recover' for case in cases),
            'recovery_interruption_runs': sum(case['mode'] == 'recover-crash' for case in cases),
            'fresh_process_recovery_runs': sum(case.get('fresh_process_recovery', False) for case in cases),
            'source_sha256': before, 'source_sha256_after': after,
            'source_extracts': extracts, 'negative_controls': negatives,
            'model_orders': orders, 'executions': compact,
            'initialization_cases': initialization, 'initialization_models': initialization_models,
            'control_flow_certificate': control_flow_certificate,
            'adapters': ['WireError retains message text', 'CheckpointCache clear is inert',
                         'PreparedDeployment.encode maps prevalidated opaque byte fixtures; one returned Prepared is consumed by the actual entry body for keys and writes',
                         'commit_deployment and lock_deployment bodies are unchanged; File.lock executes the real exclusive standard-library filesystem lock',
                         'instance-name validator accepts only the fixed valid fixture alphabet',
                         'real std::fs operations, paths, files and directory entries',
                         'directory entry iteration is sorted to make fault boundaries reproducible',
                         'File and filesystem wrappers inject before one recorded operation',
                         'Path::try_exists and metadata queries use the real filesystem without injected failure'],
            'proof_connection': 'The source entry derives backup coverage from the same Prepared as publication. Each preserve/publication/sync operation projects to StorageProgram.nextIo; complete recovery structure and its file primitive lists refine journal replay for arbitrary valid names. Journal format-file creation, exclusive locking and staging cleanup are preparatory/cleanup stutters.',
            'failure_scope': 'I/O errors and process exit without unwinding before each selected I/O, followed by recovery in a fresh process over the retained real directory; persistent errors may postpone recovery. Physical power loss, filesystem sync implementation and storage-device durability remain explicit OS contracts.'}


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--lake', default='lake')
    parser.add_argument('--rustc', default='rustc')
    parser.add_argument('--evidence', type=Path)
    args = parser.parse_args()
    receipt(args.evidence, {'status': 'running'})
    try:
        result = check(shutil.which(args.lake) or args.lake, shutil.which(args.rustc) or args.rustc)
    except BaseException as error:
        receipt(args.evidence, {'status': 'failed', 'error': str(error)})
        raise
    receipt(args.evidence, result)
    print(json.dumps({key: value for key, value in result.items() if key not in {'executions', 'model_orders'}}, indent=2))


if __name__ == '__main__':
    main()
