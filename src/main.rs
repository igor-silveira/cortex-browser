use anyhow::Result;
use clap::{Parser, Subcommand};
use std::io::Read;
use tracing::{debug, info};

use cortex_browser::{browser, dom, mcp, pipeline, serialize};

#[derive(Parser)]
#[command(name = "cortex-browser")]
#[command(about = "Compact browser perception layer for AI agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Take a snapshot of an HTML file, URL, or stdin
    Snapshot {
        /// HTML file path, URL (http/https), or '-' for stdin
        input: String,

        /// Output format: text or json
        #[arg(short, long, default_value = "text")]
        format: String,

        /// Chrome debugging port (for URL mode)
        #[arg(short, long, default_value_t = 9222)]
        port: u16,

        /// Launch a new headless Chrome instead of connecting
        #[arg(short, long)]
        launch: bool,
    },

    /// Start as an MCP (Model Context Protocol) server over stdio
    Mcp {
        /// Chrome debugging port to connect to
        #[arg(short, long, default_value_t = 9222)]
        port: u16,

        /// Launch a new headless Chrome instead of connecting
        #[arg(short, long)]
        launch: bool,
    },

    /// Start as an MCP server over HTTP (Streamable HTTP + SSE transport)
    McpHttp {
        /// Chrome debugging port to connect to
        #[arg(short, long, default_value_t = 9222)]
        port: u16,

        /// Launch a new headless Chrome instead of connecting
        #[arg(short, long)]
        launch: bool,

        /// Host to bind the HTTP server to
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to serve the MCP HTTP endpoint on
        #[arg(long, default_value_t = 8080)]
        http_port: u16,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Snapshot {
            input,
            format,
            port,
            launch,
        } => {
            info!(input = %input, format = %format, "snapshot command");
            if is_url(&input) {
                let rt = tokio::runtime::Runtime::new()?;
                rt.block_on(run_browser_snapshot(&input, &format, port, launch))
            } else {
                run_file_snapshot(&input, &format)
            }
        }
        Commands::Mcp { port, launch } => {
            info!(port = port, launch = launch, "starting MCP server");
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(mcp::run_mcp_server(launch, port))
        }
        Commands::McpHttp {
            port,
            launch,
            host,
            http_port,
        } => {
            info!(port = port, launch = launch, host = %host, http_port = http_port, "starting MCP HTTP server");
            let rt = tokio::runtime::Runtime::new()?;
            rt.block_on(mcp::run_mcp_http_server(launch, port, &host, http_port))
        }
    }
}

fn is_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

async fn run_browser_snapshot(url: &str, format: &str, port: u16, launch: bool) -> Result<()> {
    let browser = if launch {
        browser::launch().await?
    } else {
        browser::connect(port).await?
    };

    let (html, final_url) = browser::fetch_page(&browser, url).await?;
    debug!(html_len = html.len(), final_url = %final_url, "fetched page");
    let snapshot = pipeline::process(&html, &final_url);
    info!(nodes = snapshot.nodes.len(), "snapshot complete");
    print_output(&snapshot, format)
}

fn run_file_snapshot(input: &str, format: &str) -> Result<()> {
    let html = if input == "-" {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf)?;
        buf
    } else {
        std::fs::read_to_string(input)?
    };

    let url = if input == "-" { "" } else { input };
    let snapshot = pipeline::process(&html, url);
    print_output(&snapshot, format)
}

fn print_output(snapshot: &dom::PageSnapshot, format: &str) -> Result<()> {
    let output = match format {
        "json" => serde_json::to_string_pretty(snapshot)?,
        _ => serialize::to_compact_text(snapshot),
    };
    println!("{output}");
    Ok(())
}
