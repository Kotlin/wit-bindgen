use std::path::Path;
use std::process::Command;
use heck::ToUpperCamelCase;

macro_rules! codegen_test {
    // Mutes:
    // Support for multiple WIT packages (also with the same name and different versions)
    (multiversion $name:tt $test:tt) => {};
    (variants_unioning_types $name:tt $test:tt) => {};
    ($id:ident $name:tt $test:tt) => {
        #[test]
        fn $id() {
            test_helpers::run_world_codegen_test(
                "guest-kotlin",
                $test.as_ref(),
                |resolve, world, files| {
                    wit_bindgen_kotlin::Opts::default()
                        .build()
                        .generate(resolve, world, files)
                        .unwrap()
                },
                verify,
            );
        }
    };
}

test_helpers::codegen_tests!();

fn verify(dir: &Path, name: &str) {
    let mut cmd = Command::new("kwac");
    let file_name = name.to_upper_camel_case();
    cmd.arg(format!("{file_name}.kt"));
    cmd.arg("build");
    cmd.arg(name);
    cmd.current_dir(dir);
    test_helpers::run_command(&mut cmd);
}