use std::process::Command;

#[test]
fn unsupported_provider_fails_before_runtime_work() {
    let output = Command::new(env!("CARGO_BIN_EXE_tagent"))
        .env("TAGENT_PROVIDER", "unsupported")
        .env("TAGENT_API_KEY", "not-used")
        .output()
        .expect("failed to run tagent binary");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unsupported TAGENT_PROVIDER"));
}
