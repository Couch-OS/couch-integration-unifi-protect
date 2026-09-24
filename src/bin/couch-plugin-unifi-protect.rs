//! Standalone package entry point. stdout is reserved for framed protocol.
fn main() {
    let manifest = serde_json::from_str(include_str!("../../plugin.json"))
        .expect("embedded integration manifest");
    if couch_plugin::serve::<couch_unifi_protect::adapter::ProtectConnection>(manifest).is_err() {
        std::process::exit(1);
    }
}
