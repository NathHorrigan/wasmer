// !!! THIS IS A GENERATED FILE !!!
// ANY MANUAL EDITS MAY BE OVERWRITTEN AT ANY TIME
// Files autogenerated with cargo build (build/wasitests.rs).

#[test]
fn test_path_link() {
    assert_wasi_output!(
        "../../wasitests/path_link.wasm",
        "path_link",
        vec![],
        vec![
            (
                "act5".to_string(),
                ::std::path::PathBuf::from("wasitests/test_fs/hamlet/act5")
            ),
            (
                "temp".to_string(),
                ::std::path::PathBuf::from("wasitests/test_fs/temp")
            ),
        ],
        vec![],
        "../../wasitests/path_link.out"
    );
}
