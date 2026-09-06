#[test]
fn version_is_available_without_opening_a_window() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_remcmd"))
        .arg("--version")
        .output()
        .expect("version command should run");
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap().trim(),
        concat!("RemCmd ", env!("CARGO_PKG_VERSION"))
    );
}
