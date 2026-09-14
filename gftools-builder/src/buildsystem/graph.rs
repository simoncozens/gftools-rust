use std::{fmt::Display, path::Path, sync::Arc};

use petgraph::{Graph, graph::NodeIndex, visit::EdgeRef};

use crate::{
    buildsystem::{Operation, OperationOutput, output::RawOperationOutput, sourcesink::SourceSink},
    error::ApplicationError,
    operations::convert::{FileToBytes, PathToSourceFont},
};

pub type BuildStep = Arc<Box<dyn Operation>>;

pub struct AddedPath {
    pub entry_node: NodeIndex,
    pub op_nodes: Vec<NodeIndex>,
}

/// An edge in the build graph, representing data flow from one operation to another.
/// The edge specifies which output slot from the source operation it consumes.
#[derive(Clone)]
pub struct BuildEdge {
    /// The actual data/file being passed
    pub output: OperationOutput,
    /// Which output slot from the source operation (0-indexed)
    pub output_slot: usize,
}

impl Display for BuildEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.output_slot, self.output)
    }
}

pub struct BuildGraph {
    graph: Graph<Arc<Box<dyn Operation + 'static>>, BuildEdge>,
    debug_intermediates: bool,
    pub source: NodeIndex,
    pub sinks: Vec<NodeIndex>,
    /// Maps target names to their final operation node (before the sink)
    pub(crate) target_nodes: std::collections::HashMap<String, NodeIndex>,
}

impl BuildGraph {
    pub fn new(debug_intermediates: bool) -> Self {
        let mut g = Graph::new();
        let source_node: Box<dyn Operation + 'static> = Box::new(SourceSink::Source);
        let source = g.add_node(Arc::new(source_node));
        let sinks = vec![];
        Self {
            graph: g,
            debug_intermediates,
            source,
            sinks,
            target_nodes: std::collections::HashMap::new(),
        }
    }

    fn sanitize_debug_component(component: &str) -> String {
        component
            .chars()
            .map(|ch| match ch {
                'a'..='z' | 'A'..='Z' | '0'..='9' => ch,
                _ => '-',
            })
            .collect::<String>()
            .trim_matches('-')
            .to_ascii_lowercase()
    }

