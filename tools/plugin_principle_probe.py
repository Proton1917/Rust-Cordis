#!/usr/bin/env python3
"""Bind the plugin/context principle to supported Rust construction boundaries.

The constructor and outbound apply projection produce arbitrary-input Lean
certificates. Complete production methods additionally compile and run through
explicit observation adapters. Closed body recognizers reject unreviewed control
flow in framework assembly. Real kernel executions live in plugin_principle.rs;
none of these finite executions is reported as a universal Rust proof.
"""
from __future__ import annotations

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parents[1]
FORMAL = ROOT / "formal"
sys.path.insert(0, str(FORMAL))
from check_kernel_refinement import braced, unique

SOURCES = {
    "NEW": ("crates/cordis/src/registry.rs", "pub fn new<R, F, Fut>("),
    "VALIDATED": ("crates/cordis/src/registry.rs", "pub fn validated<R, C, V, F, Fut>("),
    "ALLOCATE": ("crates/cordis/src/identity.rs", "pub(crate) fn allocate("),
    "REALM_NEW": ("crates/cordis/src/context.rs", "pub fn new() -> Self"),
    "INJECT": ("crates/cordis/src/registry.rs", "pub fn inject<F, Fut>("),
    "MOUNT": ("crates/cordis/src/registry.rs", "pub fn mount_with<R:"),
    "REMOTE_DEFINITION": ("crates/cordis-supervisor/src/manager/generation.rs", "pub(super) fn definition("),
    "APPLY_GENERATION": ("crates/cordis-supervisor/src/manager/generation.rs", "async fn apply_generation("),
    "MANAGER_NEW": ("crates/cordis-supervisor/src/manager.rs", "pub async fn new(parent:"),
    "MANAGER_INITIALIZE": ("crates/cordis-supervisor/src/manager.rs", "fn initialize("),
    "WEB_MOUNT": ("crates/cordis-web-deployment/src/lib.rs", "pub fn mount(self:"),
    "ROOT_FINISH": ("crates/cordis-runtime/src/runtime/owner.rs", "pub(super) async fn finish("),
    "ROOT_DROP": ("crates/cordis-runtime/src/runtime/owner.rs", "fn drop(&mut self)"),
}
COMPILED = ("NEW", "VALIDATED", "ALLOCATE", "APPLY_GENERATION")
CASE_COUNTS = {"constructor": 192, "remote": 512, "identity": 15}


def compact(text: str) -> str:
    """Remove comments/spacing while preserving complete ordinary string tokens."""
    result, i = [], 0
    while i < len(text):
        if text[i].isspace():
            i += 1
        elif text.startswith("//", i):
            newline = text.find("\n", i)
            i = len(text) if newline < 0 else newline + 1
        elif text.startswith("/*", i):
            i, depth = i + 2, 1
            while i < len(text) and depth:
                if text.startswith("/*", i):
                    depth, i = depth + 1, i + 2
                elif text.startswith("*/", i):
                    depth, i = depth - 1, i + 2
                else:
                    i += 1
            if depth:
                raise RuntimeError("unclosed source comment")
        elif text.startswith(('r"', 'r#', 'br"', 'br#'), i):
            raise RuntimeError("raw source strings need a reviewed recognizer")
        elif text[i] == '"':
            start, i = i, i + 1
            while i < len(text) and text[i] != '"':
                i += 2 if text[i] == "\\" else 1
            if i == len(text):
                raise RuntimeError("unclosed source string")
            i += 1
            result.append(text[start:i])
        else:
            result.append(text[i])
            i += 1
    return "".join(result)


def body(fragment: str) -> str:
    return compact(fragment[fragment.index("{") + 1:fragment.rindex("}")])


