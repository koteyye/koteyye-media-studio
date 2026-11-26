use std::process::ExitCode;

const HELP: &str = "Koteyye Media Studio CLI\n\nUsage:\n  koteyye-media-studio-cli mcp serve [--show-token]\n  koteyye-media-studio-cli mcp stdio\n\nserve starts the loopback MCP server with the Studio's saved settings.\nUse --show-token to print the connection token.\nstdio bridges a running server using KOTEYYE_MCP_TOKEN.";

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    let args: Vec<_> = args.iter().map(String::as_str).collect();
    let result = match args.as_slice() {
        [] | ["--help" | "-h"] | ["mcp", "--help"] | ["mcp", "serve", "--help"] => {
            println!("{HELP}");
            return ExitCode::SUCCESS;
        }
        ["--version" | "-V"] => {
            println!("koteyye-media-studio-cli {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        ["mcp", "serve"] => koteyye_media_studio::run_mcp_server(false),
        ["mcp", "serve", "--show-token"] => koteyye_media_studio::run_mcp_server(true),
        ["mcp", "stdio"] => koteyye_media_studio::run_stdio(),
        _ => {
            eprintln!("{HELP}");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
