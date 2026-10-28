use std::process::{Command, Stdio};

#[test]
fn simulation_requires_shared_path_and_agent_command_requires_agent() {
    for (args, expected) in [
        (
            vec![
                "chat",
                "--role",
                "a",
                "--channel",
                "cpu",
                "--sim",
                "--plain",
            ],
            "--sim-path",
        ),
        (
            vec![
                "game",
                "--role",
                "a",
                "--channel",
                "cpu",
                "--sim",
                "--sim-path",
                "/tmp/test",
                "--agent-cmd",
                "echo",
                "--plain",
            ],
            "--agent-cmd requires --agent",
        ),
        (
            vec![
                "game",
                "--role",
                "a",
                "--channel",
                "cpu",
                "--sim",
                "--sim-path",
                "/tmp/test",
                "--events-stdout",
            ],
            "--events-stdout requires --plain",
        ),
        (
            vec![
                "chat",
                "--role",
                "a",
                "--channel",
                "cpu",
                "--sim",
                "--sim-path",
                "/tmp/test",
                "--plain",
                "--events-stdout",
            ],
            "--events-stdout is only valid for game mode",
        ),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_cpu-cache-chat"))
            .args(args)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains(expected));
    }
}

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