# These are complete supported bodies, including error handling, ownership
# registration and callback placement. They are structural certificates with
# an explicit extraction boundary, not proofs of their arbitrary Rust helpers.
CLOSED = {
    "VALIDATED": '''
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self {
            id: ComponentId::Closure(crate::identity::allocate(&NEXT).expect("component identity exhausted"),),
            name: name.into(), inject,
            resolve: Arc::new(move |raw| {
                let raw = raw.downcast_ref::<R>().ok_or_else(|| Error::new("invalid config type"))?;
                resolve(raw).map(|value| Arc::new(value) as RawConfig).map_err(Error::from)
            }),
            apply: Arc::new(move |ctx, raw| {
                let config = raw.downcast_ref::<C>().expect("resolved configuration type").clone();
                Box::pin(apply(ctx, config))
            }),
        }
    ''',
    "REALM_NEW": '''
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(crate::identity::allocate(&NEXT).expect("realm identity exhausted"))
    ''',
    "INJECT": '''
        self.mount_with(&ComponentDef::new("", inject, move |ctx, ()| apply(ctx)), (),)
    ''',
    "MOUNT": '''
        let location = Location::caller();
        let id = {
            let mut state = self.kernel.state.lock();
            if !self.permits_effect(&state) { return Err(Error::inactive().at(location)); }
            let target = forward(&state, component.id);
            let component = if target == component.id { component.clone() } else { state.definitions[&target].clone() };
            let id = state.fibers.insert(FiberData::child(self.scope.clone(), component.clone(), Arc::new(config), location,));
            let scope = Arc::new(Scope {
                parent: Some(self.scope.clone()), fiber: id, layer: Layer::Fiber,
                lease: Some(Arc::downgrade(&self.kernel)), has_epoch_guard: false,
            });
            state.fibers[id].canonical_scope = Arc::downgrade(&scope);
            state.fibers[id].scope = Some(scope);
            crate::reflect::insert_consumer(&mut state, id);
            state.runtimes.entry(component.id).or_insert_with(|| RuntimeData {
                component: component.clone(), fibers: Members::default(),
            }).fibers.push(id);
            id
        };
        let fiber = Fiber::handle(&self.kernel, id, &self.kernel.state.lock());
        #[cfg(test)] {
            let before = self.kernel.state.lock().before_mount_effect.take();
            if let Some(before) = before { before(); }
        }
        let child = fiber.clone();
        let effect = self.effect("ctx.use()", move |fx| {
            child.kernel.state.lock().fibers[child.id].lifetime = Some(fx.effect_id());
            fx.defer_async(move || {
                child.begin_dispose(); async move { child.wait_completion().await }
            })
        });
        let effect = match effect {
            Ok(effect) => effect,
            Err(error) => { fiber.begin_dispose(); return Err(error); }
        };
        drop(effect);
        fiber.emit_plugin();
        crate::reflect::refresh(&self.kernel, id);
        Ok(fiber)
    ''',
    "REMOTE_DEFINITION": '''
        let mut inject = Inject::new();
        for (interface, required) in &component.inject {
            let minor = interface.minor;
            inject = inject.with_named_options(
                interface.key.clone(), *required,
                Some(Arc::new(move |value| {
                    value.downcast_ref::<Binding>().is_some_and(|binding| binding.interface.minor >= minor)
                })),
            );
        }
        let name = metadata::clipped(&format!("{}/{}/{}", component.group, component.plugin, component.info.name));
        ComponentDef::new(name, inject, move |context, configuration: Configuration| {
            let core = core.clone(); let component = component.clone();
            async move {
                let manager = Manager(core.upgrade().ok_or_else(|| cordis::Error::new("核心生命周期已结束"))?,);
                let generation = manager.create_generation(context, component, configuration)?;
                manager.apply_generation(&generation).await.map_err(|problem| {
                    let retained = manager.keep_error(&generation, problem.clone());
                    *generation.instance.error.lock() = Some(retained); kernel(problem)
                })
            }
        },)
    ''',
    "MANAGER_NEW": '''
        if options.operation_queue_capacity == 0 { return Err(error(2, "管理操作队列容量需要为正整数")); }
        let (sender, receiver) = mpsc::channel(options.operation_queue_capacity);
        let available = Arc::new(Mutex::new(None)); let ready = available.clone();
        let definition = ComponentDef::new("cordis-supervisor", Inject::new(), move |context, ()| {
            *ready.lock() = Some(context); async { Ok(()) }
        });
        let fiber = parent.mount_with(&definition, ()).map_err(kernel_error)?;
        fiber.settled().await.map_err(kernel_error)?;
        let context = available.lock().take().expect("supervisor context initialized");
        Self::initialize(context, fiber, options, sender, receiver)
    ''',
    "MANAGER_INITIALIZE": '''
        let (stop, _) = watch::channel(false); let (finished, _) = watch::channel(None);
        let (pages, page_commands) = mpsc::channel(options.operation_queue_capacity);
        let manager = Self(Arc::new(Core {
            context: context.clone(), fiber,
            routes: cordis_routing::Router::new(options.max_exports, options.invoke_timeout),
            metadata: cordis_routing::metadata::MetadataBudget::new(options.max_retained_metadata_bytes,),
            options, next: AtomicU64::new(1), catalog: Mutex::new(Catalog::default()),
            contexts: Mutex::new(HashMap::new()), realms: Mutex::new(HashMap::new()),
            scopes: Mutex::new(HashMap::new()), effects: Mutex::new(Effects { records: HashMap::new(), next: HashMap::new(), }),
            stopping: AtomicBool::new(false), exported: AtomicBool::new(false),
            shutdown_export: Mutex::new(None), abandoned_snapshots: Mutex::new(Vec::new()),
            snapshot_returned: Notify::new(), commands: Mutex::new(CommandQueue { accepting: true, sender, pages, }),
            page_commands: tokio::sync::Mutex::new(page_commands), recovery_batch: Mutex::new(None),
            stop, finished, log_sink: Mutex::new(None), handler_extension: Mutex::new(None),
        }));
        let retained = manager.clone();
        context.effect("supervisor-shutdown", move |scope| {
            scope.defer_async(move || async move { retained.shutdown_contents().await.map_err(kernel) })
        }).map_err(kernel_error)?;
        manager.start_worker(receiver)?;
        Ok(manager)
    ''',
    "WEB_MOUNT": '''
        let name = spec.instance.clone(); let mut parent_view = spec.view.clone();
        let mut inject = Inject::new().with_named("cordis.web@1");
        if let Some(parent) = &spec.parent {
            parent_view = self.manager.instance_lifecycle_view(&parent_view, parent)?;
            inject = inject.with::<InstanceLifecycle>();
        }
        let current = Arc::new(Mutex::new(spec)); let revision = Arc::new(AtomicU64::new(1));
        let ready = watch::channel(None).0;
        let (configuration, generation, signal) = (current.clone(), revision.clone(), ready.clone());
        let weak = Arc::downgrade(self);
        let definition = ComponentDef::new(format!("web-owner-{name}"), inject, move |context, _revision: u64| {
            let (weak, current, revision, ready) = (weak.clone(), configuration.clone(), generation.clone(), signal.clone(),);
            async move {
                let Some(deployment) = weak.upgrade() else { return Err(cordis::Error::new("网页部署已退出")); };
                let spec = current.lock().clone(); let expected = revision.load(Ordering::Acquire);
                let result = deployment.activate(context, spec).await;
                ready.send_replace(Some(result.as_ref().map(|_| expected).map_err(Clone::clone),));
                result.map_err(kernel)
            }
        },);
        let fiber = parent_view.mount_with(&definition, 1u64).map_err(wire)?;
        self.owners.lock().insert(name, Owner { spec: current, fiber: fiber.clone(), revision, ready, },);
        Ok(fiber)
    ''',
    "ROOT_FINISH": '''
        let result = self.context.fiber().dispose().await.map_err(kernel);
        self.claim = None; result
    ''',
    "ROOT_DROP": '''
        let Some(claim) = self.claim.take() else { return; };
        let cleanup = self.context.fiber().dispose();
        self.executor.spawn(async move {
            let result = cleanup.await; drop(claim);
            if let Err(problem) = result { eprintln!("运行时取消后的根清理失败：{problem}"); }
        });
    ''',
}


