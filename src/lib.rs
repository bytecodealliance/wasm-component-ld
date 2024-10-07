use anyhow::{bail, Context, Result};
use clap::{ArgAction, CommandFactory, FromArgMatches};
use lexopt::Arg;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::str::FromStr;
use wasmparser::Payload;
use wit_component::StringEncoding;
use wit_parser::{Resolve, WorldId};

/// Representation of a flag passed to `wasm-ld`
///
/// Note that the parsing of flags in `wasm-ld` is not as uniform as parsing
/// arguments via `clap`. For example if `--foo bar` is supported that doesn't
/// mean that `--foo=bar` is supported. Similarly some options such as `--foo`
/// support optional values as `--foo=bar` but can't be specified as
/// `--foo bar`.
///
/// Finally there's currently only one "weird" flag which is `-shared` which has
/// a single dash but a long name. That's specially handled elsewhere.
///
/// The general goal here is that we want to inherit `wasm-ld`'s CLI but also
/// want to be able to reserve CLI flags for this linker itself, so `wasm-ld`'s
/// arguments are parsed where our own are intermixed.
struct LldFlag {
    clap_name: &'static str,
    long: Option<&'static str>,
    short: Option<char>,
    value: FlagValue,
}

enum FlagValue {
    /// This option has no value, e.g. `-f` or `--foo`
    None,

