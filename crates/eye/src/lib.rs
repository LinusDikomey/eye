/// command line argument parsing
pub mod args;
#[cfg(feature = "format")]
mod fmt_command;

use std::{
    io::Read,
    path::{Path, PathBuf},
};

use args::Backend;
pub use compiler::Compiler;

use compiler::{
    Def, ModuleSpan,
    compiler::{Dialects, Instances, ModuleStorage},
};

use error::{Error, span::TSpan};
use target::UnknownNativeTargetError;

#[derive(Debug)]
pub enum MainError {
    CantAccessPath(std::io::Error, PathBuf),
    NonexistentPath(PathBuf),
    MissingProjectDirectoryName(PathBuf),
    InvalidWorkingDirectory,
    MissingProjectFileName(PathBuf),
    NoMainFileInProjectDirectory,
    InvalidPath(PathBuf),
    ErrorsFound,
    BackendFailed(String),
    LinkingFailed(String),
    RunningProgramFailed(std::io::Error),
    ReadingStdinFailed(std::io::Error),
    LspFailed(String),
    UnknownNativeTarget(UnknownNativeTargetError),
}
impl From<compiler::ProjectError> for MainError {
    fn from(value: compiler::ProjectError) -> Self {
        match value {
            compiler::ProjectError::CantAccessPath(err, path) => Self::CantAccessPath(err, path),
            compiler::ProjectError::NonexistentPath(path) => Self::NonexistentPath(path),
        }
    }
}
impl From<target::UnknownNativeTargetError> for MainError {
    fn from(value: target::UnknownNativeTargetError) -> Self {
        Self::UnknownNativeTarget(value)
    }
}

