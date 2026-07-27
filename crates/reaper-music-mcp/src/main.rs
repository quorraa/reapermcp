//! The `qlabs-reaper-music-mcp` binary.
//!
//! Two responsibilities and nothing else: install the stderr panic hook before
//! anything can panic, and hand the command line to [`reaper_music_mcp::cli`].
//!
//! The panic hook comes first deliberately. The default Rust hook also writes
//! to stderr, but it is replaced anyway so that a panic produces exactly one
//! structured line and so no future change to the default can put a backtrace
//! anywhere near stdout — which, under `serve`, carries JSON-RPC and nothing
//! else.

fn main() {
    reaper_music_mcp::log::install_panic_hook();
    let argv: Vec<String> = std::env::args().skip(1).collect();

    // `serve` writes protocol frames through its own stdout sink and must not
    // receive a second handle; every other command writes its report here.
    let mut out = std::io::stdout();
    let code = reaper_music_mcp::cli::run(&argv, &mut out);
    std::process::exit(code);
}