    /// This option's value must be specified with `=`, for example `--foo=bar`
    RequiredEqual(&'static str),

    /// This option's value must be specified with ` `, for example `--foo bar`.
    ///
    /// I think that `wasm-ld` supports both `-f foo` and `-ffoo` for
    /// single-character flags, but I haven't tested as putting a space seems to
    /// work.
    RequiredSpace(&'static str),

    /// This option's value is optional but if specified it must use an `=` for
    /// example `--foo=bar` or `--foo`.
    Optional(&'static str),
}

/// This is a large macro which is intended to take CLI-looking syntax and turn
/// each individual flag into a `LldFlag` specified above.
macro_rules! flag {
    // Long options specified as:
    //
    //     -f / --foo
    //
    // or just
    //
    //     --foo
    //
    // Options can look like `--foo`, `--foo=bar`, `--foo[=bar]`, or
    // `--foo bar` to match the kinds of flags that LLD supports.
    ($(-$short:ident /)? --$($flag:tt)*) => {
        LldFlag {
            clap_name: concat!("long_", $(stringify!($flag),)*),
            long: Some(flag!(@name [] $($flag)*)),
            short: flag!(@short $($short)?),
            value: flag!(@value $($flag)*),
        }
    };

    // Short options specified as `-f` or `-f foo`.
    (-$flag:tt $($val:tt)*) => {
        LldFlag {
            clap_name: concat!("short_", stringify!($flag)),
            long: None,
            short: Some(flag!(@char $flag)),
            value: flag!(@value $flag $($val)*),
        }
    };

    // Generates the long name of a flag, collected within the `[]` argument to
    // this macro. This will iterate over the flag given as the rest of the
    // macro arguments and collect values into `[...]` and recurse.
    //
    // The first recursion case handles `foo-bar-baz=..` where Rust tokenizes
    // this as `foo` then `-` then `bar` then ... If this is found then `foo-`
    // is added to the name and then the macro recurses.
    (@name [$($name:tt)*] $n:ident-$($rest:tt)*) => (flag!(@name [$($name)* $n-] $($rest)*));
    // These are the ways options are represented, either `--foo bar`,
    // `--foo=bar`, `--foo=bar`, or `--foo`. In all these cases discard the
    // value itself and then recurse.
    (@name [$($name:tt)*] $n:ident $_value:ident) => (flag!(@name [$($name)* $n]));
    (@name [$($name:tt)*] $n:ident=$_value:ident) => (flag!(@name [$($name)* $n]));
    (@name [$($name:tt)*] $n:ident[=$_value:ident]) => (flag!(@name [$($name)* $n]));
    (@name [$($name:tt)*] $n:ident) => (flag!(@name [$($name)* $n]));
    // If there's nothing left then the `$name` has collected everything so
    // it's stringifyied and caoncatenated.
    (@name [$($name:tt)*]) => (concat!($(stringify!($name),)*));

    // This parses the value-style of the flag given. The recursion here looks
    // similar to `@name` above. except that the four terminal cases all
    // correspond to different variants of `FlagValue`.
    (@value $n:ident - $($rest:tt)*) => (flag!(@value $($rest)*));
    (@value $_flag:ident = $name:ident) => (FlagValue::RequiredEqual(stringify!($name)));
    (@value $_flag:ident $name:ident) => (FlagValue::RequiredSpace(stringify!($name)));
    (@value $_flag:ident [= $name:ident]) => (FlagValue::Optional(stringify!($name)));
    (@value $_flag:ident) => (FlagValue::None);

    // Helper for flags that have both a long and a short form to parse whether
    // a short form was provided.
    (@short) => (None);
    (@short $name:ident) => (Some(flag!(@char $name)));

    // Helper for getting the `char` of a short flag.
    (@char $name:ident) => ({
        let name = stringify!($name);
        assert!(name.len() == 1);
        name.as_bytes()[0] as char
    });
}

const LLD_FLAGS: &[LldFlag] = &[
    flag! { --allow-undefined-file=PATH },
    flag! { --allow-undefined },
    flag! { --Bdynamic },
    flag! { --Bstatic },
    flag! { --Bsymbolic },
    flag! { --build-id[=VAL] },
    flag! { --call_shared },
    flag! { --check-features },
    flag! { --color-diagnostics[=VALUE] },
    flag! { --compress-relocations },
    flag! { --demangle },
    flag! { --dn },
    flag! { --dy },
    flag! { --emit-relocs },
    flag! { --end-lib },
    flag! { --entry SYM },
    flag! { --error-limit=N },
    flag! { --error-unresolved-symbols },
    flag! { --experimental-pic },
    flag! { --export-all },
    flag! { -E / --export-dynamic },
    flag! { --export-if-defined=SYM },
    flag! { --export-memory[=NAME] },
    flag! { --export-table },
    flag! { --export=SYM },
    flag! { --extra-features=LIST },
    flag! { --fatal-warnings },
    flag! { --features=LIST },
    flag! { --gc-sections },
    flag! { --global-base=VALUE },
    flag! { --growable-table },
    flag! { --import-memory[=NAME] },
    flag! { --import-table },
    flag! { --import-undefined },
    flag! { --initial-heap=SIZE },
    flag! { --initial-memory=SIZE },
    flag! { --keep-section=NAME },
    flag! { --lto-CGO=LEVEL },
    flag! { --lto-debug-pass-manager },
    flag! { --lto-O=LEVEL },
    flag! { --lto-partitions=NUM },
    flag! { -L PATH },
    flag! { -l LIB },
    flag! { --Map=FILE },
    flag! { --max-memory=SIZE },
    flag! { --merge-data-segments },
    flag! { --mllvm=FLAG },
    flag! { -m ARCH },
    flag! { --no-check-features },
    flag! { --no-color-diagnostics },
    flag! { --no-demangle },
    flag! { --no-entry },
    flag! { --no-export-dynamic },
    flag! { --no-gc-sections },
    flag! { --no-merge-data-segments },
    flag! { --no-pie },
    flag! { --no-print-gc-sections },
    flag! { --no-whole-archive },
    flag! { --non_shared },
    flag! { -O LEVEL },
    flag! { --pie },
    flag! { --print-gc-sections },
    flag! { -M / --print-map },
    flag! { --relocatable },
    flag! { --save-temps },
    flag! { --shared-memory },
    flag! { --shared },
    flag! { --soname=VALUE },
    flag! { --stack-first },
    flag! { --start-lib },
    flag! { --static },
    flag! { -s / --strip-all },
    flag! { -S / --strip-debug },
    flag! { --table-base=VALUE },
    flag! { --thinlto-cache-dir=PATH },
    flag! { --thinlto-cache-policy=VALUE },
    flag! { --thinlto-jobs=N },
    flag! { --threads=N },
    flag! { -y / --trace-symbol=SYM },
    flag! { -t / --trace },
    flag! { --undefined=SYM },
    flag! { --unresolved-symbols=VALUE },
    flag! { --warn-unresolved-symbols },
    flag! { --whole-archive },
    flag! { --why-extract=MEMBER },
    flag! { --wrap=VALUE },
    flag! { -z OPT },
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
#[command(version)]
struct ComponentLdArgs {
    /// Which default WASI adapter, if any, to use when creating the output
    /// component.
    #[clap(long, name = "command|reactor|proxy|none")]
    wasi_adapter: Option<WasiAdapter>,

    /// Location of where to find `wasm-ld`.
    ///
    /// If not specified this is automatically detected.
    #[clap(long, name = "PATH")]
    wasm_ld_path: Option<PathBuf>,

    /// Quoting syntax for response files.
    #[clap(long, name = "STYLE")]
    rsp_quoting: Option<String>,

    /// Where to place the component output.
    #[clap(short, long)]
    output: PathBuf,

    /// Print verbose output.
    #[clap(long)]
    verbose: bool,

    /// Whether or not the output component is validated.
    ///
    /// This defaults to `true`.
    #[clap(long)]
    validate_component: Option<bool>,

    /// Whether or not imports are deduplicated based on semver in the final
    /// component.
    ///
    /// This defaults to `true`.
    #[clap(long)]
    merge_imports_based_on_semver: Option<bool>,

    /// Adapters to use when creating the final component.
    #[clap(long = "adapt", value_name = "[NAME=]MODULE", value_parser = parse_adapter)]
    adapters: Vec<(String, Vec<u8>)>,

    /// WIT file representing additional component type information to use.
    ///
    /// May be specified more than once.
    ///
    /// See also the `--string-encoding` option.
    #[clap(long = "component-type", value_name = "WIT_FILE")]
    component_types: Vec<PathBuf>,

    /// String encoding to use when creating the final component.
    ///
    /// This may be either "utf8", "utf16", or "compact-utf16".  This value is
    /// only used when one or more `--component-type` options are specified.
    #[clap(long, value_parser = parse_encoding, default_value = "utf8")]
    string_encoding: StringEncoding,

    /// Skip the `wit-component`-based process to generate a component.
    #[clap(long)]
    skip_wit_component: bool,
}

fn parse_adapter(s: &str) -> Result<(String, Vec<u8>)> {
    let (name, path) = parse_optionally_name_file(s);
    let wasm = wat::parse_file(path)?;
    Ok((name.to_string(), wasm))
}

fn parse_encoding(s: &str) -> Result<StringEncoding> {
    Ok(match s {
        "utf8" => StringEncoding::UTF8,
        "utf16" => StringEncoding::UTF16,
        "compact-utf16" => StringEncoding::CompactUTF16,
        _ => bail!("unknown string encoding: {s:?}"),
    })
}

fn parse_optionally_name_file(s: &str) -> (&str, &str) {
    let mut parts = s.splitn(2, '=');
    let name_or_path = parts.next().unwrap();
    match parts.next() {
        Some(path) => (name_or_path, path),
        None => {
            let name = Path::new(name_or_path)
                .file_name()
                .unwrap()
                .to_str()
                .unwrap();
            let name = match name.find('.') {
                Some(i) => &name[..i],
                None => name,
            };
            (name, name_or_path)
        }
    }
}

#[derive(Debug, Copy, Clone)]
enum WasiAdapter {
    Command,
    Reactor,
    Proxy,
    None,
}

impl FromStr for WasiAdapter {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "none" => Ok(WasiAdapter::None),
            "command" => Ok(WasiAdapter::Command),
            "reactor" => Ok(WasiAdapter::Reactor),
            "proxy" => Ok(WasiAdapter::Proxy),
            _ => bail!("unknown wasi adapter {s}, must be one of: none, command, reactor, proxy"),
        }
    }
}

pub fn main() {
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

        fn handle_lld_arg(
            lld: &LldFlag,
            parser: &mut lexopt::Parser,
            lld_args: &mut Vec<OsString>,
        ) -> Result<()> {
            let mut arg = OsString::new();
            match (lld.short, lld.long) {
                (_, Some(long)) => {
                    arg.push("--");
                    arg.push(long);
                }
                (Some(short), _) => {
                    arg.push("-");
                    arg.push(short.encode_utf8(&mut [0; 5]));
                }
                (None, None) => unreachable!(),
            }
            match lld.value {
                FlagValue::None => {
                    lld_args.push(arg);
                }

                FlagValue::RequiredSpace(_) => {
                    lld_args.push(arg);
                    lld_args.push(parser.value()?);
                }

                FlagValue::RequiredEqual(_) => {
                    arg.push("=");
                    arg.push(&parser.value()?);
                    lld_args.push(arg);
                }

                // If the value is optional then the argument must have an `=`
                // in the argument itself.
                FlagValue::Optional(_) => {
                    match parser.optional_value() {
                        Some(val) => {
                            arg.push("=");
                            arg.push(&val);
                        }
                        None => {}
                    }
                    lld_args.push(arg);
                }
            }
            Ok(())
        }

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
                        handle_lld_arg(lld, &mut parser, &mut lld_args)?;
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
                        handle_lld_arg(lld, &mut parser, &mut lld_args)?;
                    }
                    None => {
                        component_ld_args.push(format!("--{c}").into());
                        if let Some(arg) = command.get_arguments().find(|a| a.get_long() == Some(c))
                        {
                            match arg.get_action() {
                                ArgAction::Set | ArgAction::Append => {
                                    component_ld_args.push(parser.value()?)
                                }
                                _ => (),
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

        // If a temporary output is needed make sure it has the same file name
        // as the output of our command itself since LLD will embed this file
        // name in the name section of the output.
        let temp_dir = match self.component.output.parent() {
            Some(parent) => tempfile::TempDir::new_in(parent)?,
            None => tempfile::TempDir::new()?,
        };
        let temp_output = match self.component.output.file_name() {
            Some(name) => temp_dir.path().join(name),
            None => bail!(
                "output of {:?} does not have a file name",
                self.component.output
            ),
        };

        // Shared libraries don't get wit-component run below so place the
        // output directly at the desired output location. Otherwise output to a
        // temporary location for wit-component to read and then the real output
        // is created after wit-component runs.
        if self.skip_wit_component() {
            cmd.arg("-o").arg(&self.component.output);
        } else {
            cmd.arg("-o").arg(&temp_output);
        }

        if self.component.verbose {
            eprintln!("running LLD: {cmd:?}");
        }
        let status = cmd
            .status()
            .with_context(|| format!("failed to spawn {linker:?}"))?;
        if !status.success() {
            bail!("failed to invoke LLD: {status}");
        }

        if self.skip_wit_component() {
            return Ok(());
        }

        let reactor_adapter =
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_REACTOR_ADAPTER;
        let command_adapter =
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_COMMAND_ADAPTER;
        let proxy_adapter =
            wasi_preview1_component_adapter_provider::WASI_SNAPSHOT_PREVIEW1_PROXY_ADAPTER;
        let mut core_module = std::fs::read(&temp_output)
            .with_context(|| format!("failed to read {linker:?} output: {temp_output:?}"))?;

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

        if !self.component.component_types.is_empty() {
            let mut merged = None::<(Resolve, WorldId)>;
            for wit_file in &self.component.component_types {
                let mut resolve = Resolve::default();
                let (package, _) = resolve
                    .push_path(wit_file)
                    .with_context(|| format!("unable to add component type {wit_file:?}"))?;

                let world = resolve.select_world(package, None)?;

                if let Some((merged_resolve, merged_world)) = &mut merged {
                    let world = merged_resolve.merge(resolve)?.map_world(world, None)?;
                    merged_resolve.merge_worlds(world, *merged_world)?;
                } else {
                    merged = Some((resolve, world));
                }
            }

            let Some((resolve, world)) = merged else {
                unreachable!()
            };

            wit_component::embed_component_metadata(
                &mut core_module,
                &resolve,
                world,
                self.component.string_encoding,
            )?;
        }

        let mut encoder = wit_component::ComponentEncoder::default();
        if let Some(validate) = self.component.validate_component {
            encoder = encoder.validate(validate);
        }
        if let Some(merge) = self.component.merge_imports_based_on_semver {
            encoder = encoder.merge_imports_based_on_semver(merge);
        }
        encoder = encoder
            .module(&core_module)
            .context("failed to parse core wasm for componentization")?;
        let adapter = self.component.wasi_adapter.unwrap_or(if exports_start {
            WasiAdapter::Command
        } else {
            WasiAdapter::Reactor
        });
        let adapter = match adapter {
            WasiAdapter::Command => Some(&command_adapter[..]),
            WasiAdapter::Reactor => Some(&reactor_adapter[..]),
            WasiAdapter::Proxy => Some(&proxy_adapter[..]),
            WasiAdapter::None => None,
        };

        if let Some(adapter) = adapter {
            encoder = encoder
                .adapter("wasi_snapshot_preview1", adapter)
                .context("failed to inject adapter")?;
        }

        for (name, adapter) in self.component.adapters.iter() {
            encoder = encoder
                .adapter(name, adapter)
                .with_context(|| format!("failed to inject adapter {name:?}"))?;
        }

        let component = encoder.encode().context("failed to encode component")?;

        std::fs::write(&self.component.output, &component).context(format!(
            "failed to write output file: {:?}",
            self.component.output
        ))?;

        Ok(())
    }

    fn skip_wit_component(&self) -> bool {
        self.component.skip_wit_component
            // Skip componentization with `--shared` since that's creating a
            // shared library that's not a component yet.
            || self.shared
    }

    fn lld(&self) -> Command {
        let mut lld = self.find_lld();
        lld.args(&self.lld_args);
        if self.component.verbose {
            lld.arg("--verbose");
        }
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
        arg = match flag.value {
            FlagValue::RequiredEqual(name) | FlagValue::RequiredSpace(name) => {
                arg.action(ArgAction::Set).value_name(name)
            }
            FlagValue::Optional(name) => arg
                .action(ArgAction::Set)
                .value_name(name)
                .num_args(0..=1)
                .require_equals(true),
            FlagValue::None => arg.action(ArgAction::SetTrue),
        };
        arg = arg.help_heading("Options forwarded to `wasm-ld`");
        command = command.arg(arg);
    }

    command
}

#[test]
fn verify_app() {
    ComponentLdArgs::command().debug_assert();
    add_wasm_ld_options(ComponentLdArgs::command()).debug_assert();
}
