use couch_plugin::{testing, Manifest};

fn adapter() -> testing::Adapter<'static> {
    testing::Adapter {
        binary: std::path::Path::new(env!("CARGO_BIN_EXE_couch-plugin-unifi-protect")),
        manifest_json: include_str!("../plugin.json"),
    }
}

#[test]
fn manifests_are_protocol_4_and_feed_shaped() {
    let manifest: Manifest = serde_json::from_str(include_str!("../plugin.json")).unwrap();
    manifest.validate().unwrap();
    assert_eq!(manifest.protocol_version, 4);
    assert_eq!(manifest.min_core_protocol_version, 4);
    assert_eq!(manifest.id, "unifi-protect");
    assert_eq!(manifest.settings.len(), 2);
    assert_eq!(manifest.children.len(), 1);
    assert_eq!(manifest.children[0].kind, "camera");
    assert!(manifest.pairing.is_some_and(|pairing| pairing.required));

    let integration: serde_json::Value =
        serde_json::from_str(include_str!("../integration.json")).unwrap();
    let mut keys: Vec<_> = integration.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "binary",
            "cargo_manifest",
            "cargo_package",
            "id",
            "manifest",
            "protocol_version",
            "schema",
            "synthetic",
            "tier",
        ]
    );
    assert_eq!(integration["protocol_version"], 4);
    assert_eq!(integration["id"], "unifi-protect");
}

#[test]
fn concurrent_package_startup_is_offline_and_race_free() {
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let threads: Vec<_> = (0..4)
        .map(|_| {
            let barrier = barrier.clone();
            std::thread::spawn(move || {
                barrier.wait();
                let package = testing::Package::new(adapter());
                let mut host = package.host();
                assert_eq!(
                    host.configure(serde_json::json!({
                        "address": "192.0.2.1",
                        "api_key": "fixture-private-key"
                    })),
                    Ok(())
                );
            })
        })
        .collect();
    barrier.wait();
    for thread in threads {
        thread.join().expect("concurrent package startup");
    }
}

#[test]
fn malformed_settings_are_refused_without_network_io() {
    let package = testing::Package::new(adapter());
    let mut host = package.host();
    assert!(host
        .configure(serde_json::json!({
            "address": "https://192.0.2.1",
            "api_key": "fixture-private-key"
        }))
        .is_err());
    assert!(host
        .configure(serde_json::json!({
            "address": "192.0.2.1",
            "api_key": ""
        }))
        .is_err());
}