def closed_structure(fragments: dict[str, str]) -> None:
    for name, expected in CLOSED.items():
        if body(fragments[name]) != compact(expected):
            raise RuntimeError(f"unsupported complete construction/ownership body: {name}")


def parsed_new(fragment: str) -> tuple[str, str]:
    match = re.fullmatch(
        r'Self::validated\((name|"(?:\\.|[^"\\])*"),(inject|Inject::new\(\)),\|raw:&R\|Ok\(raw.clone\(\)\),apply\)',
        body(fragment))
    if not match:
        raise RuntimeError("unsupported complete ComponentDef::new delegation")
    return match[1], "dependencies" if match[2] == "inject" else "[]"


def parsed_allocate(fragment: str) -> int:
    match = re.fullmatch(
        r'counter\.fetch_update\(Ordering::Relaxed,Ordering::Relaxed,\|current\|\{current.checked_add\((\d+)\)\}\)\.ok\(\)',
        body(fragment))
    if not match:
        raise RuntimeError("unsupported complete identity allocation body")
    return int(match[1])


def parsed_apply(fragment: str) -> tuple[dict[str, str], str, str, str]:
    match = re.fullmatch(
        r'letrequest=payload::Apply\{([^{}]+)\};letoutput=self.lifecycle_request\('
        r'(&generation.component.connection|&self.0.connection),'
        r'("(?:\\.|[^"\\])*"),value\(&request\)\?,'
        r'(self.0.options.apply_timeout|self.0.options.dispose_timeout),\).await\?;decode\(output.value\(\)\)',
        body(fragment))
    if not match:
        raise RuntimeError("unsupported complete backend apply projection")
    allowed = {
        "plugin": {"generation.component.plugin.clone()": "generation.plugin",
                   "generation.component.info.name.clone()": "generation.component"},
        "component": {"generation.component.info.name.clone()": "generation.component",
                      "generation.component.plugin.clone()": "generation.plugin"},
        "library": {"generation.component.library": "generation.library", "None": "none"},
        "context": {"generation.id": "generation.context", "0": "0"},
        "config": {"generation.config.as_ref()": "generation.config"},
        "inherited": {"generation.inherited.as_deref()": "generation.inherited", "None": "none"},
    }
    projection = {}
    for field in match[1].split(","):
        if not field:
            continue
        key, value = field.split(":", 1)
        if key in projection or key not in allowed or value not in allowed[key]:
            raise RuntimeError("unsupported or repeated backend apply field")
        projection[key] = allowed[key][value]
    if set(projection) != set(allowed):
        raise RuntimeError("incomplete backend apply field domain")
    receiver = "generation.connection" if match[2] == "&generation.component.connection" else "fallback"
    timeout = "timeout" if match[4] == "self.0.options.apply_timeout" else "otherTimeout"
    return projection, receiver, match[3], timeout


