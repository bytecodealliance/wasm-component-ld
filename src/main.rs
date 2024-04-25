use anyhow::{bail, Context, Result};
use clap::{ArgAction, CommandFactory, FromArgMatches};
use lexopt::Arg;
use std::env;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use wasmparser::Payload;

struct LldFlag {
    clap_name: &'static str,
    long: Option<&'static str>,
    short: Option<char>,
    value: Option<&'static str>,
}

const LLD_FLAGS: &[LldFlag] = &[
    LldFlag {
        clap_name: "no-entry",
        long: Some("no-entry"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "no-demangle",
        long: Some("no-demangle"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "allow-undefined",
        long: Some("allow-undefined"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "stack-first",
        long: Some("stack-first"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "gc-sections",
        long: Some("gc-sections"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "whole-archive",
        long: Some("whole-archive"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "no-whole-archive",
        long: Some("no-whole-archive"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "fatal-warnings",
        long: Some("fatal-warnings"),
        short: None,
        value: None,
    },
    LldFlag {
        clap_name: "export",
        long: Some("export"),
        short: None,
        value: Some("SYM"),
    },
    LldFlag {
        clap_name: "entry",
        long: Some("entry"),
        short: None,
        value: Some("SYM"),
    },
    LldFlag {
        clap_name: "lib",
        long: None,
        short: Some('l'),
        value: Some("LIB"),
    },
    LldFlag {
        clap_name: "link-path",
        long: None,
        short: Some('L'),
        value: Some("PATH"),
    },
    LldFlag {
        clap_name: "extra",
        long: None,
        short: Some('z'),
        value: Some("OPT"),
    },
    LldFlag {
        clap_name: "optimize",
        long: None,
        short: Some('O'),
        value: Some("LEVEL"),
    },
    LldFlag {
        clap_name: "arch",
        long: None,
        short: Some('m'),
        value: Some("ARCH"),
    },
    LldFlag {
        clap_name: "strip-debug",
        long: Some("strip-debug"),
        short: None,
        value: None,
    },
];

const LLD_LONG_FLAGS_NONSTANDARD: &[&str] = &["-shared"];

#[derive(Default)]
struct App {
    component: ComponentLdArgs,
    lld_args: Vec<OsString>,
    shared: bool,
}

/// A linker to create a Component from input object files and libraries.
///
/// This application is an equivalent of `wasm-ld` except that it produces a
/// component instead of a core wasm module. This application behaves very
/// similarly to `wasm-ld` in that it takes the same inputs and flags, and it
/// will internally invoke `wasm-ld`. After `wasm-ld` has been invoked the core
/// wasm module will be turned into a component using component tooling and
/// embedded information in the core wasm module.
#[derive(clap::Parser, Default)]
struct ComponentLdArgs {
    /// Instructs the "proxy" adapter to be used for use in a `wasi:http/proxy`
    /// world.
    #[clap(long)]
    wasi_proxy_adapter: bool,

    /// Location of where to find `wasm-ld`.
    ///
    /// If not specified this is automatically detected.
    #[clap(long)]
    wasm_ld_path: Option<PathBuf>,

    /// Quoting syntax for response files, forwarded to LLD.
    #[clap(long)]
    rsp_quoting: Option<String>,

    /// Where to place the component output.
    #[clap(short)]
    output: Option<PathBuf>,
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
    App::parse()?.run()
}

impl App {
    /// Parse the CLI arguments into an `App` to run the linker.
    ///
    /// This is unfortunately nontrivial because the way `wasm-ld` takes
    /// arguments is not compatible with `clap`. Namely flags like
    /// `--whole-archive` are positional are processed in a stateful manner.
    /// This means that the relative ordering of flags to `wasm-ld` needs to be
    /// preserved. Additionally there are flags like `-shared` which clap does
    /// not support.
    ///
    /// To handle this the `lexopt` crate is used to perform low-level argument
    /// parsing. That's then used to determine whether the argument is intended
    /// for `wasm-component-ld` or `wasm-ld`, so arguments are filtered into two
    /// lists. Using these lists the arguments to `wasm-component-ld` are then
    /// parsed. On failure a help message is presented with all `wasm-ld`
    /// arguments added as well.
    ///
    /// This means that functionally it looks like `clap` parses everything when
    /// in fact `lexopt` is used to filter out `wasm-ld` arguments and `clap`
    /// only parses arguments specific to `wasm-component-ld`.
    fn parse() -> Result<App> {
        let mut args = env::args_os().collect::<Vec<_>>();

        // First remove `-flavor wasm` in case this is invoked as a generic LLD
        // driver. We can safely ignore that going forward.
        if let Some([flavor, wasm]) = args.get(1..3) {
            if flavor == "-flavor" && wasm == "wasm" {
                args.remove(1);
                args.remove(1);
            }
        }

        let mut command = ComponentLdArgs::command();
        let mut lld_args = Vec::new();
        let mut component_ld_args = vec![std::env::args_os().nth(0).unwrap()];
        let mut shared = false;
        let mut parser = lexopt::Parser::from_iter(args);
        loop {
            if let Some(mut args) = parser.try_raw_args() {
                if let Some(arg) = args.peek() {
                    let for_lld = LLD_LONG_FLAGS_NONSTANDARD.iter().any(|s| arg == *s);
                    if for_lld {
                        lld_args.push(arg.to_owned());
                        if arg == "-shared" {
                            shared = true;
                        }
                        args.next();
                        continue;
                    }
                }
            }

            match parser.next()? {
                Some(Arg::Value(obj)) => {
                    lld_args.push(obj);
                }
                Some(Arg::Short(c)) => match LLD_FLAGS.iter().find(|f| f.short == Some(c)) {
                    Some(lld) => {
                        lld_args.push(format!("-{c}").into());
                        if lld.value.is_some() {
                            lld_args.push(parser.value()?);
                        }
                    }
                    None => {
                        component_ld_args.push(format!("-{c}").into());
                        if let Some(arg) =
                            command.get_arguments().find(|a| a.get_short() == Some(c))
                        {
                            if let ArgAction::Set = arg.get_action() {
                                component_ld_args.push(parser.value()?);
                            }
                        }
                    }
                },
                Some(Arg::Long(c)) => match LLD_FLAGS.iter().find(|f| f.long == Some(c)) {
                    Some(lld) => {
                        lld_args.push(format!("--{c}").into());
                        if lld.value.is_some() {
                            lld_args.push(parser.value()?);
                        }
                    }
                    None => {
                        component_ld_args.push(format!("--{c}").into());
                        if let Some(arg) = command.get_arguments().find(|a| a.get_long() == Some(c))
                        {
                            if let ArgAction::Set = arg.get_action() {
                                component_ld_args.push(parser.value()?);
                            }
                        }
                    }
                },
                None => break,
            }
        }

        match command.try_get_matches_from_mut(component_ld_args.clone()) {
            Ok(matches) => Ok(App {
                component: ComponentLdArgs::from_arg_matches(&matches)?,
                lld_args,
                shared,
            }),
            Err(_) => {
                add_wasm_ld_options(ComponentLdArgs::command()).get_matches_from(component_ld_args);
                unreachable!();
            }
        }
    }

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
            cmd.arg("-o").arg(self.component.output.as_ref().unwrap());
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
        } else if self.component.wasi_proxy_adapter {
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

        std::fs::write(self.component.output.as_ref().unwrap(), &component)
            .context("failed to write output file")?;

        Ok(())
    }

    fn lld(&self) -> Command {
        let mut lld = self.find_lld();
        lld.args(&self.lld_args);
        lld
    }

    fn find_lld(&self) -> Command {
        if let Some(path) = &self.component.wasm_ld_path {
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

fn add_wasm_ld_options(mut command: clap::Command) -> clap::Command {
    use clap::Arg;

    command = command.arg(
        Arg::new("objects")
            .action(ArgAction::Append)
            .help("objects to pass to `wasm-ld`"),
    );

    for flag in LLD_FLAGS {
        let mut arg = Arg::new(flag.clap_name).help("forwarded to `wasm-ld`");
        if let Some(short) = flag.short {
            arg = arg.short(short);
        }
        if let Some(long) = flag.long {
            arg = arg.long(long);
        }
        arg = arg.action(if flag.value.is_some() {
            ArgAction::Set
        } else {
            ArgAction::SetTrue
        });
        arg = arg.help_heading("Options forwarded to `wasm-ld`");
        command = command.arg(arg);
    }

    command
}
