use anyhow::{bail, Context, Result};
use lexopt::{Arg, Parser, ValueExt};
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use wasmparser::Payload;

#[derive(Default)]
struct App {
    output: Option<PathBuf>,
    rsp_quoting: Option<String>,
    wasi_proxy_adapter: bool,
    wasm_ld_path: Option<PathBuf>,
    lld_args: Vec<OsString>,
    shared: bool,
}

fn main() {
    let err = match run() {
        Ok(()) => return,
        Err(e) => e,
    };
    eprintln!("error: {err}");
    if err.chain().len() > 1 {
        eprintln!("\nCaused by:");
        for (i, err) in err.chain().skip(1).enumerate() {
            eprintln!("{i:>5}: {}", err.to_string().replace("\n", "\n       "));
        }
    }

    std::process::exit(1);
}

fn run() -> Result<()> {
    let mut args = env::args_os().collect::<Vec<_>>();
    if let Some([flavor, wasm]) = args.get(1..3) {
        if flavor == "-flavor" && wasm == "wasm" {
            args.remove(1);
            args.remove(1);
        }
    }

    let mut app = App::default();
    let mut parser = Parser::from_iter(args);
    loop {
        if let Some(mut args) = parser.try_raw_args() {
            if let Some(arg) = args.peek() {
                if arg == "-shared" {
                    app.lld_args.push(arg.to_owned());
                    app.shared = true;
                    args.next();
                    continue;
                }
            }
        }
        match parser.next()? {
            Some(Arg::Long(
                s @ ("no-entry" | "no-demangle" | "allow-undefined" | "stack-first" | "gc-sections"
                | "whole-archive" | "no-whole-archive" | "fatal-warnings"),
            )) => {
                app.lld_args.push(format!("--{s}").into());
            }
            Some(Arg::Long("rsp-quoting")) => app.rsp_quoting = Some(parser.value()?.parse()?),
            Some(Arg::Long("wasm-ld-path")) => app.wasm_ld_path = Some(parser.value()?.into()),
            Some(Arg::Long(s @ ("export" | "entry"))) => {
                app.lld_args.push(format!("--{s}").into());
                app.lld_args.push(parser.value()?);
            }
            Some(Arg::Short(c @ ('L' | 'z' | 'l' | 'O' | 'm'))) => {
                app.lld_args.push(format!("-{c}").into());
                app.lld_args.push(parser.value()?);
            }
            Some(Arg::Short('o')) => app.output = Some(parser.value()?.into()),
            Some(Arg::Value(obj)) => app.lld_args.push(obj),
            Some(other) => bail!(other.unexpected()),
            None => break,
        }
    }
    if app.output.is_none() {
        bail!("must specify an output path via `-o`");
    }
    app.run()
}

impl App {
    fn run(&mut self) -> Result<()> {
        let mut cmd = self.lld();
        let linker = cmd.get_program().to_owned();

        let lld_output =
            tempfile::NamedTempFile::new().context("failed to create temp output file")?;

        // Shared libraries don't get wit-component run below so place the
        // output directly at the desired output location. Otherwise output to a
        // temporary location for wit-component to read and then the real output
        // is created after wit-component runs.
        if self.shared {
            cmd.arg("-o").arg(self.output.as_ref().unwrap());
        } else {
            cmd.arg("-o").arg(lld_output.path());
        }

        let status = cmd
            .status()
            .with_context(|| format!("failed to spawn {linker:?}"))?;
        if !status.success() {
            bail!("failed to invoke LLD: {status}");
        }

        // Skip componentization with `--shared` since that's creating a shared
        // library that's not a component yet.
        if self.shared {
            return Ok(());
        }

        let reactor_adapter = include_bytes!("wasi_snapshot_preview1.reactor.wasm");
        let command_adapter = include_bytes!("wasi_snapshot_preview1.command.wasm");
        let proxy_adapter = include_bytes!("wasi_snapshot_preview1.proxy.wasm");
        let core_module = std::fs::read(lld_output.path())
            .with_context(|| format!("failed to read {linker:?} output"))?;

        // Inspect the output module to see if it's a command or reactor.
        let mut exports_start = false;
        for payload in wasmparser::Parser::new(0).parse_all(&core_module) {
            match payload {
                Ok(Payload::ExportSection(e)) => {
                    for export in e {
                        if let Ok(e) = export {
                            if e.name == "_start" {
                                exports_start = true;
                                break;
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        let adapter = if exports_start {
            &command_adapter[..]
        } else if self.wasi_proxy_adapter {
            &proxy_adapter[..]
        } else {
            &reactor_adapter[..]
        };

        let component = wit_component::ComponentEncoder::default()
            .module(&core_module)
            .context("failed to parse core wasm for componentization")?
            .adapter("wasi_snapshot_preview1", adapter)
            .context("failed to inject adapter")?
            .encode()
            .context("failed to encode component")?;

        std::fs::write(self.output.as_ref().unwrap(), &component)
            .context("failed to write output file")?;

        Ok(())
    }

    fn lld(&self) -> Command {
        let mut lld = self.find_lld();
        lld.args(&self.lld_args);
        lld
    }

    fn find_lld(&self) -> Command {
        if let Some(path) = &self.wasm_ld_path {
            return Command::new(path);
        }

        // Search for the first of `wasm-ld` or `rust-lld` in `$PATH`
        let wasm_ld = format!("wasm-ld{}", env::consts::EXE_SUFFIX);
        let rust_lld = format!("rust-lld{}", env::consts::EXE_SUFFIX);
        for entry in env::split_paths(&env::var_os("PATH").unwrap_or_default()) {
            if entry.join(&wasm_ld).is_file() {
                return Command::new(wasm_ld);
            }
            if entry.join(&rust_lld).is_file() {
                let mut ret = Command::new(rust_lld);
                ret.arg("-flavor").arg("wasm");
                return ret;
            }
        }

        // Fall back to `wasm-ld` if the search failed to get an error message
        // that indicates that `wasm-ld` was attempted to be found but couldn't
        // be found.
        Command::new("wasm-ld")
    }
}
