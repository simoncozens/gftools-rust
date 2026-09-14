//! Build orchestrator module
//!
//! This code was heavily, heavily adopted from aviqqe/turtle-build.
//! Many thanks to Yota Toyama for making this code available under the MIT/Apache licenses.
//! A parallel build system in just under 200 lines of Rust is astonishing.
use crate::{
    buildsystem::{BuildGraph, BuildStep, OperationOutput, graph::BuildEdge},
    error::ApplicationError,
};
use async_recursion::async_recursion;
use dashmap::DashMap;
use futures::future::{FutureExt, Shared, try_join_all};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use petgraph::{
    Direction,
    graph::{EdgeIndex, EdgeReference, NodeIndex},
    visit::EdgeRef,
};
use std::{
    collections::HashSet, env, error::Error, future::Future, pin::Pin, process::Output, sync::Arc,
};
use tokio::{
    io::{AsyncWriteExt, stderr, stdout},
    spawn,
    sync::{Mutex, Semaphore},
    time::Instant,
    try_join,
};
use tracing::{Instrument, info, info_span};

// #[derive(Clone)]
pub struct Configuration {
    graph: BuildGraph,
}

impl Configuration {
    pub fn new(graph: BuildGraph) -> Self {
        Self { graph }
    }

    pub fn graph(&self) -> &BuildGraph {
        &self.graph
    }
}

type RawBuildFuture = Pin<Box<dyn Future<Output = Result<(), ApplicationError>> + Send>>;
pub(crate) type BuildFuture = Shared<RawBuildFuture>;

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ProgressMode {
    Disabled,
    PerTarget,
    Aggregate,
}

fn terminal_rows() -> usize {
    env::var("LINES")
        .ok()
        .and_then(|lines| lines.parse::<usize>().ok())
        .filter(|rows| *rows > 0)
        .unwrap_or(24) // all terminals are 24 rows, right?
}

fn select_progress_mode(progress: bool, target_count: usize) -> ProgressMode {
    if !progress {
        ProgressMode::Disabled
    } else if terminal_rows() < target_count {
        ProgressMode::Aggregate
    } else {
        ProgressMode::PerTarget
    }
}

/// Helper function to get final sink target filenames for a given node index.
/// This traces through outgoing edges until it reaches Sink nodes and returns the named
/// files written there, ignoring intermediate debug artifacts.
fn get_target_files(context: &Context, index: NodeIndex) -> Vec<String> {
    let mut targets = vec![];
    let mut to_visit = vec![index];
    let mut visited = HashSet::new();

    while let Some(current) = to_visit.pop() {
        if visited.contains(&current) {
            continue;
        }
        visited.insert(current);

        for edge in context
            .configuration
            .graph()
            .edges_directed(current, Direction::Outgoing)
        {
            if let Some(node_weight) = context.configuration.graph().node_weight(edge.target())
                && node_weight.shortname() == "Sink"
                && let Ok(output_lock) = edge.weight().output.lock()
                && let crate::buildsystem::output::RawOperationOutput::NamedFile(name) =
                    &*output_lock
            {
                targets.push(name.clone());
                continue;
            }
            to_visit.push(edge.target());
        }
    }

    targets.sort();
    targets.dedup();
    targets
}

