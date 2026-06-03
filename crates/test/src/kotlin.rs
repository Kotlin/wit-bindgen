use crate::{LanguageMethods, Runner, Verify};
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::process::{Command, ExitStatus};
use std::sync::{Mutex, OnceLock};
use std::{env, fs};

// auto pull kotlin compiler binary to test compilation

pub const KOTLIN_VERSION: &str = "2.4.0-RC";
// pub const KOTLIN_ZIP_SHA: &str = "5c3699980e09a65328d56a16aa8896ba0a421ce97865ca287c623897bf20a98e";

fn simple_cmd_wrapper(dir: &PathBuf, full_cmd: &str) -> ExitStatus {
    let mut cmd = Command::new("/usr/bin/bash");
    let cmd = cmd.current_dir(dir).arg("-c").arg(full_cmd);

    cmd.status().expect("Failed to execute process")
}

static COMPILED_MODULE_NUM: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Debug)]
struct KotlincWasm {
    pub path_to_tmpdir: PathBuf,
    pub path_to_dist: PathBuf,
}

impl KotlincWasm {
    const MODULE_BASE_NAME: &str = "main";

    fn compile(&self, unique_test_str: &str, files: &[PathBuf]) -> Result<()> {
        let module_name = format!("{}-{}", Self::MODULE_BASE_NAME, unique_test_str);
        let path_to_kotlinc = self
            .path_to_dist
            .join("bin/kotlinc-wasm")
            .to_str()
            .unwrap()
            .to_string();
        let path_to_outdir = self
            .path_to_tmpdir
            .join("out")
            .to_str()
            .unwrap()
            .to_string();
        let path_to_stdlib = self
            .path_to_dist
            .join("lib/kotlin-stdlib-wasm-wasi.klib")
            .to_str()
            .unwrap()
            .to_string();

        let files_as_str = files
            .iter()
            .map(|p| p.to_str().unwrap())
            .collect::<Vec<_>>()
            .join(" ");

        // stage 1: to klib
        if !simple_cmd_wrapper(
            &self.path_to_tmpdir,
            format!(
                "{path_to_kotlinc} \
            -Xwasm-target=wasm-wasi \
            -Xir-produce-klib-file \
            -ir-output-dir {path_to_outdir} \
            -ir-output-name {module_name} \
            -libraries {path_to_stdlib} \
            {}
            ",
                files_as_str.as_str()
            )
            .as_str(),
        )
        .success()
        {
            bail!("Stage 1 of compilation failed");
        }

        // stage 2: to binary
        if !simple_cmd_wrapper(
            &self.path_to_tmpdir,
            format!(
                "{path_to_kotlinc} \
            -Xwasm-target=wasm-wasi \
            -Xir-produce-js \
            -Xinclude={path_to_outdir}/{module_name}.klib \
            -ir-output-dir {path_to_outdir} \
            -ir-output-name {module_name} \
            -libraries {path_to_stdlib} \
            {}
            ",
                files_as_str.as_str()
            )
            .as_str(),
        )
        .success()
        {
            bail!("Stage 2 of compilation failed");
        }

        Ok(())
    }
}
// intentionally not dropped right now, because we want to reuse it anyway
// impl Drop for KotlincWasm{
//     fn drop(&mut self) {
//         fs::remove_dir_all(&self.path_to_tmpdir).expect("Failed to remove temp dir")
//     }
// }

static KOTLINC_DOWNLOAD: OnceLock<KotlincWasm> = OnceLock::new();
fn reuse_or_download_kotlinc_wasm() -> Result<KotlincWasm> {
    let path_to_tmpdir = env::temp_dir().join(format!("kotlinc-wasm-v{KOTLIN_VERSION}"));
    let expected_dist_path = path_to_tmpdir.join("kotlinc");

    if expected_dist_path.exists() {
        // previous run on this machine has downloaded it already
        return Ok(KotlincWasm {
            path_to_tmpdir: path_to_tmpdir.clone(),
            path_to_dist: path_to_tmpdir.clone().join("kotlinc"),
        });
    }

    // doesn't exist yet, so create and download
    fs::create_dir_all(&path_to_tmpdir)?;

    return download_and_extract_kotlinc_wasm(path_to_tmpdir);
}

