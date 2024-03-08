use anyhow::{bail, Context, Result};
use clap::Parser;
use std::env;
use std::path::PathBuf;
use std::process::Command;
use wasmparser::Payload;

#[derive(Parser)]
struct App {
    #[clap(flatten)]
    lld: WasmLdArgs,

    #[clap(short = 'o')]
    output: PathBuf,

    #[clap(long)]
    rsp_quoting: Option<String>,

    #[clap(long)]
    wasi_proxy_adapter: bool,
}

#[derive(Parser)]
struct WasmLdArgs {
    #[clap(long)]
    export: Vec<String>,
    #[clap(short = 'z')]
    z_opts: Vec<String>,
    #[clap(long)]
    stack_first: bool,
    #[clap(long)]
    allow_undefined: bool,
    #[clap(long)]
    fatal_warnings: bool,
    #[clap(long)]
    no_demangle: bool,
    #[clap(long)]
    gc_sections: bool,
    #[clap(short = 'O')]
    optimize: Option<u32>,
    #[clap(short = 'L')]
    link_path: Vec<PathBuf>,
    #[clap(short = 'l')]
    libraries: Vec<PathBuf>,
    #[clap(long)]
    no_entry: bool,
    #[clap(short = 'm')]
    target_emulation: Option<String>,
    #[clap(long)]
    strip_all: bool,

    objects: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let mut args = env::args_os().collect::<Vec<_>>();
    if let Some([flavor, wasm]) = args.get(1..3) {
        if flavor == "-flavor" && wasm == "wasm" {
            args.remove(1);
            args.remove(1);
        }
    }
    App::parse_from(args).run()
}

impl App {
    fn run(&mut self) -> Result<()> {
        let mut cmd = self.lld();
        let linker = cmd.get_program().to_owned();

        let lld_output =
            tempfile::NamedTempFile::new().context("failed to create temp output file")?;
        cmd.arg("-o").arg(lld_output.path());

        let status = cmd
            .status()
            .with_context(|| format!("failed to spawn {linker:?}"))?;
        if !status.success() {
            bail!("failed to invoke LLD: {status}");
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

        std::fs::write(&self.output, &component).context("failed to write output file")?;

        Ok(())
    }

    fn lld(&self) -> Command {
        let mut lld = self.find_lld();
        for export in self.lld.export.iter() {
            lld.arg("--export").arg(export);
        }
        for opt in self.lld.z_opts.iter() {
            lld.arg("-z").arg(opt);
        }
        if self.lld.stack_first {
            lld.arg("--stack-first");
        }
        if self.lld.allow_undefined {
            lld.arg("--allow-undefined");
        }
        if self.lld.fatal_warnings {
            lld.arg("--fatal-warnings");
        }
        if self.lld.no_demangle {
            lld.arg("--no-demangle");
        }
        if self.lld.gc_sections {
            lld.arg("--gc-sections");
        }
        if self.lld.no_entry {
            lld.arg("--no-entry");
        }
        if let Some(opt) = self.lld.optimize {
            lld.arg(&format!("-O{opt}"));
        }
        for path in self.lld.link_path.iter() {
            lld.arg("-L").arg(path);
        }
        for obj in self.lld.objects.iter() {
            lld.arg(obj);
        }
        for lib in self.lld.libraries.iter() {
            lld.arg("-l").arg(lib);
        }
        if let Some(arg) = &self.lld.target_emulation {
            lld.arg("-m").arg(arg);
        }
        if self.lld.strip_all {
            lld.arg("--strip-all");
        }
        lld
    }

    fn find_lld(&self) -> Command {
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
