fn main() {
    println!("cargo:rerun-if-env-changed=CLAUDE_SYNC_RELAY_URL");
    println!("cargo:rerun-if-env-changed=CLAUDE_SYNC_GITHUB_CLIENT_ID");
    tauri_build::build()
}
