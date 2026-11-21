use super::gate::{checked, library};
use super::*;

#[test]
fn native_thread_receives_management_reply_before_dependency_reload_or_group_restart() {
    let case = TestGuard::for_package(Path::new(env!("CARGO_MANIFEST_DIR")), "management-impact");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let target = cordis_testkit::build::shared_target_dir(&root);
    let artifacts = {
        let _lock = cordis_testkit::build::shared_build_lock(&root).unwrap();
        checked(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args([
                    "build",
                    "-p",
                    "cordis-host",
                    "-p",
                    "kv-mem",
                    "-p",
                    "kv-file",
                    "--target-dir",
                ])
                .arg(&target),
        );
        checked(
            Command::new(env!("CARGO"))
                .current_dir(&root)
                .args([
                    "build",
                    "--manifest-path",
                    "crates/cordis-host/tests/fixtures/Cargo.toml",
                    "-p",
                    "management-caller",
                    "--target-dir",
                ])
                .arg(&target),
        );
        let mut files = vec!["cordis-host".to_owned()];
        for name in ["kv_mem", "kv_file", "management_caller"] {
            files.push(library(Path::new(""), name).to_str().unwrap().to_owned());
        }
        cordis_testkit::build::snapshot_artifacts(
            &case,
            "management-tools",
            "debug",
            &files.iter().map(String::as_str).collect::<Vec<_>>(),
        )
        .unwrap()
    };
    for replace in [false, true] {
        let work = project(
            &case,
            if replace {
                "group-restart"
            } else {
                "optional-service"
            },
        );
        let plugins = work.join("plugins");
        fs::create_dir_all(&plugins).unwrap();
        fs::copy(env!("CARGO_BIN_EXE_cordis"), work.join("cordis")).unwrap();
        fs::copy(artifacts.join("cordis-host"), work.join("cordis-host")).unwrap();
        let caller = library(Path::new(""), "management_caller");
        fs::copy(artifacts.join(&caller), plugins.join(&caller)).unwrap();
        let (provider, component, group, config) = if replace {
            let base = library(Path::new(""), "kv_mem");
            fs::copy(artifacts.join(&base), plugins.join(&base)).unwrap();
            fs::copy(
                library(&artifacts, "kv_file"),
                work.join("candidate")
                    .with_extension(std::env::consts::DLL_EXTENSION),
            )
            .unwrap();
            (base, "kv-store", "caller", "path='values.cbor'")
        } else {
            (caller.clone(), "provider", "provider", "enabled=false")
        };
        let role = if replace { "admin" } else { "reader" };
        fs::write(work.join("cordis.toml"), format!(
            "[runtime]\ncheckpoint_ms=0\nconfirm_ms=0\nretired_image_limit=0\ninvoke_timeout_ms=4000\nlifecycle_timeout_ms=2000\n\
             [instance.caller]\nplugin={}\ncomponent='{role}'\ngroup='caller'\ngrants={{'cordis.manage@1'=['configure','install','replace']}}\nconfig={{directory={}}}\n\
             [instance.provider]\nplugin={}\ncomponent='{component}'\ngroup='{group}'\nconfig={{{config}}}\n",
            quoted(&caller), quoted(&work), quoted(&provider)
        )).unwrap();
        let mut core = Process::start_binary(&work.join("cordis"), &work, role);
        active(&work, "provider");
        let start = active(&work, "caller");
        let pid = |s: &Value| {
            s["groups"]
                .as_array()
                .unwrap()
                .iter()
                .find(|g| g["name"] == "caller")
                .unwrap()["pid"]
                .clone()
        };
        let previous = instance(&start, "caller").unwrap()["context"].clone();
        fs::write(work.join("go"), "").unwrap();
        until("管理应答与实例换代", || {
            let s = status(&work)?;
            let caller = instance(&s, "caller")?;
            (work.join("accepted").is_file()
                && caller["state"] == "active"
                && if replace {
                    pid(&s) != pid(&start)
                        && s["last_swap"]
                            .as_array()?
                            .iter()
                            .any(|swap| swap["succeeded"] == true)
                } else {
                    caller["context"] != previous
                        && fs::read_to_string(work.join("value")).ok()?.as_str() == "107"
                })
            .then_some(())
        });
        checked(
            Command::new("/bin/kill").args(["-TERM", &core.0.as_ref().unwrap().id().to_string()]),
        );
        core.finish();
    }
    case.success();
}
