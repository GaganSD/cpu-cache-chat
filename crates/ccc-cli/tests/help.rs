use std::process::Command;

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