def certificate(fragments: dict[str, str]) -> str:
    name, dependencies = parsed_new(fragments["NEW"])
    increment = parsed_allocate(fragments["ALLOCATE"])
    fields, connection, method, timeout = parsed_apply(fragments["APPLY_GENERATION"])
    values = ", ".join(fields[key] for key in ["plugin", "component", "library", "context", "config", "inherited"])
    return f'''import PluginSourceVectors
namespace Cordis.PluginSourceCertificate
open PluginSource
def parsedDefinition (name : String) (dependencies : List Bindings.Dependency)
    (implementation : Payload) : PluginPrinciple.Definition Payload :=
  ⟨{name}, {dependencies}, implementation⟩
theorem source_constructor_all_implementations (name : String)
    (dependencies : List Bindings.Dependency) (implementation : Payload) :
    parsedDefinition name dependencies implementation = ⟨name, dependencies, implementation⟩ := by rfl
def parsedRoute (generation : RemoteGeneration Payload Inherited)
    (timeout otherTimeout fallback : Nat) : ApplyRoute Payload Inherited :=
  ⟨{connection}, {method}, ⟨{values}⟩, {timeout}⟩
theorem source_route_all_payloads (generation : RemoteGeneration Payload Inherited)
    (timeout otherTimeout fallback : Nat) :
    parsedRoute generation timeout otherTimeout fallback = routeApply generation timeout := by rfl
def parsedAllocate (counter : Nat) : Option Nat × Nat :=
  if counter + {increment} ≤ identityLimit then (some counter, counter + {increment}) else (none, counter)
theorem source_allocator_all_frontiers (counter : Nat) :
    parsedAllocate counter = allocate counter := by
  unfold parsedAllocate allocate WireSemantics.allocateId
  by_cases available : counter < identityLimit
  · have checked : counter + 1 ≤ identityLimit := by omega
    simp [available, checked]
  · have checked : ¬ counter + 1 ≤ identityLimit := by omega
    simp [available, checked]
end Cordis.PluginSourceCertificate
'''


