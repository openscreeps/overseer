//! `overseer-proxy`: forwards one Screeps server to browsers (see the `overseer_proxy` crate
//! documentation for what passes through and what the operator can see).
//!
//! ```sh
//! overseer-proxy --upstream https://screeps.com/ --static target/web-demo
//! ```
//!
//! Logs go to standard error, one line per request (method, path, status, duration). Stop with
//! Ctrl-C.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use overseer_proxy::{Proxy, ProxyConfig};
use tracing::level_filters::LevelFilter;
use tracing::{error, info};
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use url::Url;

/// Forwards `api/*`, `room-history[/*]` and `socket/websocket` to one Screeps server, so that
/// browser builds of overseer can reach servers without CORS support (such as screeps.com).
/// The operator of a proxy sees every token passing through it: run your own.
#[derive(Debug, Parser)]
#[command(version)]
struct Args {
    /// Upstream base URL; a path prefix (`https://screeps.com/season/`) is kept.
    #[arg(long, env = "OVERSEER_PROXY_UPSTREAM", value_name = "URL")]
    upstream: Url,
    /// Address to listen on (`0.0.0.0:8080` for every interface).
    #[arg(
        long,
        env = "OVERSEER_PROXY_LISTEN",
        value_name = "ADDRESS",
        default_value = "127.0.0.1:8080"
    )]
    listen: SocketAddr,
    /// Directory served for every other path (the web build, served same-origin).
    #[arg(long = "static", env = "OVERSEER_PROXY_STATIC", value_name = "DIR")]
    static_dir: Option<PathBuf>,
    /// Origin (`scheme://host[:port]`) of a page allowed to use the proxy cross-origin;
    /// repeatable, comma-separated in the environment variable.
    #[arg(
        long = "allow-origin",
        env = "OVERSEER_PROXY_ALLOW_ORIGIN",
        value_name = "ORIGIN",
        value_delimiter = ','
    )]
    allow_origin: Vec<String>,
}

#[tokio::main]
async fn main() -> ExitCode {
    let args = Args::parse();
    // The proxy's own request lines; dependencies only when something goes wrong.
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_writer(std::io::stderr))
        .with(
            Targets::new()
                .with_default(LevelFilter::WARN)
                .with_target("overseer_proxy", LevelFilter::INFO),
        )
        .init();
    let config = ProxyConfig {
        upstream: args.upstream,
        listen: args.listen,
        static_dir: args.static_dir,
        allowed_origins: args.allow_origin,
    };
    let proxy = match Proxy::start(config).await {
        Ok(proxy) => proxy,
        Err(failure) => {
            error!("{failure}");
            return ExitCode::FAILURE;
        }
    };
    info!(url = %proxy.url(), upstream = %proxy.upstream(), "listening");
    if let Err(failure) = tokio::signal::ctrl_c().await {
        error!("cannot wait for Ctrl-C: {failure}");
    }
    info!("stopping");
    proxy.shutdown().await;
    ExitCode::SUCCESS
}
