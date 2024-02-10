use anyhow::{bail, Context, Result};
use clap::Parser;
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

    objects: Vec<PathBuf>,
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().collect::<Vec<_>>();
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

        let lld_output =
            tempfile::NamedTempFile::new().context("failed to create temp output file")?;
        cmd.arg("-o").arg(lld_output.path());
        let status = cmd.status().context("failed to spawn `rust-lld`")?;
        if !status.success() {
            bail!("failed to invoke LLD: {status}");
        }

        let reactor_adapter = include_bytes!("wasi_snapshot_preview1.reactor.wasm");
        let command_adapter = include_bytes!("wasi_snapshot_preview1.command.wasm");
        let proxy_adapter = include_bytes!("wasi_snapshot_preview1.proxy.wasm");
        let core_module =
            std::fs::read(lld_output.path()).context("failed to read `rust-lld` output")?;

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
        let mut lld = Command::new("rust-lld");
        lld.arg("-flavor").arg("wasm");
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
        lld
    }
}
