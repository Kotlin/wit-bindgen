use crate::{LanguageMethods, Runner, Verify};
use anyhow::{bail, Result};
use std::process::Command;

pub struct Kotlin;

impl LanguageMethods for Kotlin {
    fn display(&self) -> &str {
        "kotlin"
    }

    fn comment_prefix_for_test_config(&self) -> Option<&str> {
        Some("//@")
    }

    fn prepare(&self, runner: &mut Runner) -> Result<()> {
        println!("Testing if ktfmt is available...");
        let test_crate = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let wit_bindgen_root = test_crate.parent().unwrap().parent().unwrap();
        let ktfmt_jar = wit_bindgen_root.join("ktfmt-0.47-jar-with-dependencies.jar");
        if !ktfmt_jar.exists() {
            bail!(
                "ktfmt jar not found at `{}`",
                ktfmt_jar.display()
            );
        }
        runner.run_command(Command::new("java").arg("-version"))?;
        Ok(())
    }

    fn default_bindgen_args_for_codegen(&self) -> &[&str] {
        &["--generate-stubs"]
    }

    fn compile(&self, _runner: &Runner, _compile: &crate::Compile) -> Result<()> {
        bail!("compiling Kotlin to a wasm component is not yet supported")
    }

    fn should_fail_verify(
        &self,
        name: &str,
        config: &crate::config::WitConfig,
        _args: &[String],
    ) -> bool {
        config.error_context
            // TODO: fix these codegen failures, and organize them better (e.g. add as a failure condition config.async_ here, once we have real verification)
            || matches!(
                name,
                "future-same-type-different-names.wit"
                    | "futures.wit"
                    | "import-and-export-resource-alias.wit"
                    | "import-and-export-resource.wit"
                    | "import-export-future.wit"
                    | "import-export-stream.wit"
                    | "issue-1432.wit"
                    | "issue-1544.wit"
                    | "issue1515-special-in-comment.wit"
                    | "issue573.wit"
                    | "issue929-only-methods.wit"
                    | "issue929.wit"
                    | "keywords-in-interfaces-and-worlds.wit"
                    | "named-fixed-length-list.wit"
                    | "rename-interface.wit"
                    | "resource-alias.wit"
                    | "resource-borrow-in-record.wit"
                    | "resource-fallible-constructor.wit"
                    | "resource-local-alias.wit"
                    | "resource-own-in-other-interface.wit"
                    | "resources-in-aggregates.wit"
                    | "resources-with-futures.wit"
                    | "resources-with-lists.wit"
                    | "resources-with-streams.wit"
                    | "resources.wit"
                    | "return-resource-from-export.wit"
                    | "smoke-export.wit"
                    | "smoke.wit"
                    | "streams.wit"
                    | "unused-import.wit"
                    | "use-across-interfaces.wit"
            )
    }

    fn verify(&self, runner: &Runner, verify: &Verify) -> Result<()> {
        let test_crate = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let wit_bindgen_root = test_crate.parent().unwrap().parent().unwrap();
        let ktfmt_jar = wit_bindgen_root.join("ktfmt-0.47-jar-with-dependencies.jar");

        let mut cmd = Command::new("java");
        cmd.arg("-jar")
            .arg(&ktfmt_jar)
            .arg(verify.bindings_dir.file_name().unwrap())
            .current_dir(verify.bindings_dir.parent().unwrap());
        runner.run_command(&mut cmd)

        // TODO actually compile the bindings to verify compilation
    }
}
