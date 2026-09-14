use gftools_builder::{BuildConfig, build};
use tracing_chrome::ChromeLayerBuilder;

use clap::Parser;
use std::{process::exit, time::Duration};
use tokio::{
    io::{AsyncWriteExt, stderr},
    time::sleep,
};
use tracing_subscriber::{EnvFilter, prelude::*};

#[derive(clap::Parser)]
#[command(version, about)]
struct Args {
    /// Increase logging
    #[command(flatten)]
    verbosity: clap_verbosity_flag::Verbosity,
    /// Generate the recipe and dump as YAML but do not build
    #[clap(long)]
    pub generate: bool,
    /// Enable profiling and write trace data to the specified file
    #[clap(long)]
    pub profile: Option<String>,
    #[cfg(feature = "graphviz")]
    /// Draw the graph of the build process
    /// This will create a file named `graph.svg` in the current directory
    #[clap(long)]
    graph: bool,
    #[clap(long)]
    ascii_graph: bool,
    /// Keep named intermediate files for debugging instead of anonymous temporaries/in-memory edges
    #[clap(long)]
    debug: bool,
    /// Disable progress bars
    #[clap(long)]
    no_progress: bool,
    /// Limit number of parallel jobs (defaults to number of CPU cores)
    #[clap(long)]
    jobs: Option<usize>,
    config_file: String,
}

#[tokio::main]
async fn main() {
    let args = Args::parse();
    let mut _guard = None;
    if let Some(ref profile_file) = args.profile {
        // Initialize tracing subscriber if profiling is enabled
        let (chrome_layer, guard) = ChromeLayerBuilder::new()
            .include_args(true)
            .include_locations(true)
            .file(profile_file)
            .build();
        _guard = Some(guard);

        let env_filter = EnvFilter::new("gftools_builder=info");

        // Set up the tracing subscriber with JSON output to stderr
        tracing_subscriber::registry()
            .with(env_filter)
            .with(chrome_layer)
            .init();
    }
    env_logger::Builder::new()
        .filter_level(args.verbosity.into())
        .init();

    let job_limit = args.jobs.unwrap_or_else(num_cpus::get);
    log::info!("Starting gftools-builder with {} parallel jobs", job_limit);

    let build_config = BuildConfig {
        config_path: args.config_file.clone(),
        job_limit,
        generate_only: args.generate,
        #[cfg(feature = "graphviz")]
        draw_graph: args.graph,
        ascii_graph: args.ascii_graph,
        debug_intermediates: args.debug,
        verbosity: args.verbosity.log_level().unwrap_or(log::Level::Info),
        progress: !args.no_progress,
    };

    if let Err(error) = build(build_config).await {
        stderr()
            .write_all(format!("{error}\n").as_bytes())
            .await
            .unwrap();

        // Delay for the error message to be written completely hopefully.
        sleep(Duration::from_millis(1)).await;

        exit(1)
    }
}
