use std::process::{Command, Stdio};

#[test]
fn invalid_sim_path_exits_nonzero_in_plain_mode() {
    let result = Command::new(env!("CARGO_BIN_EXE_cpu-cache-chat"))
        .args([
            "chat",
            "--role",
            "b",
            "--channel",
            "cpu",
            "--sim",
            "--sim-path",
            "/no/such/directory/session",
            "--plain",
        ])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(!result.status.success());
}
