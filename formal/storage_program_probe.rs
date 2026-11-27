// Compiles with exact production storage bodies supplied by storage_program_probe.py.
// std::fs performs real I/O; wrappers record calls and inject one error or process
// interruption at a selected boundary. Paths and directory entries remain real.
use std::{
    collections::HashMap, fmt, io::{Read, Write}, path::{Path, PathBuf}, sync::{Arc, Mutex},
};

#[derive(Clone, Debug)] struct WireError(String);
impl WireError { fn new(_: u32, text: impl Into<String>) -> Self { Self(text.into()) } }
impl fmt::Display for WireError { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { self.0.fmt(f) } }
type Result<T> = std::result::Result<T, WireError>;
fn parameter(value: impl fmt::Display) -> WireError { WireError::new(2, value.to_string()) }
struct Limits { max_frame: usize }
impl Limits { const DEFAULT: Self = Self { max_frame: 16 * 1024 * 1024 }; }
mod config {
    use super::*;
    pub struct Configuration { pub bytes: String }
    fn instance_name_shape(value: &str) -> bool { !value.is_empty() && value.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') }
    pub fn instance_name(value: &str) -> Result<()> {
        if instance_name_shape(value) { Ok(()) } else { Err(parameter("invalid fixture name")) }
    }
}
struct Overlay { bytes: String }
mod payload { pub struct State { pub bytes: Vec<u8> } }
#[derive(Default)] struct CheckpointCache;
impl CheckpointCache { fn clear(&mut self) {} }
struct CacheMutex<T>(Mutex<T>);
impl<T> CacheMutex<T> {
    fn new(value: T) -> Self { Self(Mutex::new(value)) }
    fn lock(&self) -> std::sync::MutexGuard<'_, T> { self.0.lock().unwrap() }
}
#[derive(Clone)] struct Store { directory: PathBuf, checkpoints: Arc<CacheMutex<CheckpointCache>> }

#[derive(Default)] struct Probe {
    root: PathBuf, phase: String, calls: Vec<String>, semantic: Vec<String>,
    fail_at: usize, crash: bool, fired: bool, mode: String, plan: String,
}
static PROBE: Mutex<Option<Probe>> = Mutex::new(None);
fn label(path: &Path) -> String {
    let probe = PROBE.lock().unwrap();
    let p = probe.as_ref().unwrap();
    path.strip_prefix(&p.root).unwrap_or(path).to_string_lossy().into_owned()
}
fn with_io<T>(op: &str, first: &Path, second: Option<&Path>, body: impl FnOnce() -> std::io::Result<T>) -> std::io::Result<T> {
    let a = label(first); let b = second.map(label).unwrap_or_default();
    let (inject, crash) = {
        let mut probe = PROBE.lock().unwrap(); let p = probe.as_mut().unwrap();
        p.calls.push(format!("{}\t{}\t{}\t{}", p.phase, op, a, b));
        let inject = p.calls.len() == p.fail_at && !p.fired;
        if inject { p.fired = true; }
        (inject, p.crash)
    };
    if inject {
        if crash {
            let (mode, plan) = {
                let p = PROBE.lock().unwrap(); let p = p.as_ref().unwrap();
                (p.mode.clone(), p.plan.clone())
            };
            report(&mode, "crashed", &plan);
            std::io::stdout().flush().unwrap();
            // Exit without unwinding. The driver starts a fresh process over
            // this directory to run the actual recovery method.
            std::process::exit(86);
        }
        return Err(std::io::Error::other("injected I/O failure"));
    }
    body()
}
fn semantic(event: String) { let mut p = PROBE.lock().unwrap(); let p = p.as_mut().unwrap(); p.semantic.push(format!("{}\t{event}", p.calls.len())); }
fn key(path: &Path) -> Option<usize> {
    match path.file_name()?.to_str()? {
        "deployment-base-v1.toml" => Some(0), "a.state" => Some(1),
        "instances.toml" => Some(2), "sources.json" => Some(3),
        "b.state" => Some(4), "c.state" => Some(5), "z.state" => Some(6), _ => None,
    }
}
fn active_phase() -> String { PROBE.lock().unwrap().as_ref().unwrap().phase.clone() }
fn set_phase(value: &str) { PROBE.lock().unwrap().as_mut().unwrap().phase = value.into(); }
fn root_target(path: &Path) -> bool { label(path).split('/').count() == 1 }