def dependency_boundary(workspace: dict, kernel: dict, lock: dict) -> dict:
    if any(key in manifest for manifest in [workspace, kernel] for key in ["patch", "replace"]):
        raise RuntimeError("dependency source overrides require boundary review")
    if (ROOT / "crates/cordis/build.rs").exists() or kernel.get("package", {}).get("build"):
        raise RuntimeError("kernel build script requires boundary review")
    direct = dict(kernel.get("dependencies", {}))
    if kernel.get("build-dependencies") or kernel.get("target"):
        raise RuntimeError("kernel build/target dependency sections require boundary review")
    expected = {"slotmap", "parking_lot", "futures-util", "thiserror"}
    if set(direct) != expected:
        raise RuntimeError("kernel production dependency set crosses the reviewed foundation boundary")
    inherited = workspace["workspace"]["dependencies"]
    for name, dependency in direct.items():
        if dependency != {"workspace": True}:
            raise RuntimeError("unreviewed kernel dependency selection")
        resolved = inherited[name]
        if isinstance(resolved, dict) and any(key in resolved for key in ["path", "git", "package"]):
            raise RuntimeError("kernel foundation dependency resolves to an unreviewed source")
    packages = lock["package"]
    by_name = {}
    for package in packages:
        by_name.setdefault(package["name"], []).append(package)
    local = {package["name"] for package in packages if "source" not in package}
    visited, pending = set(), list(direct)
    # Conservatively traverse all locked versions and optional lockfile edges.
    while pending:
        name = pending.pop()
        if name in visited:
            continue
        if name in local:
            raise RuntimeError(f"kernel production graph reaches a workspace application package: {name}")
        if name not in by_name:
            raise RuntimeError(f"kernel production graph lacks a locked package: {name}")
        visited.add(name)
        for package in by_name[name]:
            pending.extend(dependency.split()[0] for dependency in package.get("dependencies", []))
    return {"direct_production_dependencies": sorted(direct), "locked_transitive_overapproximation": sorted(visited),
            "workspace_packages_excluded": sorted(local), "development_dependencies_excluded_from_production": sorted(kernel.get("dev-dependencies", {}))}


def execute(command: list[str], *, success: bool = True, cwd: Path = FORMAL, timeout: int = 180) -> subprocess.CompletedProcess[str]:
    result = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=timeout)
    if success and result.returncode:
        raise RuntimeError(f"{' '.join(command)} failed:\n{result.stdout}{result.stderr}")
    return result


def observations(output: str) -> dict[tuple[str, int], str]:
    result = {}
    for line in output.splitlines():
        kind, number, value = line.split("\t", 2)
        key = kind, int(number)
        if key in result:
            raise RuntimeError(f"duplicate plugin source case: {key}")
        result[key] = value
    if set(result) != {(kind, i) for kind, count in CASE_COUNTS.items() for i in range(count)}:
        raise RuntimeError("incomplete plugin source case domain")
    return result