pub async fn run(
    graph: BuildGraph,
    job_limit: usize,
    progress: bool,
) -> Result<(), ApplicationError> {
    let target_count = graph.target_nodes.len();
    let progress_mode = select_progress_mode(progress, target_count);
    let configuration = Configuration::new(graph);
    let context = Arc::new(Context::new(
        job_limit,
        Arc::new(configuration),
        progress,
        progress_mode,
        target_count,
    ));
    let mut target_futures = Vec::with_capacity(target_count);

    for (name, target_node) in &context.configuration.graph().target_nodes {
        trigger_build(context.clone(), *target_node).await?;
        if matches!(context.progress_mode, ProgressMode::PerTarget) {
            context.add_progressbar(*target_node, name);
        }

        let build_future = context
            .build_futures
            .get(target_node)
            .ok_or(ApplicationError::Build)?
            .value()
            .clone();

        target_futures.push((name.clone(), build_future));
    }

    let result = match context.progress_mode {
        ProgressMode::Aggregate => {
            try_join_all(target_futures.into_iter().map(|(name, build_future)| {
                let context = context.clone();
                async move {
                    build_future.await?;
                    context.finish_target_progress(&name);
                    Ok::<(), ApplicationError>(())
                }
            }))
            .await
        }
        ProgressMode::Disabled | ProgressMode::PerTarget => {
            try_join_all(
                target_futures
                    .into_iter()
                    .map(|(_, build_future)| build_future),
            )
            .await
        }
    };

    result.map(|_| ())
}

#[async_recursion]
async fn trigger_build(context: Arc<Context>, build: NodeIndex) -> Result<(), ApplicationError> {
    let targets = get_target_files(&context, build);
    let targets_str = targets.join(", ");
    let span = info_span!("trigger_build", targets = %targets_str);

    context.build_futures.entry(build).or_insert_with(|| {
        let context_clone = context.clone();
        let future: RawBuildFuture =
            Box::pin(spawn_build(context_clone, build).instrument(span.clone()));
        future.shared()
    });

    Ok(())
}

async fn spawn_build(context: Arc<Context>, index: NodeIndex) -> Result<(), ApplicationError> {
    spawn(async move {
        let targets = get_target_files(&context, index);
        let targets_str = targets.join(", ");
        let span = info_span!("Building",
            operation = %context.configuration.graph().node_weight(index).map(|op| op.shortname()).unwrap_or("unknown"),
            targets = %targets_str
        );

        async {
            let build = context
                .configuration
                .graph()
                .node_weight(index)
                .expect("Build step not found in graph");
            let mut futures = vec![];

            // Make sure we have all our dependencies. (in-edges of this index)
            let in_edges: Vec<_> = context
                .configuration
                .graph()
                .edges_directed(index, Direction::Incoming)
                .collect();
            // Collect inputs by slot, similar to how we handle outputs
            let max_input_slot = in_edges.iter().map(|e| e.weight().output_slot).max().unwrap_or(0);
            let mut input_files = vec![None; max_input_slot + 1];
            for edge in &in_edges {
                let slot = edge.weight().output_slot;
                if input_files[slot].is_none() {
                    input_files[slot] = Some(edge.weight().output.clone());
                }
            }
            // Convert to non-Option vec (all slots should be filled)
            let input_files: Vec<OperationOutput> = input_files.into_iter().flatten().collect();
            // Collect outputs by slot. Multiple edges may reference the same slot (broadcasting).
            // We need to build a Vec where outputs[slot] contains the OperationOutput for that slot.
            let out_edges: Vec<_> = context
                .configuration
                .graph()
                .edges_directed(index, Direction::Outgoing)
                .collect();
            // Find the maximum slot number to size our output vector
            let max_slot = out_edges.iter().map(|e| e.weight().output_slot).max().unwrap_or(0);
            let mut output_files = vec![None; max_slot + 1];
            // Fill in the output slots - if multiple edges use the same slot, they share the same OperationOutput
            for edge in out_edges {
                let slot = edge.weight().output_slot;
                if output_files[slot].is_none() {
                    output_files[slot] = Some(edge.weight().output.clone());
                }
            }

            // Convert to non-Option vec (all slots should be filled)
            let output_files: Vec<OperationOutput> = output_files.into_iter().flatten().collect();
            // Build all input dependencies
            for edge in &in_edges {
                futures.push(build_input(context.clone(), edge.source()).await?);
            }
            try_join_all(futures).await?;

            // OK, we are ready.
            run_op(&context, build, &input_files, &output_files, &targets).await?;

            if matches!(context.progress_mode, ProgressMode::PerTarget) {
                let op_desc = build.shortname().to_string();
                for edge in context
                    .configuration
                    .graph()
                    .edges_directed(index, Direction::Outgoing)
                {
                    context.step_progressbar(edge, &op_desc);
                }
            }

            Ok::<(), ApplicationError>(())
        }
        .instrument(span)
        .await
    })
    .await?
}