/// Bit makeshift right now, fix once we can actually access the wasm-wasi stdlib in the dist
fn download_and_extract_kotlinc_wasm(path_to_tmpdir: PathBuf) -> Result<KotlincWasm> {
    if !simple_cmd_wrapper(&path_to_tmpdir, format!("curl -L -O https://github.com/JetBrains/kotlin/releases/download/v{KOTLIN_VERSION}/kotlin-compiler-{KOTLIN_VERSION}.zip").as_str()).success() {
        bail!("Failed to download kotlin compiler release");
    }

    if !simple_cmd_wrapper(
        &path_to_tmpdir,
        format!("unzip kotlin-compiler-{KOTLIN_VERSION}.zip").as_str(),
    )
    .success()
    {
        bail!("Failed to extract kotlin compiler release");
    }

    // TODO remove this in the future
    const WASM_WASI_STDLIB_KLIB_VERSION: &str = "2.4.20-dev-5102";
    // add in the wasm-wasi stdlib, because the RC isnt new enough to have it yet
    if !simple_cmd_wrapper(&path_to_tmpdir, format!("curl -L \"https://packages.jetbrains.team/maven/p/kt/dev/org/jetbrains/kotlin/kotlin-stdlib-wasm-wasi/{WASM_WASI_STDLIB_KLIB_VERSION}/kotlin-stdlib-wasm-wasi-{WASM_WASI_STDLIB_KLIB_VERSION}.klib\" -o kotlinc/lib/kotlin-stdlib-wasm-wasi.klib").as_str()).success() {
        bail!("Failed to download kotlin wasm-wasi stdlib");
    }

    Ok(KotlincWasm {
        path_to_dist: path_to_tmpdir.join("kotlinc"),
        path_to_tmpdir,
    })
}

pub struct Kotlin;

impl LanguageMethods for Kotlin {
    fn display(&self) -> &str {
        "kotlin"
    }

    fn comment_prefix_for_test_config(&self) -> Option<&str> {
        Some("//@")
    }

    fn prepare(&self, runner: &mut Runner) -> Result<()> {
        KOTLINC_DOWNLOAD
            .set(reuse_or_download_kotlinc_wasm()?)
            .expect("KOTLINC_WASM was mistakenly initialized already");
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
        if config.error_context {
            return true;
        }

        if config.async_
            // Except these actually do work:
            && !matches!(name,
                "async-trait-function.wit" |
                "async-resource-func.wit" |
                "issue-1433.wit"
            )
        {
            return true;
        }

        // TODO: these should also be fixed, but depend on unimplemented features, which is less critical
        if matches!(name, "map.wit") {
            return true;
        }

        // TODO: fix these codegen failures
        matches!(
            name,
            "resource-alias.wit"
                | "import-and-export-resource-alias.wit"
                | "resources-in-aggregates.wit"
                | "issue929-only-methods.wit"
                | "resource-local-alias.wit"
                | "resources-with-lists.wit"
                | "resource-fallible-constructor.wit"
                | "import-and-export-resource.wit"
                | "issue1515-special-in-comment.wit"
                | "issue929.wit"
                | "named-fixed-length-list.wit"
                | "issue-1433.wit"
        )
    }

    // TODO probably use runner, e.g. for run_command?

    fn verify(&self, runner: &Runner, verify: &Verify) -> Result<()> {
        let test_crate = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

        // KOTLINC_DOWNLOAD is initialized in prepare()
        let kotlinc_wasm = KOTLINC_DOWNLOAD.get().unwrap();

        // first get the files without a fixed name
        let mut files = verify
            .bindings_dir
            .read_dir()?
            .filter_map(|entry| entry.ok())
            .filter_map(|entry| {
                entry
                    .path()
                    .file_name()
                    .and_then(|name| name.to_str())
                    .map(String::from)
            })
            .filter(|name| name.ends_with(".kt"))
            .map(|name| verify.bindings_dir.join(name))
            .collect::<Vec<_>>();

        // then add component support (its not on the same flat level as the other files)
        files.push(
            files
                .first()
                .unwrap()
                .parent()
                .unwrap()
                .join("runtime")
                .join("ComponentSupport.kt")
                .to_path_buf(),
        );

        // TODO for now I'm simply assuming the test names are unique
        kotlinc_wasm.compile(
            verify.wit_test.file_stem().unwrap().to_str().unwrap(),
            &*files,
        )
    }
}