def write_receipt(path: Path | None, evidence: dict) -> None:
    if path is None:
        return
    path.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile("w", dir=path.parent, delete=False) as stream:
        temporary = Path(stream.name)
        json.dump(evidence, stream, ensure_ascii=False, indent=2)
        stream.write("\n")
    try:
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def verify(args: argparse.Namespace) -> dict:
    inputs = {ROOT / path for path, _ in SOURCES.values()} | {
        Path(__file__).resolve(), ROOT / "Cargo.toml", ROOT / "Cargo.lock", ROOT / "crates/cordis/Cargo.toml",
        ROOT / "crates/cordis-testkit/Cargo.toml", ROOT / "rust-toolchain.toml",
        ROOT / "crates/cordis/tests/plugin_principle.rs", FORMAL / "PluginPrinciple.lean",
        FORMAL / "PluginSourceVectors.lean", FORMAL / "plugin_principle_probe.rs",
        FORMAL / "check_kernel_refinement.py", FORMAL / "lakefile.toml", FORMAL / "lake-manifest.json", FORMAL / "lean-toolchain",
    } | set((ROOT / "crates/cordis/src").rglob("*.rs")) \
      | set((ROOT / "crates/cordis-testkit/src").rglob("*.rs")) | set(FORMAL.glob("*.lean"))
    def snapshot() -> dict[str, str]:
        return {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted(inputs)}
    before = snapshot()
    fragments = {name: braced((ROOT / path).read_text(), anchor) for name, (path, anchor) in SOURCES.items()}
    manifests = [tomllib.loads((ROOT / name).read_text()) for name in ["Cargo.toml", "crates/cordis/Cargo.toml", "Cargo.lock"]]
    dependencies = dependency_boundary(*manifests)
    closed_structure(fragments)
    template = (FORMAL / "plugin_principle_probe.rs").read_text()
    for name in COMPILED:
        unique(template, f"// @SOURCE:{name}@")
    # Rebuild the transitive proof imports before elaborating the source-facing
    # program; a standalone invocation must bind the same source closure.
    execute([args.lake, "build", "PluginPrinciple"])
    execute([args.lake, "env", "lean", "-o", ".lake/build/lib/lean/PluginSourceVectors.olean", "PluginSourceVectors.lean"])
    expected = observations(execute([args.lake, "env", "lean", "--run", "PluginSourceVectors.lean"]).stdout)
    controls = {}
    with tempfile.TemporaryDirectory(prefix="cordis-plugin-principle-") as directory:
        temporary = Path(directory)
        cert = temporary / "Certificate.lean"
        certificate_source = certificate(fragments)
        cert.write_text(certificate_source)
        execute([args.lake, "env", "lean", "--trust=0", str(cert)])

        def probe(parts: dict[str, str], name: str) -> subprocess.CompletedProcess[str]:
            source = template
            for key in COMPILED:
                source = source.replace(f"// @SOURCE:{key}@", parts[key])
            program, binary = temporary / f"{name}.rs", temporary / name
            program.write_text(source)
            execute([args.rustc, "--edition=2024", "-O", str(program), "-o", str(binary)])
            return execute([str(binary)], success=False)

        actual_process = probe(fragments, "source")
        if actual_process.returncode:
            raise RuntimeError(f"source probe execution failed: {actual_process.stderr}")
        actual = observations(actual_process.stdout)
        mismatch = [key for key in expected if actual[key] != expected[key]]
        if mismatch:
            key = mismatch[0]
            raise RuntimeError(f"plugin source differs at {key}: Lean={expected[key]!r}, Rust={actual[key]!r}")
        mutations = {
            "constructor_discards_name": ("NEW", "Self::validated(name, inject,", 'Self::validated("", inject,'),
            "constructor_discards_dependencies": ("NEW", "Self::validated(name, inject,", "Self::validated(name, Inject::new(),"),
            "validated_discards_callback": ("VALIDATED", "Box::pin(apply(ctx, config))", "Box::pin(async { Ok(()) })"),
            "apply_changes_plugin": ("APPLY_GENERATION", "plugin: generation.component.plugin.clone()", "plugin: generation.component.info.name.clone()"),
            "apply_discards_context": ("APPLY_GENERATION", "context: generation.id", "context: 0"),
            "apply_discards_library": ("APPLY_GENERATION", "library: generation.component.library", "library: None"),
            "apply_discards_configuration": ("APPLY_GENERATION", "config: generation.config.as_ref()", "config: &0"),
            "apply_discards_inherited_state": ("APPLY_GENERATION", "inherited: generation.inherited.as_deref()", "inherited: None"),
            "apply_uses_manager_connection": ("APPLY_GENERATION", "&generation.component.connection", "&self.0.connection"),
            "apply_changes_method": ("APPLY_GENERATION", '"apply"', '"describe"'),
            "apply_changes_deadline": ("APPLY_GENERATION", "self.0.options.apply_timeout", "self.0.options.dispose_timeout"),
            "apply_hides_backend_error": ("APPLY_GENERATION", "decode(output.value())", "Ok(())"),
            "identity_skips_frontier": ("ALLOCATE", "checked_add(1)", "checked_add(2)"),
        }
        for name, (part, old, new) in mutations.items():
            unique(fragments[part], old)
            mutant = dict(fragments, **{part: fragments[part].replace(old, new)})
            observed = probe(mutant, name)
            if observed.returncode == 0 and observations(observed.stdout) == expected:
                raise RuntimeError(f"compiled negative control passed: {name}")
            controls[name] = "rejected"
            try:
                changed_certificate = certificate(mutant)
                closed_structure(mutant)
            except RuntimeError:
                controls[f"source_{name}"] = "rejected"
            else:
                cert.write_text(changed_certificate)
                rejected = execute([args.lake, "env", "lean", "--trust=0", str(cert)], success=False)
                if rejected.returncode == 0 or "error:" not in rejected.stdout + rejected.stderr:
                    raise RuntimeError(f"universal source certificate accepted negative control: {name}")
                controls[f"certificate_{name}"] = "rejected"
        mutant = dict(fragments, ALLOCATE='''pub(crate) fn allocate(counter: &AtomicU64) -> Option<u64> {
            Some(counter.fetch_add(1, Ordering::Relaxed))
        }''')
        wrapped = probe(mutant, "identity_wraps")
        if wrapped.returncode or observations(wrapped.stdout) == expected:
            raise RuntimeError("wrapping identity negative control did not execute with a semantic mismatch")
        controls["identity_wraps"] = "rejected"
        try:
            parsed_allocate(mutant["ALLOCATE"])
        except RuntimeError:
            controls["source_identity_wraps"] = "rejected"
        else:
            raise RuntimeError("source allocator accepted wraparound")

        structural_mutations = {
            "mount_omits_child_recovery": ("MOUNT", "child.begin_dispose();", "drop(child.clone());"),
            "remote_ignores_required_dependency": ("REMOTE_DEFINITION", "*required,", "false,"),
            "remote_ignores_minimum_version": ("REMOTE_DEFINITION", "binding.interface.minor >= minor", "true"),
            "manager_omits_owned_shutdown": ("MANAGER_INITIALIZE", "retained.shutdown_contents().await.map_err(kernel)", "Ok(())"),
            "web_omits_declared_provider": ("WEB_MOUNT", 'Inject::new().with_named("cordis.web@1")', "Inject::new()"),
            "runtime_releases_claim_before_cleanup": ("ROOT_DROP", "let result = cleanup.await;\n            drop(claim);", "drop(claim);\n            let result = cleanup.await;"),
            "realm_reuses_default_identity": ("REALM_NEW", 'Self(crate::identity::allocate(&NEXT).expect("realm identity exhausted"))', "Self::DEFAULT"),
        }
        for name, (part, old, new) in structural_mutations.items():
            unique(fragments[part], old)
            mutant = dict(fragments, **{part: fragments[part].replace(old, new)})
            try:
                closed_structure(mutant)
            except RuntimeError:
                controls[name] = "rejected"
            else:
                raise RuntimeError(f"complete source shape accepted negative control: {name}")
        injected = copy.deepcopy(manifests)
        injected[1]["dependencies"]["cordis-runtime"] = {"workspace": True}
        try:
            dependency_boundary(*injected)
        except RuntimeError:
            controls["kernel_depends_on_runtime"] = "rejected"
        else:
            raise RuntimeError("dependency boundary accepted runtime in kernel")
        overridden = copy.deepcopy(manifests)
        overridden[0]["workspace"]["dependencies"]["slotmap"] = {"path": "plugins/new-business"}
        try:
            dependency_boundary(*overridden)
        except RuntimeError:
            controls["kernel_foundation_redirected_to_plugin"] = "rejected"
        else:
            raise RuntimeError("dependency boundary accepted a plugin source override")

    # Run actual production state machines once, in addition to extracted-method
    # observations. The caller's pinned Cargo environment is preserved.
    cargo = execute([args.cargo, "test", "--locked", "-p", "cordis", "--test", "plugin_principle"], cwd=ROOT, timeout=300)
    match = re.search(r"test result: ok\. (\d+) passed; 0 failed; 0 ignored; 0 measured; 0 filtered out", cargo.stdout)
    if not match or int(match[1]) != 4:
        raise RuntimeError("real kernel plugin/context integration domain changed or was filtered")
    after = snapshot()
    if before != after:
        raise RuntimeError("plugin principle inputs changed during verification")
    return {
        "status": "passed", "compiled_input_cases": sum(CASE_COUNTS.values()), "case_domains": CASE_COUNTS,
        "negative_controls": controls, "source_sha256": after, "source_before_sha256": before,
        "extracted_functions": {key: hashlib.sha256(value.encode()).hexdigest() for key, value in fragments.items()},
        "source_certificate_lean": certificate_source,
        "source_certificate_sha256": hashlib.sha256(certificate_source.encode()).hexdigest(),
        "production_dependency_boundary": dependencies,
        "closed_assembly_bodies": sorted(CLOSED),
        "actual_kernel_integration": {"tests_passed": 4, "parameterized_plugin_scenarios": 12,
            "tests": ["extensions_replace_through_declared_dependencies_and_owned_effects",
                      "composed_extensions_keep_instance_identity_and_parent_ownership",
                      "registered_inverses_restore_context_state",
                      "coeffect_views_preserve_parent_metadata_and_registration_realm"],
            "stdout": cargo.stdout},
        "support_contracts": [
            "Constructor methods use actual std Arc, Any downcast, closures and futures; Context and Inject are observation adapters in extracted cases.",
            "The entire apply_generation method runs with an argument-recording transport and identity serialization adapter; wire encoding and backend execution are separate existing gates.",
            "Freshness extraction uses the production AtomicU64 helper; finite sequential boundary executions complement the arbitrary-trace model and production concurrency regression.",
            "Closed framework bodies establish recognized assembly and ownership placement; helper implementations and arbitrary business callbacks are not universally verified by body recognition.",
            "The Cargo lock traversal conservatively includes optional edges and excludes development dependencies; it certifies this production dependency boundary, not a globally minimal kernel.",
            "Real kernel integration executes public Context, ComponentDef, Inject, registry replacement, typed metadata, isolated providers and effect cleanup without a custom kernel.",
            "The restricted extractor, source recognizer, Rust compiler and documented adapters are the source certificate trust boundary.",
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--lake", default="lake")
    parser.add_argument("--rustc", default="rustc")
    parser.add_argument("--cargo", default="cargo")
    parser.add_argument("--evidence", type=Path)
    args = parser.parse_args()
    write_receipt(args.evidence, {"status": "running"})
    try:
        evidence = verify(args)
    except (RuntimeError, OSError, ValueError, KeyError, subprocess.TimeoutExpired) as error:
        write_receipt(args.evidence, {"status": "failed", "error": str(error)})
        print(error, file=sys.stderr)
        return 1
    write_receipt(args.evidence, evidence)
    print(json.dumps(evidence, ensure_ascii=False, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