async fn build_input(
    context: Arc<Context>,
    input: NodeIndex,
) -> Result<BuildFuture, ApplicationError> {
    trigger_build(context.clone(), input).await?;
    context
        .build_futures
        .get(&input)
        .ok_or(ApplicationError::Build)
        .map(|f| f.clone())
        .map_err(|_| ApplicationError::Build)
}

async fn run_op(
    context: &Context,
    op: &BuildStep,
    inputs: &[OperationOutput],
    outputs: &[OperationOutput],
    final_targets: &[String],
) -> Result<(), ApplicationError> {
    let input_strs: Vec<String> = inputs.iter().map(|o| o.to_string()).collect();
    let inputs_str = input_strs.join(", ");
    let output_strs: Vec<String> = outputs.iter().map(|o| o.to_string()).collect();
    let outputs_str = output_strs.join(", ");

    let span = info_span!(
        "run_op",
        operation = %op.shortname(),
        targets = %outputs_str
    );

    let description = format!("{}: {} -> {}", op.description(), inputs_str, outputs_str,);

    let target_summary = if final_targets.is_empty() {
        outputs_str.clone()
    } else {
        final_targets.join(", ")
    };

    let failure_context = format!(
        "operation '{}' while building [{}] from [{}]",
        op.shortname(),
        target_summary,
        inputs_str
    );

    let inner = async {
        let ((output, duration), _console) = try_join!(
            async {
                let start_time = Instant::now();
                if !inputs.is_empty() && !outputs.is_empty() && !op.hidden() {
                    context.print_description(&description).await;
                }
                let output = context
                    .run_with_semaphore(|| op.execute(inputs, outputs))
                    .await
                    .map_err(|e| ApplicationError::Other(format!("{}: {}", failure_context, e)))?;

                let elapsed = Instant::now() - start_time;
                Ok::<_, ApplicationError>((output, elapsed))
            },
            async {
                let console = context.console().lock().await;
                // if !inputs.is_empty() && !outputs.is_empty() && !op.hidden() {
                //     stderr()
                //         .write_all(format!("Completed {}\n", &description).as_bytes())
                //         .await?;
                // }
                // debug!(context, console, "command: {}", rule.command());

                Ok(console)
            }
        )?;

        // Emit profiling event with duration for trace analysis
        info!(
            duration_ms = duration.as_millis() as u64,
            "Operation completed: {}", &description
        );

        if !output.status.success() {
            stdout().write_all(&output.stdout).await?;
            stderr().write_all(&output.stderr).await?;
            return Err(ApplicationError::Other(format!(
                "{}: process exited with status {}",
                failure_context, output.status
            )));
        }

        Ok::<(), ApplicationError>(())
    };

    inner.instrument(span).await
}

pub struct Context {
    command_semaphore: Semaphore,
    /// Just a thing that you lock to print to the console.
    console: Mutex<()>,
    pub configuration: Arc<Configuration>,
    pub build_futures: DashMap<NodeIndex, BuildFuture>,
    pub progress: bool,
    progress_mode: ProgressMode,
    pub progressbars: MultiProgress,
    aggregate_progress_bar: Option<indicatif::ProgressBar>,
    pub progress_bar_for_target: DashMap<NodeIndex, indicatif::ProgressBar>,
    pub edges_to_final_target_nodes: DashMap<EdgeIndex, Vec<NodeIndex>>,
}

