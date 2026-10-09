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
#[derive(Clone)]
pub struct BuildEdge {
    /// The actual data/file being passed
    pub output: OperationOutput,
    /// Which output slot of the source operation this edge reads from (0-indexed)
    pub from_slot: usize,
    /// Which input slot of the destination operation this edge feeds (0-indexed)
    pub to_slot: usize,
}

impl Display for BuildEdge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}->{}:{}", self.from_slot, self.to_slot, self.output)
    }
}

pub struct BuildGraph {
    graph: Graph<Arc<Box<dyn Operation + 'static>>, BuildEdge>,
    debug_intermediates: bool,
    pub source: NodeIndex,
    pub sinks: Vec<NodeIndex>,
    /// Maps target names to the node that finally produces their artifact (the
    /// "owner"). This is *derived* by [`BuildGraph::resolve_dependencies`] and is
    /// what the orchestrator iterates over to know what to build.
    pub(crate) target_nodes: std::collections::HashMap<String, NodeIndex>,
    /// The last operation node on each target's own path, before any fusing
    /// operation takes ownership of its artifact.
    terminals: std::collections::HashMap<String, NodeIndex>,
    /// The sink node that writes each target's file.
    sink_nodes: std::collections::HashMap<String, NodeIndex>,
    /// Synthetic `Source` nodes for input files that aren't produced by any
    /// recipe target, keyed by path so they're shared between consumers.
    external_sources: std::collections::HashMap<String, NodeIndex>,
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
            terminals: std::collections::HashMap::new(),
            sink_nodes: std::collections::HashMap::new(),
            external_sources: std::collections::HashMap::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
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
                            if let Some(node_op) = self.graph.node_weight(edge.target())
                                && node_op.shortname() == conv_op.shortname()
                            {
                                return !started_at_source
                                    || edge.weight().output.value_eq(&computed_output);
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
                                from_slot: 0,
                                to_slot: 0,
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
                from_slot: 0,
                to_slot: 0,
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
            .map(|e| (e.target(), e.weight().from_slot, e.weight().to_slot))
            .collect();
        for (target, from_slot, to_slot) in outgoing {
            let edge = BuildEdge {
                output: final_output.clone(),
                from_slot,
                to_slot,
            };
            self.graph.update_edge(current_node, target, edge);
        }

        // Create a sink node and add it to the list of sinks
        let sink_node = self.graph.add_node(Arc::new(Box::new(SourceSink::Sink)));
        let edge = BuildEdge {
            output: final_output,
            from_slot: 0,
            to_slot: 0,
        };
        self.graph.update_edge(current_node, sink_node, edge);
        self.sinks.push(sink_node);

        // Note this target's terminal node and sink; ownership is worked out later
        // in `resolve_dependencies`, once every path has been materialised.
        self.terminals
            .insert(sink_filename.to_string(), current_node);
        self.sink_nodes.insert(sink_filename.to_string(), sink_node);

        AddedPath {
            entry_node: entry_node.unwrap_or(sink_node),
            op_nodes,
        }
    }

