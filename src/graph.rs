use std::collections::{HashMap, HashSet, VecDeque};

use crate::contracts::{GraphEdge, LinkKind, SourceDocument, SourceRef};

#[derive(Clone, Debug, Default)]
pub struct GraphSnapshot {
    pub nodes: Vec<SourceRef>,
    pub edges: Vec<GraphEdge>,
}

#[derive(Default)]
struct Resolver {
    paths: HashMap<(String, String), Vec<SourceRef>>,
    titles: HashMap<(String, String), Vec<SourceRef>>,
    aliases: HashMap<(String, String), Vec<SourceRef>>,
}

impl Resolver {
    fn from_documents(documents: &[SourceDocument]) -> Self {
        let mut resolver = Self::default();
        for document in documents {
            let source = document.source.clone();
            let path = normalize_relative(&source.relative_path);
            if let Some(path) = path {
                resolver.insert_path(&source.root_id, &path, &source);
                if let Some(without_extension) = strip_markdown_extension(&path) {
                    resolver.insert_path(&source.root_id, &without_extension, &source);
                }
            }

            if !document.title.trim().is_empty() {
                resolver.insert_title(&source.root_id, &normalize_key(&document.title), &source);
            }
            if let Some(stem) = note_stem(&source.relative_path) {
                resolver.insert_title(&source.root_id, &normalize_key(&stem), &source);
            }
            for alias in aliases(document) {
                resolver.insert_alias(&source.root_id, &normalize_key(&alias), &source);
            }
        }
        resolver
    }

    fn insert_path(&mut self, root_id: &str, key: &str, source: &SourceRef) {
        self.paths
            .entry((root_id.to_owned(), normalize_key(key)))
            .or_default()
            .push(source.clone());
    }

    fn insert_title(&mut self, root_id: &str, key: &str, source: &SourceRef) {
        self.titles
            .entry((root_id.to_owned(), key.to_owned()))
            .or_default()
            .push(source.clone());
    }

    fn insert_alias(&mut self, root_id: &str, key: &str, source: &SourceRef) {
        self.aliases
            .entry((root_id.to_owned(), key.to_owned()))
            .or_default()
            .push(source.clone());
    }

    fn lookup_unique(
        index: &HashMap<(String, String), Vec<SourceRef>>,
        root_id: &str,
        key: &str,
    ) -> Resolution {
        let Some(found) = index.get(&(root_id.to_owned(), normalize_key(key))) else {
            return Resolution::Missing;
        };
        let unique = found.iter().cloned().collect::<HashSet<_>>();
        match unique.len() {
            0 => Resolution::Missing,
            1 => Resolution::Found(unique.into_iter().next().expect("one source")),
            _ => Resolution::Ambiguous,
        }
    }
}

enum Resolution {
    Found(SourceRef),
    Missing,
    Ambiguous,
}

/// Derive only edges directly represented by parsed source links.
///
/// All resolution is scoped to the source's configured root. A target is
/// resolved by exact relative path first, then title/stem, then alias, and
/// only when that key maps to a single source. Missing or ambiguous targets
/// remain explicit unresolved edges.
pub fn derive_graph(documents: &[SourceDocument]) -> GraphSnapshot {
    let resolver = Resolver::from_documents(documents);
    let mut nodes = documents
        .iter()
        .map(|document| document.source.clone())
        .collect::<Vec<_>>();
    nodes.sort();
    nodes.dedup();

    let mut edges = Vec::new();
    for document in documents {
        for link in &document.links {
            let resolution = resolve_link(document, link.target.as_str(), link.kind, &resolver);
            let (to, unresolved_target) = match resolution {
                Resolution::Found(source) => (Some(source), None),
                Resolution::Missing | Resolution::Ambiguous => (
                    None,
                    Some(unresolved_label(
                        link.target.as_str(),
                        link.heading.as_deref(),
                    )),
                ),
            };
            edges.push(GraphEdge {
                from: document.source.clone(),
                to,
                unresolved_target,
                kind: link.kind,
                start_byte: link.start_byte,
                end_byte: link.end_byte,
            });
        }
    }
    edges.sort_by(|left, right| {
        left.from
            .cmp(&right.from)
            .then_with(|| left.start_byte.cmp(&right.start_byte))
            .then_with(|| left.end_byte.cmp(&right.end_byte))
            .then_with(|| left.to.cmp(&right.to))
            .then_with(|| left.unresolved_target.cmp(&right.unresolved_target))
    });

    GraphSnapshot { nodes, edges }
}

/// Return unique adjacent nodes up to `max_hops`, capped to 256 results.
/// Incoming edges are included so Obsidian backlinks are available to callers.
pub fn neighbors(
    graph: &GraphSnapshot,
    source: &SourceRef,
    max_hops: u8,
    max_nodes: usize,
) -> Vec<SourceRef> {
    let max_hops = max_hops.min(8);
    let max_nodes = max_nodes.min(256);
    if max_hops == 0 || max_nodes == 0 {
        return Vec::new();
    }

    let mut visited = HashSet::from([source.clone()]);
    let mut queue = VecDeque::from([(source.clone(), 0_u8)]);
    let mut result = Vec::new();

    while let Some((current, depth)) = queue.pop_front() {
        if depth >= max_hops {
            continue;
        }
        let mut adjacent = Vec::new();
        for edge in &graph.edges {
            if edge.from == current {
                if let Some(target) = &edge.to {
                    adjacent.push(target.clone());
                }
            }
            if edge.to.as_ref() == Some(&current) {
                adjacent.push(edge.from.clone());
            }
        }
        adjacent.sort();
        adjacent.dedup();

        for next in adjacent {
            if visited.insert(next.clone()) {
                result.push(next.clone());
                if result.len() >= max_nodes {
                    return result;
                }
                queue.push_back((next, depth + 1));
            }
        }
    }
    result
}