pub fn run(args: args::Args) -> Result<(), MainError> {
    let target = args
        .target
        .clone()
        .map_or_else(target::Target::native, Ok)?;
    match args.cmd {
        args::Cmd::ListTargets => {
            list_targets(args.backend);
            return Ok(());
        }
        #[cfg(feature = "lsp")]
        args::Cmd::Lsp => {
            eye_lsp::run();
            return Ok(());
        }
        #[cfg(feature = "format")]
        args::Cmd::Fmt(fmt_args) => return fmt_command::format(args.path, fmt_args),
        _ => {}
    }
    let start_time = std::time::Instant::now();
    let mut compiler = compiler::Compiler::new();
    if args.crash_on_error {
        eprintln!("enabling crash on error");
        compiler.errors.enable_crash_on_error();
    }

    // add standard library
    let std = compiler.add_project("std".to_owned(), compiler::std_path::find(), vec![])?;

    // create project
    let dependencies = vec![std];
    let (name, storage) = path_arg(args.path)?;
    println!("Compiling {name} ...");
    let project = match storage {
        ModuleStorage::Path(path) => compiler.add_project(name.clone(), path, dependencies)?,
        ModuleStorage::String(content) => {
            compiler.add_single_file_project_from_string(name.clone(), content, dependencies)
        }
    };
    let root_module = compiler.get_project(project).root_module;

    compiler.resolve_builtins(std);

    // always check the complete project
    compiler.check_complete_project(project);

    // check that the main function exists if we are not compiling a library
    let main = (!args.lib)
        .then(|| {
            let main_def = compiler.resolve_in_module(
                root_module,
                "main",
                ModuleSpan {
                    module: root_module,
                    span: TSpan::new(0, 0),
                },
            );
            let (main_function_module, main_id) = match main_def {
                Def::Function(main_module, main_id) => (main_module, main_id),
                Def::Invalid => {
                    compiler.print_errors();
                    return Err(MainError::ErrorsFound);
                }
                other => {
                    // unwrapping is fine, only returns none when the Def is invalid
                    let span = other.get_span(&mut compiler).unwrap();

                    compiler
                        .errors
                        .emit(span.module, Error::MainIsNotAFunction.at_span(span.span));
                    compiler.print_errors();
                    return Err(MainError::ErrorsFound);
                }
            };
            let signature = compiler.get_signature(main_function_module, main_id);
            if let Err(err) = compiler::check::verify_main_signature(&compiler, signature) {
                if let Some(error) = err {
                    compiler.errors.emit(main_function_module, error);
                }
                compiler.print_errors();
                return Err(MainError::ErrorsFound);
            }
            Ok((main_function_module, main_id))
        })
        .transpose()?;

    if tracing::enabled!(target: "hir", tracing::Level::DEBUG) {
        compiler.print_project_hir(project);
    }

    if compiler.print_errors() && !args.run_with_errors {
        return Err(MainError::ErrorsFound);
    }

    match args.cmd {
        args::Cmd::Check => {}
        args::Cmd::Build | args::Cmd::Run => {
            let mut env = ir::Environment::new(ir::Primitive::create_infos());
            let dialects = Dialects {
                arith: env.get_dialect_module(),
                tuple: env.get_dialect_module(),
                mem: env.get_dialect_module(),
                cf: env.get_dialect_module(),
                main: env.create_module("main"),
            };
            let mut instances = Instances::new();
            if let Some(main) = main {
                let Some(main_ir_id) =
                    compiler.emit_ir_from_root(&mut env, &dialects, &mut instances, main)
                else {
                    if args.run_with_errors {
                        eprintln!("Failed to run due to main function failing to generate");
                    }
                    return Err(MainError::ErrorsFound);
                };
                // verification was already done so the error can be ignored here
                _ = compiler.verify_main_and_add_entry_point(
                    &mut env,
                    &dialects,
                    main,
                    main_ir_id,
                    &mut instances,
                    &target,
                );
            } else {
                compiler.emit_whole_project_ir(&mut env, &dialects, &mut instances, project);
            }
            if tracing::enabled!(target: "ir", tracing::Level::DEBUG) {
                if args.debug_functions.is_empty() {
                    tracing::debug!(target: "ir", "{}", env.display_module(dialects.main));
                } else {
                    // display functions separately if there was a filter on the output functions so that
                    // they will be filtered
                    for func in env.get_module(dialects.main).functions() {
                        tracing::debug!(
                            target: "ir", function = func.name, "\n{}",
                            func.display(&env),
                        );
                    }
                }
            }
            if compiler.print_errors() && !args.run_with_errors {
                // compiler errors might have originated from generating dependencies
                return Err(MainError::ErrorsFound);
            }
            // Done with front-end, drop compiler so all the memory gets freed while in backend
            drop(compiler);

            #[cfg(debug_assertions)]
            ir::verify::module(&env, dialects.main);

            if args.optimize {
                let pipeline = ir::optimize::optimizing_pipeline(&mut env);
                pipeline.process_module(&mut env, dialects.main);

                #[cfg(debug_assertions)]
                ir::verify::module(&env, dialects.main);
            }

            std::fs::create_dir_all(Path::new("eyebuild")).unwrap();
            let obj_file = format!("eyebuild/{name}.o");
            match args.backend {
                Backend::C => todo!("reimplement C backend"),
                #[cfg(feature = "fast-backend")]
                Backend::Fast => {
                    let backend = ir_backend_fast::Backend::new();
                    backend
                        .emit_module(&mut env, dialects.main, &target, Path::new(&obj_file))
                        .map_err(|err| MainError::BackendFailed(format!("{err:?}")))?;
                }
                #[cfg(feature = "llvm-backend")]
                Backend::Llvm => {
                    let backend = ir_backend_llvm::Backend::new();
                    let target = args.target.as_ref().map(|target| {
                        std::ffi::CString::new(target.to_string().as_bytes())
                            .expect("invalid target string")
                    });
                    backend
                        .emit_module(
                            &mut env,
                            dialects.main,
                            target.as_deref(),
                            Path::new(&obj_file),
                        )
                        .map_err(|err| MainError::BackendFailed(format!("{err:?}")))?;
                }
            }
            if args.timings {
                eprintln!("Compiling took {:?}", start_time.elapsed());
            }
            if args.emit_obj || args.lib {
                return Ok(());
            }
            let exe_file = exe_file_path(&name);
            if let Err(err) = compiler::link(
                &obj_file,
                &exe_file,
                args.link_cmd.as_deref(),
                &args.link,
                &target,
            ) {
                return Err(MainError::LinkingFailed(err));
            }
            if matches!(args.cmd, args::Cmd::Run) {
                println!("Running {name}...");
                let mut command = std::process::Command::new(exe_file);

                #[cfg(target_family = "unix")]
                {
                    let err = std::os::unix::process::CommandExt::exec(&mut command);
                    return Err(MainError::RunningProgramFailed(err));
                }
                #[cfg(not(target_family = "unix"))]
                {
                    let exit_code = command
                        .status()
                        .map_err(MainError::RunningProgramFailed)?
                        .code()
                        .unwrap_or(0);
                    std::process::exit(exit_code);
                }
            }
        }
        args::Cmd::ListTargets => unreachable!(),
        #[cfg(feature = "format")]
        args::Cmd::Fmt(_) => unreachable!(),
        #[cfg(feature = "lsp")]
        args::Cmd::Lsp => unreachable!(),
    }

    Ok(())
}

pub fn exe_file_path(name: &str) -> String {
    #[cfg(not(target_os = "windows"))]
    let exe_file_extension = "";
    #[cfg(target_os = "windows")]
    let exe_file_extension = ".exe";
    format!("eyebuild/{name}{exe_file_extension}")
}

