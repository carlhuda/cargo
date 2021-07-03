use cargo_test_support::project;

#[cargo_test]
// This test checks if Cargo produces a binary with the name we provide in `Cargo.toml` in the
// `filename` parameter under the `[[bin]]` section.
fn diff_bin_name() {
    // Create the project.
    let p = project()
        .file(
            "Cargo.toml",
            r#"
                [project]
                name =  "foo"
                version = "0.0.1"
                authors = []

                [[bin]]
                name = "foo"
                filename = "bar"
                path = "src/main.rs"
            "#,
        )
        .file("src/main.rs", "fn main() { assert!(true) }")
        .build();

    // Run cargo build.
    p.cargo("build").run();

    // Check the name of the binary that cargo has generated.
    // A binary with the name of the crate should NOT be created.
    let foo_path = p.bin("foo");
    assert!(!foo_path.is_file());
    // A binary with the name provided in `filename` parameter should be created.
    let bar_path = p.bin("bar");
    assert!(bar_path.is_file());
}
