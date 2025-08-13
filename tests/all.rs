use anyhow::{bail, Context, Result};
use std::env;
use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

fn compile(args: &[&str], src: &str) -> Vec<u8> {
    Project::new().compile(args, src)
}

struct Project {
    tempdir: tempfile::TempDir,
}

impl Project {
    fn new() -> Project {
        Project {
            tempdir: tempfile::TempDir::new().unwrap(),
        }
    }
    fn file(&self, name: &str, contents: &str) {
        fs::write(self.tempdir.path().join(name), contents.as_bytes()).unwrap();
    }

    fn compile(&self, args: &[&str], src: &str) -> Vec<u8> {
        self.try_compile(args, src, true).unwrap()
    }

    fn try_compile(&self, args: &[&str], src: &str, inherit_stderr: bool) -> Result<Vec<u8>> {
        let mut myself = env::current_exe().unwrap();
        myself.pop(); // exe name
        myself.pop(); // 'deps'
        myself.push("wasm-component-ld");
        let mut rustc = Command::new("rustc");
        rustc
            .arg("--target")
            .arg("wasm32-wasip1")
            .arg("-")
            .arg("-o")
            .arg("-")
            .arg("-C")
            .arg(&format!("linker={}", myself.to_str().unwrap()))
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .current_dir(self.tempdir.path());
        if !inherit_stderr {
            rustc.stderr(Stdio::piped());
        }
        let mut rustc = rustc.spawn().context("failed to spawn rustc")?;

        rustc
            .stdin
            .take()
            .context("stdin should be present")?
            .write_all(src.as_bytes())
            .context("failed to write stdin")?;
        let output = rustc
            .wait_with_output()
            .context("failed to wait for subprocess")?;
        if !output.status.success() {
            let mut error = format!("subprocess failed: {:?}", output.status);
            if !output.stdout.is_empty() {
                error.push_str(&format!(
                    "\nstdout: {}",
                    String::from_utf8_lossy(&output.stdout)
                ));
            }
            if !output.stderr.is_empty() {
                error.push_str(&format!(
                    "\nstderr: {}",
                    String::from_utf8_lossy(&output.stderr)
                ));
            }
            bail!("{error}")
        }
        Ok(output.stdout)
    }
}

fn assert_component(bytes: &[u8]) {
    assert!(wasmparser::Parser::is_component(&bytes));
    wasmparser::Validator::new().validate_all(&bytes).unwrap();
}

fn assert_module(bytes: &[u8]) {
    assert!(wasmparser::Parser::is_core_wasm(&bytes));
    wasmparser::Validator::new().validate_all(&bytes).unwrap();
}

#[test]
fn empty() {
    let output = compile(&["--crate-type", "cdylib"], "");
    assert_component(&output);
}

#[test]
fn empty_main() {
    let output = compile(&[], "fn main() {}");
    assert_component(&output);
}

#[test]
fn hello_world() {
    let output = compile(
        &[],
        r#"
fn main() {
    println!("hello!");
}
"#,
    );
    assert_component(&output);
}

#[test]
fn cdylib_arbitrary_export() {
    let output = compile(
        &["--crate-type", "cdylib"],
        r#"
#[no_mangle]
pub extern "C" fn foo() {
    println!("x");
}
        "#,
    );
    assert_component(&output);
}

#[test]
fn can_access_badfd() {
    let output = compile(
        &[],
        r#"
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    fn adapter_open_badfd(fd: &mut u32) -> u32;
}

fn main() {
    let mut fd = 0;
    let rc = unsafe {
        adapter_open_badfd(&mut fd)
    };
    assert_eq!(rc, 0);
    assert_eq!(fd, 3);
}
        "#,
    );
    assert_component(&output);
}

#[test]
fn linker_flags() {
    let output = compile(
        &[
            "-Clink-arg=--max-memory=65536",
            "-Clink-arg=-zstack-size=32",
            "-Clink-arg=--global-base=2048",
            "-Clink-arg=--append-lld-flag=--no-merge-data-segments",
        ],
        r#"
fn main() {
}
        "#,
    );
    assert_component(&output);
}

#[test]
fn invalid_lld_flag() {
    let result = Project::new().try_compile(
        &["-Clink-arg=--append-lld-flag=--nonexistent-lld-flag"],
        r#"
fn main() {
    println!("hello!");
}
"#,
        false,
    );
    let err = result.unwrap_err();
    let err = format!("{err:?}");
    println!("error: {err}");
    assert!(err.contains("error: unknown argument: --nonexistent-lld-flag"));
}

#[test]
fn component_type_wit_file() {
    let project = Project::new();
    project.file(
        "foo.wit",
        r#"
package foo:bar;

interface foo {
  bar: func(s: string) -> string;
}

world root {
  import foo;
  export foo;
}
"#,
    );
    let output = project.compile(
        &[
            "-Clink-arg=--component-type",
            "-Clink-arg=foo.wit",
            "-Clink-arg=--string-encoding",
            "-Clink-arg=utf16",
            "--crate-type",
            "cdylib",
        ],
        r#"
#[no_mangle]
pub extern "C" fn cabi_realloc(ptr: *mut u8, old_size: i32, align: i32, new_size: i32) -> *mut u8 {
    _ = (ptr, old_size, align, new_size);
    unreachable!()
}

#[link(wasm_import_module = "foo:bar/foo")]
extern "C" {
    #[link_name = "bar"]
    fn import(ptr: *mut u8, len: i32, return_ptr: *mut *mut u8);
}

#[export_name = "foo:bar/foo#bar"]
pub unsafe extern "C" fn export(ptr: *mut u8, len: i32) -> *mut u8 {
    let mut result = std::ptr::null_mut();
    import(ptr, len, &mut result);
    result
}
"#,
    );
    assert_component(&output);
}

#[test]
fn skip_component() {
    let output = compile(
        &["-Clink-arg=--skip-wit-component"],
        r#"
fn main() {
}
        "#,
    );
    assert_module(&output);
}

#[test]
fn rustc_using_argfile() {
    let prefix = (0..200).map(|_| 'a').collect::<String>();
    let p = Project {
        tempdir: tempfile::TempDir::with_prefix(&prefix).unwrap(),
    };

    let mut src = String::new();
    for i in 0..1000 {
        src.push_str(&format!("mod m{i};\n"));
        p.file(
            &format!("m{i}.rs"),
            &format!("#[no_mangle] pub extern \"C\" fn f{i}() {{}}"),
        );
    }
    src.push_str("fn main() {}");

    p.file("args", "@args2");
    p.file("args2", "--skip-wit-component");
    let output = p.compile(&["-Ccodegen-units=1000", "-Clink-arg=@args"], &src);
    assert_module(&output);
}

#[test]
fn hello_world_with_realloc_as_memory_grow() {
    let output = compile(
        &["-Clink-arg=--realloc-via-memory-grow"],
        r#"
fn main() {
    println!("hello!");
}
"#,
    );
    assert_component(&output);
}

// The adapter uses legacy names, so this option will always result in an error
// right now.
#[test]
fn legacy_names_currently_required() {
    let result = Project::new().try_compile(
        &["-Clink-arg=--reject-legacy-names"],
        r#"
fn main() {
    println!("hello!");
}
"#,
        false,
    );
    let err = result.unwrap_err();
    let err = format!("{err:?}");
    println!("error: {err}");
    assert!(err.contains("unknown or invalid component model import syntax"));
}
