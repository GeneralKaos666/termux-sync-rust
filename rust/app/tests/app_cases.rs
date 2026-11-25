#[test]
fn recorded_cases() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::env::set_current_dir(&root).unwrap();
    trycmd::TestCases::new()
        .register_bin(
            "run-original",
            std::path::Path::new(env!("CARGO_BIN_EXE_termux-sync-rs")),
        )
        .case(root.join("rust/app-cases/*.toml"));
}