impl Context {
    pub(crate) fn new(
        job_limit: usize,
        configuration: Arc<Configuration>,
        progress: bool,
        progress_mode: ProgressMode,
        total_targets: usize,
    ) -> Self {
        let progressbars = MultiProgress::new();
        let aggregate_progress_bar = if matches!(progress_mode, ProgressMode::Aggregate) {
            let sty = ProgressStyle::with_template(
                "{prefix:40!} {wide_bar:.cyan/blue} {pos}/{len} {msg:10!}",
            )
            .unwrap();

            let pb = progressbars
                .add(ProgressBar::new(total_targets as u64))
                .with_finish(indicatif::ProgressFinish::Abandon);
            pb.set_style(sty);
            pb.set_prefix("targets".to_string());

            Some(pb)
        } else {
            None
        };

        Self {
            command_semaphore: Semaphore::new(job_limit),
            console: Mutex::new(()),
            configuration,
            build_futures: DashMap::new(),
            progress,
            progress_mode,
            progressbars,
            aggregate_progress_bar,
            progress_bar_for_target: DashMap::new(),
            edges_to_final_target_nodes: DashMap::new(),
        }
    }

    pub fn console(&self) -> &Mutex<()> {
        &self.console
    }

    pub fn add_progressbar(&self, target: NodeIndex, name: &str) {
        if !matches!(self.progress_mode, ProgressMode::PerTarget) {
            return;
        }

        // Walk the graph backwards to count steps for this target
        let mut steps = 0;
        let mut node = target;
        let basename = name.rsplit('/').next().unwrap_or(name);
        let graph = self.configuration.graph();
        // Check for edges outgoing too
        if let Some(edge) = graph.edges_directed(node, Direction::Outgoing).next() {
            self.edges_to_final_target_nodes
                .entry(edge.id())
                .or_default()
                .push(target);
        }

        while let Some(edge) = graph.edges_directed(node, Direction::Incoming).next() {
            steps += 1;
            node = edge.source();
            self.edges_to_final_target_nodes
                .entry(edge.id())
                .or_default()
                .push(target);
        }
        let sty =
            ProgressStyle::with_template("{prefix:40!} {wide_bar:.cyan/blue} {msg:10!}").unwrap();

        let pb = self
            .progressbars
            .add(ProgressBar::new(steps as u64))
            .with_finish(indicatif::ProgressFinish::Abandon);
        pb.set_style(sty);
        pb.set_prefix(basename.to_string());
        self.progress_bar_for_target.insert(target, pb);
    }

    pub fn step_progressbar(&self, op_step: EdgeReference<BuildEdge>, op_desc: &str) {
        if !matches!(self.progress_mode, ProgressMode::PerTarget) {
            return;
        }

        // Find the final target(s) for this op, and increment it
        let target_nodes = self
            .edges_to_final_target_nodes
            .get(&op_step.id())
            .map(|entry| entry.value().clone())
            .unwrap_or_default();

        for target_node in target_nodes {
            if let Some(pb) = self.progress_bar_for_target.get(&target_node) {
                pb.set_message(op_desc.to_string());
                pb.inc(1);
            }
        }
    }

    pub fn finish_target_progress(&self, name: &str) {
        if let Some(pb) = &self.aggregate_progress_bar {
            pb.set_message(name.rsplit('/').next().unwrap_or(name).to_string());
            pb.inc(1);
            if pb.position() >= pb.length().unwrap_or_default() {
                pb.finish();
            }
        }
    }

    pub async fn run_with_semaphore(
        &self,
        fnc: impl Fn() -> Result<Output, ApplicationError>,
    ) -> Result<Output, Box<dyn Error>> {
        let permit = self.command_semaphore.acquire().await?;
        let output = fnc()?;

        drop(permit);

        Ok(output)
    }

    pub async fn print_description(&self, description: &str) {
        if self.progress {
            let _ = self.progressbars.println(description);
        } else {
            let _console_lock = self.console.lock().await;
            println!("{description}");
        }
    }
}
