use cordis_runtime::config;
use cordis_supervisor::manager::RetiredImageBasis;

#[test]
fn native_recovery_basis_is_explicit_and_legacy_config_stays_cumulative() {
    let legacy = config::parse(b"[runtime]\nretired_image_limit = 20\n").unwrap();
    assert_eq!(
        legacy.runtime.retired_image_basis,
        RetiredImageBasis::Cumulative
    );
    for (name, expected) in [
        ("cumulative", RetiredImageBasis::Cumulative),
        ("open-handles", RetiredImageBasis::OpenHandles),
        ("mapped-images", RetiredImageBasis::MappedImages),
    ] {
        let text = format!("[runtime]\nretired_image_basis = '{name}'\n");
        let configuration = config::parse(text.as_bytes()).unwrap();
        assert_eq!(configuration.runtime.retired_image_basis, expected);
    }
    assert!(config::parse(b"[runtime]\nretired_image_basis = 'unknown'\n").is_err());
}