/// Return the source's direct explicit outgoing and incoming relations.
pub fn adjacent_edges(graph: &GraphSnapshot, source: &SourceRef, limit: usize) -> Vec<GraphEdge> {
    graph
        .edges
        .iter()
        .filter(|edge| edge.from == *source || edge.to.as_ref() == Some(source))
        .take(limit.min(512))
        .cloned()
        .collect()
}

fn resolve_link(
    document: &SourceDocument,
    raw_target: &str,
    kind: LinkKind,
    resolver: &Resolver,
) -> Resolution {
    let target = raw_target.trim();
    if target.is_empty() || target.starts_with('#') {
        return Resolution::Found(document.source.clone());
    }
    if is_external_target(target) || target.starts_with('/') {
        return Resolution::Missing;
    }

    let target_without_heading = target.split('#').next().unwrap_or(target).trim();
    let target_without_query = target_without_heading
        .split('?')
        .next()
        .unwrap_or(target_without_heading)
        .trim();
    if target_without_query.is_empty() {
        return Resolution::Found(document.source.clone());
    }

    if kind == LinkKind::Markdown {
        let parent = std::path::Path::new(&document.source.relative_path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new(""));
        if let Some(relative) = normalize_join(parent, target_without_query) {
            match Resolver::lookup_unique(&resolver.paths, &document.source.root_id, &relative) {
                Resolution::Found(source) => return Resolution::Found(source),
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::Missing => {
                    if let Some(without_extension) = strip_markdown_extension(&relative) {
                        match Resolver::lookup_unique(
                            &resolver.paths,
                            &document.source.root_id,
                            &without_extension,
                        ) {
                            Resolution::Found(source) => return Resolution::Found(source),
                            Resolution::Ambiguous => return Resolution::Ambiguous,
                            Resolution::Missing => {}
                        }
                    }
                }
            }
        }
    }

    let root_target = normalize_relative(target_without_query);
    if let Some(path) = root_target {
        match Resolver::lookup_unique(&resolver.paths, &document.source.root_id, &path) {
            Resolution::Found(source) => return Resolution::Found(source),
            Resolution::Ambiguous => return Resolution::Ambiguous,
            Resolution::Missing => {}
        }
        if let Some(without_extension) = strip_markdown_extension(&path) {
            match Resolver::lookup_unique(
                &resolver.paths,
                &document.source.root_id,
                &without_extension,
            ) {
                Resolution::Found(source) => return Resolution::Found(source),
                Resolution::Ambiguous => return Resolution::Ambiguous,
                Resolution::Missing => {}
            }
        }
    }

    let title_key = normalize_key(target_without_query);
    match Resolver::lookup_unique(&resolver.titles, &document.source.root_id, &title_key) {
        Resolution::Found(source) => return Resolution::Found(source),
        Resolution::Ambiguous => return Resolution::Ambiguous,
        Resolution::Missing => {}
    }
    match Resolver::lookup_unique(&resolver.aliases, &document.source.root_id, &title_key) {
        Resolution::Found(source) => Resolution::Found(source),
        Resolution::Ambiguous | Resolution::Missing => Resolution::Missing,
    }
}

fn aliases(document: &SourceDocument) -> Vec<String> {
    let Some(value) = document.frontmatter.get("aliases") else {
        return Vec::new();
    };
    match value {
        serde_json::Value::String(alias) => vec![alias.clone()],
        serde_json::Value::Array(values) => values
            .iter()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

fn note_stem(relative_path: &str) -> Option<String> {
    let name = std::path::Path::new(relative_path).file_name()?.to_str()?;
    let stem = strip_markdown_extension(name).unwrap_or_else(|| name.to_owned());
    (!stem.trim().is_empty()).then_some(stem)
}

fn normalize_join(base: &std::path::Path, target: &str) -> Option<String> {
    if target.starts_with('/') || is_external_target(target) {
        return None;
    }
    let combined = base.join(target.replace('\\', "/"));
    normalize_relative(&combined.to_string_lossy())
}

fn normalize_relative(path: &str) -> Option<String> {
    let replaced = path.trim().replace('\\', "/");
    if replaced.is_empty() || replaced.starts_with('/') || is_external_target(&replaced) {
        return None;
    }
    let mut components: Vec<&str> = Vec::new();
    for component in replaced.split('/') {
        match component {
            "" | "." => {}
            ".." => {
                components.pop()?;
            }
            value if value.contains(':') => return None,
            value => components.push(value),
        }
    }
    if components.is_empty() {
        None
    } else {
        Some(components.join("/"))
    }
}

fn strip_markdown_extension(path: &str) -> Option<String> {
    let path = path.trim();
    let (stem, extension) = path.rsplit_once('.')?;
    extension
        .eq_ignore_ascii_case("md")
        .then(|| stem.to_owned())
}

fn normalize_key(value: &str) -> String {
    value.trim().replace('\\', "/").to_lowercase()
}

fn is_external_target(target: &str) -> bool {
    let lower = target.trim().to_lowercase();
    lower.contains("://")
        || lower.starts_with("mailto:")
        || lower.starts_with("data:")
        || lower.starts_with("file:")
        || lower.starts_with("obsidian:")
}

fn unresolved_label(target: &str, heading: Option<&str>) -> String {
    match heading {
        Some(heading) if !heading.trim().is_empty() => format!("{}#{heading}", target.trim()),
        _ => target.trim().to_owned(),
    }
}
