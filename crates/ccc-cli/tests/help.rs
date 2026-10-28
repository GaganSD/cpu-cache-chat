use std::process::Command;

#[test]
fn version_identifies_wire_compatibility() {
    let output = Command::new(env!("CARGO_BIN_EXE_cpu-cache-chat"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(output.status.success());
    let version = String::from_utf8(output.stdout).unwrap();
    assert!(version.contains(&format!("protocol={}", ccc_core::PROTOCOL_VERSION)));
    assert!(version.contains("source="));
}

#[test]
fn help_lists_commands_and_plain_option() {
    let binary = env!("CARGO_BIN_EXE_cpu-cache-chat");
    let output = Command::new(binary).arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).unwrap();
    for command in ["chat", "game", "probe", "inspect-isolation"] {
        assert!(help.contains(command));
    }
    let output = Command::new(binary)
        .args(["chat", "--help"])
        .output()
        .unwrap();
    assert!(String::from_utf8(output.stdout)
        .unwrap()
        .contains("--plain"));
}
