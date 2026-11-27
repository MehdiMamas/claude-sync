//! The updater public key is the v1 key, byte for byte: a changed key would strand every v1 client
//! (they could not verify an update), and the command line trusts the same key.
const V1_KEY: &str = "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDU0QjY2QkU1RTM0MTI2NDkKUldSSkprSGo1V3UyVlByU2doaS9VNmM1eWNwdHBhRkFQdkdsZ1hsUXI4Tm50Z3FFYnlXLzNmN0sK";

#[test]
fn the_updater_key_in_tauri_conf_is_the_v1_key() {
    let conf = include_str!("../../../app/src-tauri/tauri.conf.json");
    let v: serde_json::Value = serde_json::from_str(conf).unwrap();
    assert_eq!(v["plugins"]["updater"]["pubkey"], V1_KEY);
    assert_eq!(claude_sync_cli::update::updater_pubkey().unwrap(), V1_KEY);
}