pub fn path_arg(path: Option<String>) -> Result<(String, ModuleStorage), MainError> {
    let path = match path {
        Some(s) if s == "-" => {
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .map_err(MainError::ReadingStdinFailed)?;
            return Ok((
                "<stdin>".to_owned(),
                ModuleStorage::String(buf.into_boxed_str()),
            ));
        }
        Some(path_str) => {
            let path = PathBuf::from(path_str);
            path.canonicalize().unwrap_or(path)
        }
        None => std::env::current_dir().map_err(|_| MainError::InvalidWorkingDirectory)?,
    };
    if !path
        .try_exists()
        .map_err(|err| MainError::CantAccessPath(err, path.to_path_buf()))?
    {
        return Err(MainError::NonexistentPath(path.to_owned()));
    }
    let name = if path.is_file() {
        path.file_stem()
            .ok_or_else(|| MainError::MissingProjectFileName(path.to_path_buf()))?
            .to_str()
            .ok_or_else(|| MainError::InvalidPath(path.to_path_buf()))?
    } else {
        path.file_name()
            .ok_or_else(|| MainError::MissingProjectDirectoryName(path.to_path_buf()))?
            .to_str()
            .ok_or_else(|| MainError::InvalidPath(path.to_path_buf()))?
    };
    Ok((name.to_owned(), ModuleStorage::Path(path)))
}

pub fn enable_tracing(args: &args::Args) {
    #[cfg(not(debug_assertions))]
    {
        if !args.debug.is_empty() {
            eprintln!(
                "Tried to enable debugging in a release build which doesn't support debug tracing"
            );
        }
        return;
    }

    #[cfg(debug_assertions)]
    {
        use std::collections::HashSet;
        use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};

        struct FunctionFilter {
            allowed: HashSet<Box<str>>,
        }
        impl<S> tracing_subscriber::layer::Filter<S> for FunctionFilter {
            fn enabled(
                &self,
                _meta: &tracing::Metadata<'_>,
                _cx: &tracing_subscriber::layer::Context<'_, S>,
            ) -> bool {
                true
            }

            fn event_enabled(
                &self,
                event: &tracing::Event<'_>,
                _cx: &tracing_subscriber::layer::Context<'_, S>,
            ) -> bool {
                let mut matched = None;
                struct StringVisitor<'a> {
                    allowed: &'a HashSet<Box<str>>,
                    matched: &'a mut Option<bool>,
                }
                impl<'a> tracing::field::Visit for StringVisitor<'a> {
                    fn record_debug(
                        &mut self,
                        _field: &tracing::field::Field,
                        _value: &dyn std::fmt::Debug,
                    ) {
                    }

                    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                        if field.name() == "function" {
                            let name = value
                                .split_once('[')
                                .map_or(value, |(name, _generics)| name);
                            *self.matched = Some(self.allowed.contains(name))
                        }
                    }
                }
                event.record(&mut StringVisitor {
                    allowed: &self.allowed,
                    matched: &mut matched,
                });
                matched.unwrap_or(true)
            }
        }
        let mut env_filter;
        if args.debug.iter().any(|s| &**s == "all") {
            env_filter = EnvFilter::new("debug")
        } else {
            env_filter = EnvFilter::new("info");
            for flag in &args.debug {
                env_filter = env_filter.add_directive(format!("{flag}=debug").parse().unwrap());
            }
        };
        let subscriber = tracing_subscriber::registry().with(env_filter);
        let fmt_layer = tracing_subscriber::fmt::layer().without_time();

        if !args.debug_functions.is_empty() {
            subscriber
                .with(fmt_layer.with_filter(FunctionFilter {
                    allowed: args.debug_functions.iter().cloned().collect(),
                }))
                .init();
        } else {
            subscriber
                .with(fmt_layer)
                .with(tracing_error::ErrorLayer::default())
                .init();
        }
        let default_panic_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let span_trace = tracing_error::SpanTrace::capture();
            eprintln!("panic occured:\n{span_trace}");
            default_panic_hook(info);
        }));
    }
}

fn list_targets(backend: args::Backend) {
    let targets = match backend {
        Backend::C => vec!["C".to_owned()],
        #[cfg(feature = "llvm-backend")]
        Backend::Llvm => ir_backend_llvm::list_targets(),
        #[cfg(feature = "fast-backend")]
        Backend::Fast => ir_backend_fast::list_targets(),
    };
    println!(
        "Available targets for {backend} backend: (total: {})",
        targets.len()
    );
    for target in targets {
        println!("\t{target}");
    }
}
