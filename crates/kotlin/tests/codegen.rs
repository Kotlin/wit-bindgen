use std::path::Path;
use std::process::Command;

macro_rules! codegen_test {
    // Mutes:
    // Support for multiple WIT packages (also with the same name and different versions)
    (multiversion $name:tt $test:tt) => {};
    (variants_unioning_types $name:tt $test:tt) => {};

    // not implemented: resource imports from worlds
    (resource_borrow_in_record $name:tt $test:tt) => {};
    (return_resource_from_export $name:tt $test:tt) => {};

    ($id:ident $name:tt $test:tt) => {
        #[test]
        fn $id() {
            test_helpers::run_world_codegen_test(
                "guest-kotlin",
                $test.as_ref(),
                |resolve, world, files| {
                    wit_bindgen_kotlin::Opts { generate_stubs: true }
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
    let mut ktfmt = Command::new("java");
    ktfmt.arg("-jar");
    ktfmt.arg("../../../ktfmt-0.47-jar-with-dependencies.jar");
    ktfmt.arg(dir.file_name().unwrap());
    ktfmt.current_dir(dir.parent().unwrap());
    test_helpers::run_command(&mut ktfmt);

    let mut cmd = Command::new("kwac");
    cmd.arg(dir.file_name().unwrap());
    cmd.arg("build");
    cmd.arg(name);
    cmd.current_dir(dir.parent().unwrap());
    test_helpers::run_command(&mut cmd);
}