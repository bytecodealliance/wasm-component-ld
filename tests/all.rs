use std::env;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

fn compile(args: &[&str], src: &str) -> Vec<u8> {
    let tempdir = tempfile::TempDir::new().unwrap();
    let mut myself = env::current_exe().unwrap();
    myself.pop(); // exe name
    myself.pop(); // 'deps'
    myself.push("wasm-component-ld");
    let mut rustc = Command::new("rustc")
        .arg("--target")
        .arg("wasm32-wasip1")
        .arg("-")
        .arg("-o")
        .arg("-")
        .arg("-C")
        .arg(&format!("linker={}", myself.to_str().unwrap()))
        .args(args)
        .current_dir(tempdir.path())
        .stdout(Stdio::piped())
        .stdin(Stdio::piped())
        .spawn()
        .expect("failed to spawn rustc");

    rustc
        .stdin
        .take()
        .unwrap()
        .write_all(src.as_bytes())
        .unwrap();
    let mut ret = Vec::new();
    rustc.stdout.take().unwrap().read_to_end(&mut ret).unwrap();
    assert!(rustc.wait().unwrap().success());
    ret
}

fn assert_component(bytes: &[u8]) {
    assert!(wasmparser::Parser::is_component(&bytes));
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
        ],
        r#"
fn main() {
}
        "#,
    );
    assert_component(&output);
}
