#[test]
fn recorded_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    // trycmd 1.2.1 has no `TestCases::cwd`, so the working directory must be
    // changed globally. This is the only test in this binary, so the change
    // cannot race with another test.
    std::env::set_current_dir(&root).unwrap();
    trycmd::TestCases::new()
        .register_bin(
            "run-original",
            std::path::Path::new(env!("CARGO_BIN_EXE_termux-sync-rs")),
        )
        .case(root.join("app-cases/*.toml"));
}