struct File { inner: std::fs::File, path: PathBuf, directory: bool }
impl File {
    fn create(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let inner = with_io("create", path, None, || std::fs::File::create(path))?;
        if active_phase() == "commit" && root_target(path) {
            if let Some(name) = path.file_name().and_then(|x| x.to_str()).and_then(|x| x.strip_suffix(".next")) {
                if let Some(id) = key(Path::new(name)) { semantic(format!("create:{id}")); }
            }
        }
        if active_phase() == "prepare" && label(path).starts_with(".deployment-staging/") {
            if let Some(name) = path.file_name().and_then(|x| x.to_str()).and_then(|x| x.strip_suffix(".absent")) {
                if let Some(id) = key(Path::new(name)) { semantic(format!("preserve:{id}")); }
            }
        }
        Ok(Self { inner, path: path.into(), directory: false })
    }
    fn open(path: impl AsRef<Path>) -> std::io::Result<Self> {
        let path = path.as_ref();
        let inner = with_io("open", path, None, || std::fs::File::open(path))?;
        let directory = inner.metadata()?.is_dir();
        Ok(Self { inner, path: path.into(), directory })
    }
    fn lock(&self) -> std::io::Result<()> {
        with_io("lock", &self.path, None, || self.inner.lock())
    }
    fn sync_all(&self) -> std::io::Result<()> {
        with_io(if self.directory { "sync-dir" } else { "sync-file" }, &self.path, None, || self.inner.sync_all())?;
        let phase = active_phase(); let relative = label(&self.path);
        if phase == "prepare" && self.directory && relative == ".deployment-staging" {
            semantic("sync-journal".into());
        } else if phase == "prepare" && self.directory && relative.is_empty() {
            semantic("sync-journal-parent".into());
            // The source entry immediately calls commit_prepared after this
            // successful preparation barrier. Phase is observation state only.
            set_phase("commit");
        } else if phase == "commit" && self.directory && relative.is_empty() {
            let undo_exists = self.path.join(".deployment-undo").exists();
            semantic(if undo_exists { "sync-data" } else { "sync-decision" }.into());
        } else if phase == "commit" && !self.directory && root_target(&self.path) {
            semantic("sync-file".into());
        }
        Ok(())
    }
}
impl Read for File {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        with_io("read", &self.path, None, || self.inner.read(bytes))
    }
}
impl Write for File {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        with_io("write", &self.path, None, || self.inner.write(bytes))
    }
    fn write_all(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        with_io("write-all", &self.path, None, || self.inner.write_all(bytes))?;
        if active_phase() == "commit" && root_target(&self.path) {
            let value = std::str::from_utf8(bytes).unwrap().parse::<usize>().unwrap();
            semantic(format!("write:{value}"));
        }
        Ok(())
    }
    fn flush(&mut self) -> std::io::Result<()> { self.inner.flush() }
}
mod fs {
    use super::*;
    pub fn create_dir_all(path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref(); with_io("create-dir-all", path, None, || std::fs::create_dir_all(path))
    }
    pub fn create_dir(path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref(); with_io("create-dir", path, None, || std::fs::create_dir(path))
    }
    pub fn remove_dir_all(path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref(); with_io("remove-dir", path, None, || std::fs::remove_dir_all(path))
    }
    pub fn remove_file(path: impl AsRef<Path>) -> std::io::Result<()> {
        let path = path.as_ref();
        let result = with_io("remove", path, None, || std::fs::remove_file(path));
        if active_phase() == "commit" && root_target(path) &&
            (result.is_ok() || result.as_ref().err().is_some_and(|x| x.kind() == std::io::ErrorKind::NotFound)) {
            if let Some(id) = key(path) { semantic(format!("remove:{id}")); }
        }
        result
    }
    pub fn hard_link(from: impl AsRef<Path>, to: impl AsRef<Path>) -> std::io::Result<()> {
        let (from, to) = (from.as_ref(), to.as_ref());
        with_io("hard-link", from, Some(to), || std::fs::hard_link(from, to))?;
        if active_phase() == "prepare" && label(to).starts_with(".deployment-staging/") {
            if let Some(id) = key(from) { semantic(format!("preserve:{id}")); }
        }
        Ok(())
    }
    pub fn rename(from: impl AsRef<Path>, to: impl AsRef<Path>) -> std::io::Result<()> {
        let (from, to) = (from.as_ref(), to.as_ref());
        with_io("rename", from, Some(to), || std::fs::rename(from, to))?;
        let (a, b, phase) = (label(from), label(to), active_phase());
        if phase == "prepare" && a == ".deployment-staging" && b == ".deployment-undo" {
            semantic("rename-journal".into());
        } else if phase == "commit" && a == ".deployment-undo" && b == ".deployment-staging" {
            semantic("move-decision".into());
        } else if phase == "commit" && root_target(to) {
            if let Some(id) = key(to) { semantic(format!("rename:{id}")); }
        }
        Ok(())
    }
    pub struct DirEntry(std::fs::DirEntry);
    impl DirEntry {
        pub fn path(&self) -> PathBuf { self.0.path() }
        pub fn file_name(&self) -> std::ffi::OsString { self.0.file_name() }
        pub fn file_type(&self) -> std::io::Result<std::fs::FileType> {
            with_io("file-type", &self.0.path(), None, || self.0.file_type())
        }
    }
    pub struct ReadDir { entries: std::vec::IntoIter<std::fs::DirEntry>, path: PathBuf }
    impl Iterator for ReadDir {
        type Item = std::io::Result<DirEntry>;
        fn next(&mut self) -> Option<Self::Item> {
            self.entries.next().map(|entry| with_io("read-entry", &self.path, None, || Ok(DirEntry(entry))))
        }
    }
    pub fn read_dir(path: impl AsRef<Path>) -> std::io::Result<ReadDir> {
        let path = path.as_ref();
        let entries = with_io("read-dir", path, None, || std::fs::read_dir(path))?;
        let mut entries = entries.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        Ok(ReadDir { entries: entries.into_iter(), path: path.into() })
    }
}

