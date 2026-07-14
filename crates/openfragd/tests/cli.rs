use std::process::Command;

#[test]
fn version_command_identifies_the_v1_daemon() {
    let output = Command::new(env!("CARGO_BIN_EXE_openfragd"))
        .arg("--version")
        .output()
        .expect("run openfragd");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).expect("UTF-8 version output"),
        "openfragd 1.0.0\n"
    );
    assert!(output.stderr.is_empty());
}
