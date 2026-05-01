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

#[test]
fn help_does_not_require_api_key() {
    let output = Command::new(env!("CARGO_BIN_EXE_tagent"))
        .arg("--help")
        .env_remove("TAGENT_API_KEY")
        .env_remove("MINIMAX_API_KEY")
        .output()
        .expect("failed to run tagent binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("--date"));
    assert!(stdout.contains("providers"));
}

#[test]
fn config_check_reports_effective_settings_without_analysis() {
    let output = Command::new(env!("CARGO_BIN_EXE_tagent"))
        .args([
            "config",
            "check",
            "--provider",
            "openai",
            "--model",
            "gpt-4o-mini",
            "--concurrency",
            "2",
        ])
        .env("OPENAI_API_KEY", "test-key")
        .output()
        .expect("failed to run tagent binary");

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Configuration OK"));
    assert!(stdout.contains("Provider: openai"));
    assert!(stdout.contains("Batch concurrency: 2"));
}