/* PRODUCTION_FUNCTIONS */
impl<'a> PreparedDeployment<'a> {
    // Serialization is an explicit prevalidated-byte adapter. The production
    // commit_deployment body owns this one result throughout keys and writes.
    fn encode<S: std::borrow::Borrow<payload::State>>(
        base: &config::Configuration, overlay: &Overlay,
        states: &HashMap<String, Option<S>>, sources: &'a [u8],
    ) -> Result<Self> {
        let prepared = Self {
            base: base.bytes.clone(), overlay: overlay.bytes.clone(), sources,
            states: states.iter().map(|(name,value)| {
                (name.clone(), value.as_ref().map(|state| state.borrow().bytes.clone()))
            }).collect(),
        };
        PROBE.lock().unwrap().as_mut().unwrap().plan = prepared_plan(&prepared);
        Ok(prepared)
    }
}
impl Store {
/* PRODUCTION_METHODS */
}

fn paths() -> [&'static str; 7] { [BASE, "a.state", "instances.toml", "sources.json", "b.state", "c.state", "z.state"] }
fn contents(path: &Path) -> Option<Vec<u8>> {
    match std::fs::read(path) { Ok(bytes) => Some(bytes), Err(e) if e.kind() == std::io::ErrorKind::NotFound => None, Err(e) => panic!("{e}") }
}
fn image(directory: &Path) -> Vec<Option<Vec<u8>>> { paths().map(|p| contents(&directory.join(p))).to_vec() }
fn fixture(scenario: usize) -> (PreparedDeployment<'static>, Vec<Option<Vec<u8>>>, Vec<Option<Vec<u8>>>) {
    let before: Vec<_> = (0..paths().len()).map(|id| {
        if id == 5 || (scenario == 3 && id == 1) { None }
        else { Some(format!("{}", 10 + id).into_bytes()) }
    }).collect();
    let states = match scenario {
        0 => HashMap::new(),
        1 | 3 => HashMap::from([("a".into(), Some(b"21".to_vec()))]),
        2 => HashMap::from([("a".into(), None)]),
        4 => HashMap::from([("a".into(), Some(b"21".to_vec())), ("b".into(), None)]),
        _ => HashMap::from([("a".into(), Some(b"21".to_vec())), ("b".into(), None), ("c".into(), Some(b"25".to_vec())), ("z".into(), Some(b"26".to_vec()))]),
    };
    let prepared = PreparedDeployment { base: "20".into(), overlay: "22".into(), states, sources: b"23" };
    let mut after = before.clone(); after[0] = Some(b"20".to_vec()); after[2] = Some(b"22".to_vec()); after[3] = Some(b"23".to_vec());
    for (name, value) in &prepared.states { after[key(Path::new(&format!("{name}.state"))).unwrap()] = value.clone(); }
    (prepared, before, after)
}
fn fixture_store(root: &Path) -> Store {
    Store { directory: root.into(), checkpoints: Arc::new(CacheMutex::new(CheckpointCache)) }
}
fn prepared_plan(prepared: &PreparedDeployment<'_>) -> String {
    let mut changes = vec!["0:20".to_string()];
    let mut backups = vec![2, 3, 0];
    for (name, value) in &prepared.states {
        let id = key(Path::new(&format!("{name}.state"))).unwrap(); backups.push(id);
        changes.push(format!("{id}:{}", value.as_ref().map(|v| String::from_utf8(v.clone()).unwrap()).unwrap_or_else(|| "-".into())));
    }
    changes.extend(["2:22".into(), "3:23".into()]);
    format!("{}|{}", backups.iter().map(ToString::to_string).collect::<Vec<_>>().join(","), changes.join(","))
}
fn setup(root: &Path, scenario: usize) -> (Store, PreparedDeployment<'static>, Vec<Option<Vec<u8>>>, Vec<Option<Vec<u8>>>) {
    let (prepared, before, after) = fixture(scenario);
    std::fs::create_dir_all(root.join("consumed")).unwrap();
    std::fs::write(root.join("untouched"), b"frame").unwrap();
    for (id, bytes) in before.iter().enumerate() {
        if let Some(bytes) = bytes {
            let path = root.join(paths()[id]);
            std::fs::write(&path, bytes).unwrap();
            std::fs::File::open(path).unwrap().sync_all().unwrap();
        }
    }
    std::fs::File::open(root).unwrap().sync_all().unwrap();
    assert_eq!(image(root), before);
    (fixture_store(root), prepared, before, after)
}
fn report(tag: &str, result: &str, plan: &str) {
    let p = PROBE.lock().unwrap(); let p = p.as_ref().unwrap();
    println!("RESULT\t{tag}\t{result}\t{}\t{}\t{plan}", p.calls.len(), usize::from(p.fired));
    for event in &p.semantic { println!("SEM\t{event}"); }
    for (i, call) in p.calls.iter().enumerate() { println!("CALL\t{}\t{call}", i + 1); }
}
fn initialization_case(root: &Path, fail_at: usize) {
    *PROBE.lock().unwrap() = Some(Probe { root: root.into(), phase: "initialize".into(), ..Default::default() });
    let mut first = Vec::new();
    let result = prepare_directory_tree(root, |path| {
        first.push(path.to_path_buf());
        if fail_at != 0 && first.len() == fail_at {
            Err(io_error(std::io::Error::other("injected directory barrier failure")))
        } else { sync_directory_contents(path) }
    });
    let canonical = root.canonicalize().unwrap();
    let mut required = vec![canonical.join("consumed"), canonical.join("data")];
    required.extend(canonical.ancestors().map(Path::to_path_buf));
    assert!(fail_at <= required.len());
    assert_eq!(result.is_ok(), fail_at == 0, "directory barrier failure must reject initialization");
    let prefix = if fail_at == 0 { required.len() } else { fail_at };
    assert_eq!(first, required[..prefix], "initialization must stop at the first failed barrier");
    if let Ok(path) = &result { assert_eq!(path, &canonical); }
    std::fs::write(root.join("untouched"), b"frame").unwrap();
    let mut retry = Vec::new();
    let retried = prepare_directory_tree(root, |path| {
        retry.push(path.to_path_buf()); sync_directory_contents(path)
    }).unwrap();
    assert_eq!(retried, canonical);
    assert_eq!(retry, required, "retry must confirm children and every existing ancestor again");
    assert_eq!(std::fs::read(root.join("untouched")).unwrap(), b"frame");
    println!("INITIALIZATION\t{fail_at}\t{}\t{}", usize::from(result.is_ok()), required.len());
    for path in &required { println!("INITREQUIRED\t{}", path.display()); }
    for path in &first { println!("INITFIRST\t{}", path.display()); }
    for path in &retry { println!("INITRETRY\t{}", path.display()); }
}
fn main() {
    let args: Vec<_> = std::env::args().collect();
    let root = PathBuf::from(&args[1]); let scenario: usize = args[2].parse().unwrap();
    let mode = &args[3]; let fail_at: usize = args[4].parse().unwrap();
    if mode == "initialize" { initialization_case(&root, fail_at); return; }
    if mode == "resume" || mode == "resume-recovery" {
        let (_, before, after) = fixture(scenario);
        *PROBE.lock().unwrap() = Some(Probe { root: root.clone(), phase: "recover".into(), ..Default::default() });
        fixture_store(&root).restore_deployment_files().unwrap();
        let recovered = image(&root);
        if mode == "resume-recovery" { assert_eq!(recovered, before, "recovery restart must restore old image"); }
        else { assert!(recovered == before || recovered == after, "process restart exposed mixed deployment"); }
        assert_eq!(std::fs::read(root.join("untouched")).unwrap(), b"frame");
        println!("RESUMED\t{mode}");
        return;
    }
    let (store, prepared, before, after) = setup(&root, scenario);
    let plan = prepared_plan(&prepared);
    *PROBE.lock().unwrap() = Some(Probe { root: root.clone(), ..Default::default() });
    if mode == "recover" || mode == "recover-crash" {
        set_phase("prepare"); store.prepare_deployment(prepared.states.keys()).unwrap();
        for (id, value) in after.iter().enumerate() {
            match value {
                Some(bytes) => {
                    let staged = root.join(paths()[id]).with_added_extension("fixture");
                    std::fs::write(&staged, bytes).unwrap();
                    std::fs::rename(staged, root.join(paths()[id])).unwrap();
                },
                None => { let _ = std::fs::remove_file(root.join(paths()[id])); }
            }
        }
        *PROBE.lock().unwrap() = Some(Probe { root: root.clone(), phase: "recover".into(), fail_at, crash: mode == "recover-crash", mode: mode.clone(), plan: plan.clone(), ..Default::default() });
        let result = store.restore_deployment_files();
        let first = if result.is_ok() { "ok" } else { "error" };
        report(mode, first, &plan);
        PROBE.lock().unwrap().as_mut().unwrap().fail_at = 0;
        store.restore_deployment_files().unwrap();
        assert_eq!(image(&root), before, "restarted recovery must restore entire old image");
    } else {
        *PROBE.lock().unwrap() = Some(Probe { root: root.clone(), phase: "prepare".into(), fail_at, crash: mode == "crash", mode: mode.clone(), plan: plan.clone(), ..Default::default() });
        let base = config::Configuration { bytes: prepared.base.clone() };
        let overlay = Overlay { bytes: prepared.overlay.clone() };
        let states: HashMap<_,_> = prepared.states.iter().map(|(name,value)| {
            (name.clone(), value.as_ref().map(|bytes| payload::State {bytes:bytes.clone()}))
        }).collect();
        let result = store.commit_deployment(&base, &overlay, &states, prepared.sources);
        let plan = PROBE.lock().unwrap().as_ref().unwrap().plan.clone();
        let name = match &result {
            Ok(committed) => if committed.cleanup_error.is_some() { "committed-cleanup-error" } else { "committed" },
            Err(CommitError::NotCommitted(_)) => "not-committed",
            Err(CommitError::RecoveryRequired(_)) => "recovery-required",
        };
        report(mode, name, &plan);
        match result {
            Ok(_) => assert_eq!(image(&root), after, "acknowledged deployment must expose new image"),
            Err(CommitError::NotCommitted(_)) => assert_eq!(image(&root), before, "rejected deployment must restore old image"),
            _ => {
                PROBE.lock().unwrap().as_mut().unwrap().fail_at = 0;
                set_phase("recover"); store.restore_deployment_files().unwrap();
                let recovered = image(&root);
                assert!(recovered == before || recovered == after, "restart exposed a mixed deployment");
            }
        }
    }
    assert_eq!(std::fs::read(root.join("untouched")).unwrap(), b"frame", "unmentioned names obey frame condition");
}