    fn debug_filename(
        &self,
        source_filename: &str,
        sink_filename: &str,
        op_chain: &[String],
        kind: crate::buildsystem::DataKind,
    ) -> String {
        let sink_path = Path::new(sink_filename);
        let directory = Path::new("debug-build");
        let source_name = Path::new(source_filename)
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "intermediate".to_string());
        let extension = match kind {
            crate::buildsystem::DataKind::SourceFont => Path::new(source_filename)
                .extension()
                .map(|ext| ext.to_string_lossy().to_string())
                .unwrap_or_else(|| "glyphs".to_string()),
            crate::buildsystem::DataKind::Path
            | crate::buildsystem::DataKind::Bytes
            | crate::buildsystem::DataKind::BinaryFont
            | crate::buildsystem::DataKind::Any => sink_path
                .extension()
                .map(|ext| ext.to_string_lossy().to_string())
                .or_else(|| {
                    Path::new(source_filename)
                        .extension()
                        .map(|ext| ext.to_string_lossy().to_string())
                })
                .unwrap_or_else(|| "bin".to_string()),
        };
        let chain = op_chain
            .iter()
            .map(|component| Self::sanitize_debug_component(component))
            .collect::<Vec<_>>()
            .join("-");
        directory
            .join(format!("{source_name}-{chain}.{extension}"))
            .to_string_lossy()
            .to_string()
    }

    fn default_output_for_kind(
        &self,
        source_filename: &str,
        sink_filename: &str,
        op_chain: &[String],
        kind: crate::buildsystem::DataKind,
    ) -> OperationOutput {
        if self.debug_intermediates && !op_chain.is_empty() {
            return RawOperationOutput::from(
                self.debug_filename(source_filename, sink_filename, op_chain, kind)
                    .as_str(),
            )
            .into();
        }

        match kind {
            crate::buildsystem::DataKind::Path => RawOperationOutput::TemporaryFile(None).into(),
            crate::buildsystem::DataKind::Bytes
            | crate::buildsystem::DataKind::BinaryFont
            | crate::buildsystem::DataKind::Any
            | crate::buildsystem::DataKind::SourceFont => {
                RawOperationOutput::InMemoryBytes(Vec::new()).into()
            }
        }
    }

    pub fn externals(&self, direction: petgraph::Direction) -> impl Iterator<Item = NodeIndex> {
        self.graph.externals(direction)
    }
    pub fn node_weight(&self, index: NodeIndex) -> Option<&BuildStep> {
        self.graph.node_weight(index)
    }
    pub fn edges_directed(
        &'_ self,
        index: NodeIndex,
        direction: petgraph::Direction,
    ) -> impl Iterator<Item = petgraph::graph::EdgeReference<'_, BuildEdge>> {
        self.graph.edges_directed(index, direction)
    }

    pub fn add_path<S: AsRef<str>>(
        &mut self,
        source_filename: &str,
        operations: Vec<(Option<S>, BuildStep)>,
        sink_filename: &str,
    ) -> AddedPath {
        use crate::buildsystem::operation::DataKind;
        let mut current_node = self.source;
        let mut current_kind: DataKind = DataKind::Path;
        let mut op_nodes: Vec<NodeIndex> = Vec::new();
        let mut entry_node: Option<NodeIndex> = None;
        let mut debug_chain: Vec<String> = Vec::new();

        for (index, (input_filename, op)) in operations.into_iter().enumerate() {
            let computed_output: OperationOutput = if let Some(input_filename) = input_filename {
                RawOperationOutput::from(input_filename.as_ref()).into()
            } else if index == 0 {
                RawOperationOutput::from(source_filename).into()
            } else {
                self.default_output_for_kind(
                    source_filename,
                    sink_filename,
                    &debug_chain,
                    current_kind,
                )
            };

            let started_at_source = current_node == self.source;

            let mut broadcast_output = if started_at_source {
                computed_output.clone()
            } else {
                self.graph
                    .edges_directed(current_node, petgraph::Direction::Outgoing)
                    .next()
                    .map(|edge| edge.weight().output.clone())
                    .unwrap_or_else(|| computed_output.clone())
            };

            let want_kind = op.input_kinds().first().cloned().unwrap_or(DataKind::Any);
            let need_conversion = !(want_kind == DataKind::Any || want_kind == current_kind);
            if need_conversion {
                let conv: Option<(Box<dyn Operation>, DataKind)> = match (current_kind, want_kind) {
                    (DataKind::Path, DataKind::Bytes) => {
                        Some((Box::new(FileToBytes), DataKind::Bytes))
                    }
                    (DataKind::Path, DataKind::SourceFont) => {
                        Some((Box::new(PathToSourceFont), DataKind::SourceFont))
                    }
                    _ => None,
                };
                if let Some((conv_op, new_kind)) = conv {
                    let conv_shortname = conv_op.identifier();
                    let existing_conv = self
                        .graph
                        .edges_directed(current_node, petgraph::Direction::Outgoing)
                        .find(|edge| {
                            if let Some(node_op) = self.graph.node_weight(edge.target()) {
                                if node_op.shortname() == conv_op.shortname() {
                                    return !started_at_source
                                        || edge.weight().output.value_eq(&computed_output);
                                }
                            }
                            false
                        })
                        .map(|edge| edge.target());

                    let conv_node = if let Some(existing) = existing_conv {
                        existing
                    } else {
                        let new_conv_node = self.graph.add_node(Arc::new(conv_op));
                        self.graph.update_edge(
                            current_node,
                            new_conv_node,
                            BuildEdge {
                                output: broadcast_output.clone(),
                                output_slot: 0,
                            },
                        );
                        new_conv_node
                    };

                    if entry_node.is_none() && current_node == self.source {
                        entry_node = Some(conv_node);
                    }

                    current_node = conv_node;
                    current_kind = new_kind;
                    debug_chain.push(conv_shortname);
                    broadcast_output = self
                        .graph
                        .edges_directed(current_node, petgraph::Direction::Outgoing)
                        .next()
                        .map(|edge| edge.weight().output.clone())
                        .unwrap_or_else(|| {
                            self.default_output_for_kind(
                                source_filename,
                                sink_filename,
                                &debug_chain,
                                current_kind,
                            )
                        });
                }
            }

            let op_shortname = op.identifier();
            let is_source = current_node == self.source;

            if let Some(existing_node) = self
                .graph
                .edges_directed(current_node, petgraph::Direction::Outgoing)
                .find(|edge| {
                    let target_op = &self.graph[edge.target()];
                    let same_op = target_op.identifier() == op.identifier();
                    if is_source {
                        same_op && edge.weight().output.value_eq(&computed_output)
                    } else {
                        same_op
                    }
                })
                .map(|edge| edge.target())
            {
                current_node = existing_node;
                op_nodes.push(existing_node);
                debug_chain.push(op_shortname);

                if entry_node.is_none() && current_node == self.source {
                    entry_node = Some(existing_node);
                }

                if let Some(ok) = self
                    .graph
                    .node_weight(existing_node)
                    .and_then(|op| op.output_kinds().first().cloned())
                    && ok != DataKind::Any
                {
                    current_kind = ok;
                }

                continue;
            }

            let next_node = self.graph.add_node(op);
            let edge = BuildEdge {
                output: broadcast_output,
                output_slot: 0,
            };
            self.graph.update_edge(current_node, next_node, edge);

            if entry_node.is_none() && current_node == self.source {
                entry_node = Some(next_node);
            }

            current_node = next_node;
            op_nodes.push(next_node);
            debug_chain.push(op_shortname);

            if let Some(ok) = self
                .graph
                .node_weight(current_node)
                .and_then(|op| op.output_kinds().first().cloned())
                && ok != DataKind::Any
            {
                current_kind = ok;
            }
        }

        // When adding a sink edge, force the output to be the named target file
        // and broadcast that same OperationOutput to all existing outgoing edges.
        let final_output: OperationOutput = RawOperationOutput::from(sink_filename).into();

        // If there are existing outgoing edges, update them to use the named output
        // so downstream consumers see the real target filename (not a temp file).
        let outgoing: Vec<_> = self
            .graph
            .edges_directed(current_node, petgraph::Direction::Outgoing)
            .map(|e| (e.target(), e.weight().output_slot))
            .collect();
        for (target, slot) in outgoing {
            let edge = BuildEdge {
                output: final_output.clone(),
                output_slot: slot,
            };
            self.graph.update_edge(current_node, target, edge);
        }

        // Create a sink node and add it to the list of sinks, using slot 0
        let sink_node = self.graph.add_node(Arc::new(Box::new(SourceSink::Sink)));
        let edge = BuildEdge {
            output: final_output,
            output_slot: 0,
        };
        self.graph.update_edge(current_node, sink_node, edge);
        self.sinks.push(sink_node);

        // Track this target's final node (before sink) for dependency resolution
        self.target_nodes
            .insert(sink_filename.to_string(), current_node);

        AddedPath {
            entry_node: entry_node.unwrap_or(sink_node),
            op_nodes,
        }
    }

    /// Add a dependency from a target to a node that needs it as an additional input.
    /// This creates edges from the dependency target's producing node to the dependent node.
    /// For operations that need n inputs and produce n outputs (like BuildStat), this also
    /// redirects the dependency's sink to go through the dependent node.
    ///
    /// # Arguments
    /// * `target_name` - The name of the target that produces the needed file
    /// * `dependent_node` - The node that needs the target as an additional input
    /// * `input_slot` - Which input slot (0-indexed) this dependency fills
    pub fn add_dependency(
        &mut self,
        target_name: &str,
        dependent_node: NodeIndex,
        input_slot: usize,
    ) -> Result<(), ApplicationError> {
        let maybe_node = self.target_nodes.get(target_name);
        if maybe_node.is_none() && std::path::Path::new(target_name).exists() {
            // For existing files, we don't need a producer. Just add a source node for it
            let source_node = self.graph.add_node(Arc::new(Box::new(SourceSink::Source)));
            let edge = BuildEdge {
                output: RawOperationOutput::from(target_name).into(),
                output_slot: input_slot,
            };
            self.graph.update_edge(source_node, dependent_node, edge);
            self.target_nodes
                .insert(target_name.to_string(), source_node);
            return Ok(());
        }
        let producer_node = maybe_node.ok_or_else(|| {
            ApplicationError::InvalidRecipe(format!(
                "Dependency target '{}' not found. Make sure it appears in the recipe before it's referenced.",
                target_name
            ))
        })?;

        // A dependency can be requested multiple times when different recipe targets
        // share the same operation node (e.g. VF target + VF webfont target sharing
        // BuildStat). Once target_nodes has been updated to point at dependent_node,
        // a duplicate call would try to wire dependent_node -> dependent_node,
        // introducing a self-cycle. Treat this as already satisfied.
        if *producer_node == dependent_node {
            return Ok(());
        }

        // Get the specific output that produces this target.
        let (producer_output, _producer_output_slot) = self
            .graph
            .edges_directed(*producer_node, petgraph::Direction::Outgoing)
            .find_map(|edge| {
                edge.weight()
                    .output
                    .lock()
                    .ok()
                    .and_then(|output| match &*output {
                        RawOperationOutput::NamedFile(name) if name == target_name => {
                            Some((edge.weight().output.clone(), edge.weight().output_slot))
                        }
                        _ => None,
                    })
            })
            .unwrap_or_else(|| (RawOperationOutput::from(target_name).into(), 0));

        // Add input edge from producer to dependent, specifying the input slot
        let input_edge = BuildEdge {
            output: producer_output,
            output_slot: input_slot,
        };
        self.graph
            .update_edge(*producer_node, dependent_node, input_edge);

        // Find the sink node for this dependency target and redirect it through dependent_node
        // We need to find the sink that's writing to this specific target file
        let sink_edges: Vec<_> = self
            .graph
            .edges_directed(*producer_node, petgraph::Direction::Outgoing)
            .filter(|edge| {
                // Check if this edge goes to a Sink node AND has the matching target output
                if let Some(node_weight) = self.graph.node_weight(edge.target())
                    && node_weight.shortname() == "Sink"
                {
                    if let Ok(output) = edge.weight().output.lock()
                        && let RawOperationOutput::NamedFile(filename) = &*output
                    {
                        return filename == target_name;
                    }
                }
                false
            })
            .map(|e| (e.target(), e.weight().output.clone()))
            .collect();

        // For each sink, redirect it to go through the dependent node
        for (sink_node, output) in sink_edges {
            // Remove the edge from producer to sink
            if let Some(edge_idx) = self.graph.find_edge(*producer_node, sink_node) {
                self.graph.remove_edge(edge_idx);
            }

            // Add edge from dependent_node to sink using the same output slot as the input
            let output_edge = BuildEdge {
                output,
                output_slot: input_slot,
            };
            self.graph
                .update_edge(dependent_node, sink_node, output_edge);
        }

        if self.target_nodes.contains_key(target_name) {
            self.target_nodes
                .insert(target_name.to_string(), dependent_node);
        }

        Ok(())
    }

    pub fn add_source_dependency(
        &mut self,
        target_name: &str,
        dependent_node: NodeIndex,
    ) -> Result<(), ApplicationError> {
        let Some(producer_node) = self.target_nodes.get(target_name).copied() else {
            return Ok(());
        };

        let (producer_output, producer_output_slot) = self
            .graph
            .edges_directed(producer_node, petgraph::Direction::Outgoing)
            .find_map(|edge| {
                edge.weight()
                    .output
                    .lock()
                    .ok()
                    .and_then(|output| match &*output {
                        RawOperationOutput::NamedFile(name) if name == target_name => {
                            Some((edge.weight().output.clone(), edge.weight().output_slot))
                        }
                        _ => None,
                    })
            })
            .unwrap_or_else(|| (RawOperationOutput::from(target_name).into(), 0));

        if let Some(edge_idx) = self.graph.find_edge(self.source, dependent_node) {
            self.graph.remove_edge(edge_idx);
        }

        self.graph.update_edge(
            producer_node,
            dependent_node,
            BuildEdge {
                output: producer_output,
                output_slot: producer_output_slot,
            },
        );

        Ok(())
    }

    pub fn ensure_directories(&self) -> Result<(), ApplicationError> {
        for edge in self.graph.raw_edges() {
            if edge.weight.output.is_named_file()
                && let Some(parent) =
                    std::path::Path::new(&edge.weight.output.to_filename(None)?).parent()
            {
                std::fs::create_dir_all(parent).map_err(|e| {
                    ApplicationError::Other(format!(
                        "Could not create directory {}: {}",
                        parent.display(),
                        e
                    ))
                })?;
            }
        }
        Ok(())
    }

    #[cfg(feature = "graphviz")]
    pub fn draw(&self) -> Result<String, ApplicationError> {
        let contents = format!("{}", petgraph::dot::Dot::new(&self.graph));
        let mut parser = layout::gv::DotParser::new(&contents);
        let tree = parser
            .process()
            .map_err(|e| ApplicationError::Other(format!("Could not parse graph: {e}")))?;
        let mut gb = layout::gv::GraphBuilder::new();
        gb.visit_graph(&tree);
        let mut vg = gb.get();
        let mut svg = layout::backends::svg::SVGWriter::new();
        vg.do_it(false, false, false, &mut svg);
        let svg_contents = svg.finalize();
        Ok(svg_contents)
    }

    pub fn ascii(&self, verbosity: log::Level) -> Result<String, ApplicationError> {
        // In ascii_dag we can't put a label on an edge. To get around that,
        // we create another petgraph where as well as the original nodes,
        // each edge in self.graph becomes a node, and we add edges from
        // the source node to the edge node, and from the edge node to the
        // target node.
        let mut graph: Graph<String, ()> = Graph::new();
        // First let's copy what we need to know about the nodes
        for index in self.graph.node_indices() {
            let op = self.graph.node_weight(index).unwrap();
            graph.add_node(op.shortname().to_string());
        }
        // Now let's add nodes for the edges
        for edge in self.graph.raw_edges() {
            let edge_node = if verbosity >= log::Level::Debug {
                graph.add_node(format!("{:?}", edge.weight.output))
            } else {
                graph.add_node(format!("{}", edge.weight.output))
            };
            graph.add_edge(edge.source(), edge_node, ());
            graph.add_edge(edge_node, edge.target(), ());
        }

        // And now we can create the nodes and edges for ascii_dag.
        let nodes: Vec<(usize, &str)> = graph
            .node_indices()
            .map(|index| (index.index(), graph[index].as_str()))
            .collect();
        let edges: Vec<(usize, usize)> = graph
            .raw_edges()
            .iter()
            .map(|edge| (edge.source().index(), edge.target().index()))
            .collect();
        let dag = ascii_dag::DAG::from_edges(&nodes, &edges);
        let contents = dag.render();
        Ok(contents)
    }
}

impl Default for BuildGraph {
    fn default() -> Self {
        Self::new(false)
    }
}

impl OperationOutput {
    fn value_eq(&self, other: &Self) -> bool {
        if let (Ok(a), Ok(b)) = (self.lock(), other.lock()) {
            *a == *b
        } else {
            false
        }
    }
}
