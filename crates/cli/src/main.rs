fn main() -> std::process::ExitCode {
    claude_sync_cli::run(std::env::args().collect(), &claude_sync_cli::Ctx::cli())
}