    /// Wire up all the cross-references (`needs` entries and `source:` steps)
    /// once every target's linear path has been materialised.
    ///
    /// This runs in phases so that it is order-independent and idempotent:
    ///
    /// 1. Work out the *owner* of each target's artifact. By default that's the
    ///    last node of the target's own path; a fusing operation
    ///    ([`Operation::fuses_targets`]) takes ownership of the targets it
    ///    re-emits.
    /// 2. Add a fan-in edge for each `needs` entry. A fusing operation reads the
    ///    *pre-fusion* artifact of the targets it re-emits; every other consumer
    ///    reads the target's final (owned) artifact.
    /// 3. Move the sink of each fused target so its file is written by the owner.
    /// 4. Rewire `source:` steps naming another target to read that target's
    ///    owned artifact.
    /// 5. Record the final owners in `target_nodes` for the orchestrator.
    pub(crate) fn resolve_dependencies(
        &mut self,
        dependencies: &[(NodeIndex, Vec<String>, bool)],
        source_dependencies: &[(NodeIndex, String)],
    ) -> Result<(), ApplicationError> {
        // Phase 1: work out ownership.
        let mut owner: std::collections::HashMap<String, (NodeIndex, usize)> = self
            .terminals
            .iter()
            .map(|(name, node)| (name.clone(), (*node, 0)))
            .collect();
        for (node, needs, fuses) in dependencies {
            if !*fuses {
                continue;
            }
            for (index, need) in needs.iter().enumerate() {
                if self.terminals.contains_key(need) {
                    owner.insert(need.clone(), (*node, index + 1));
                }
            }
        }

        // Phase 2: add the fan-in edges for every `needs` entry.
        for (node, needs, fuses) in dependencies {
            for (index, need) in needs.iter().enumerate() {
                let (source_node, from_slot) = if *fuses {
                    // A fusing operation reads the pre-fusion artifact of each
                    // target it re-emits, so resolve to that target's terminal.
                    match self.terminals.get(need) {
                        Some(terminal) => (*terminal, 0),
                        None => (self.external_source(need)?, 0),
                    }
                } else {
                    // Every other consumer reads the target's final artifact.
                    match owner.get(need) {
                        Some((owner_node, owner_slot)) => (*owner_node, *owner_slot),
                        None => (self.external_source(need)?, 0),
                    }
                };
                self.graph.update_edge(
                    source_node,
                    *node,
                    BuildEdge {
                        output: RawOperationOutput::from(need.as_str()).into(),
                        from_slot,
                        to_slot: index + 1,
                    },
                );
            }
        }

        // Phase 3: move fused targets' sinks so the owner writes their files.
        for (name, (owner_node, owner_slot)) in &owner {
            let Some(terminal) = self.terminals.get(name).copied() else {
                continue;
            };
            if terminal == *owner_node {
                continue;
            }
            let Some(sink_node) = self.sink_nodes.get(name).copied() else {
                continue;
            };
            if let Some(edge_idx) = self.graph.find_edge(terminal, sink_node) {
                self.graph.remove_edge(edge_idx);
            }
            self.graph.update_edge(
                *owner_node,
                sink_node,
                BuildEdge {
                    output: RawOperationOutput::from(name.as_str()).into(),
                    from_slot: *owner_slot,
                    to_slot: 0,
                },
            );
        }

        // Phase 4: `source:` steps that name another target.
        for (entry_node, source_name) in source_dependencies {
            let Some((owner_node, owner_slot)) = owner.get(source_name) else {
                // An external file: leave the edge from the global Source node.
                continue;
            };
            if let Some(edge_idx) = self.graph.find_edge(self.source, *entry_node) {
                self.graph.remove_edge(edge_idx);
            }
            self.graph.update_edge(
                *owner_node,
                *entry_node,
                BuildEdge {
                    output: RawOperationOutput::from(source_name.as_str()).into(),
                    from_slot: *owner_slot,
                    to_slot: 0,
                },
            );
        }

        // Phase 5: derived producer map for the orchestrator.
        self.target_nodes = owner
            .into_iter()
            .map(|(name, (node, _slot))| (name, node))
            .collect();

        Ok(())
    }

    /// Return (creating if necessary) the synthetic `Source` node that feeds an
    /// input file which isn't produced by any recipe target. Reusing one node per
    /// file keeps repeated references idempotent and deterministic.
    fn external_source(&mut self, name: &str) -> Result<NodeIndex, ApplicationError> {
        if let Some(node) = self.external_sources.get(name) {
            return Ok(*node);
        }
        if !Path::new(name).exists() {
            return Err(ApplicationError::InvalidRecipe(format!(
                "Dependency target '{}' not found. Make sure it appears in the recipe before it's referenced.",
                name
            )));
        }
        let node = self.graph.add_node(Arc::new(Box::new(SourceSink::Source)));
        self.external_sources.insert(name.to_string(), node);
        Ok(node)
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
