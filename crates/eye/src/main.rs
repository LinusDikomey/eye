fn main() -> Result<(), eye::MainError> {
    let args: eye::args::Args = clap::Parser::parse();
    #[cfg(feature = "lsp")]
    if !matches!(args.cmd, eye::args::Cmd::Lsp) {
        eye::enable_tracing(&args);
    }
    #[cfg(not(feature = "lsp"))]
    eye::enable_tracing(&args);
    eye::run(args)
}
