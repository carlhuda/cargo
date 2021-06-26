use cargo_test_support::project;

#[cargo_test]
fn diff_bin_name() {
    // create the project
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
        .file( "src/main.rs", "fn main() { assert!(true) }" )
        .build();
    
    // cargo build
    p.cargo("build")
        .run();

    // check which files were created
    
    // a file with name foo should not be created
    let foo_path = p.bin("foo");
    assert!(foo_path.is_file());
    // a file with name bar should be created
    let bar_path = p.bin("bar");
    assert!(bar_path.is_file());
}
