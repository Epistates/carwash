//! The TUI's single source of truth and the rows derived from it.

use super::query::{Candidate, Fuzzy, Query};
use carwash_core::cache::SizeCache;
use carwash_core::select::{Hold, Policy};
use carwash_core::{Artifact, ArtifactId, EcoId, Project, ProjectId, ScanEvent, Size};
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EntryStatus {
    Present,
    Deleting,
    Deleted,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub artifact: Artifact,
    /// Last known size from the cache, shown until a fresh measurement arrives.
    pub cached: Option<Size>,
    pub status: EntryStatus,
}

impl Entry {
    pub fn size(&self) -> Option<Size> {
        self.artifact.size.or(self.cached)
    }

    pub fn is_estimate(&self) -> bool {
        self.artifact.size.is_none() && self.cached.is_some()
    }

    /// Newest modification time, including a cached measurement's.
    pub fn last_modified(&self) -> Option<SystemTime> {
        self.size()
            .and_then(|s| s.newest)
            .or(self.artifact.modified)
    }

    pub fn is_live(&self) -> bool {
        self.status != EntryStatus::Deleted
    }
}

#[derive(Debug, Default)]
pub struct Store {
    pub root: PathBuf,
    pub projects: Vec<Project>,
    project_index: HashMap<ProjectId, usize>,
    pub entries: Vec<Entry>,
    entry_index: HashMap<ArtifactId, usize>,
    pub warnings: usize,
    pub dirs: u64,
    /// Bumped on every change, so derived rows know when to rebuild.
    pub revision: u64,
}

impl Store {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            ..Self::default()
        }
    }

    pub fn apply(&mut self, event: ScanEvent, cache: &SizeCache) {
        match event {
            ScanEvent::Project(project) => {
                self.project_index.insert(project.id, self.projects.len());
                self.projects.push(project);
            }
            ScanEvent::Artifact(artifact) => {
                let cached = cache.get(&artifact.path).map(|c| c.size);
                self.entry_index.insert(artifact.id, self.entries.len());
                self.entries.push(Entry {
                    artifact,
                    cached,
                    status: EntryStatus::Present,
                });
            }
            ScanEvent::Warning(_) => self.warnings += 1,
            ScanEvent::DiscoveryFinished { dirs, .. } => self.dirs = dirs,
            ScanEvent::Git { id, state } => {
                if let Some(entry) = self.entry_mut(id) {
                    entry.artifact.git = state;
                }
            }
            ScanEvent::Measured { id, size } => {
                if let Some(entry) = self.entry_mut(id) {
                    entry.artifact.size = Some(size);
                }
            }
            ScanEvent::Finished { .. } => {}
        }
        self.revision += 1;
    }

    pub fn project(&self, id: ProjectId) -> Option<&Project> {
        self.project_index.get(&id).map(|&i| &self.projects[i])
    }

    pub fn entry(&self, id: ArtifactId) -> Option<&Entry> {
        self.entry_index.get(&id).map(|&i| &self.entries[i])
    }

    pub fn entry_mut(&mut self, id: ArtifactId) -> Option<&mut Entry> {
        self.revision += 1;
        self.entry_index
            .get(&id)
            .copied()
            .map(|i| &mut self.entries[i])
    }

    pub fn set_status(&mut self, id: ArtifactId, status: EntryStatus) {
        if let Some(entry) = self.entry_mut(id) {
            entry.status = status;
        }
    }

    pub fn relative<'a>(&self, path: &'a Path) -> std::borrow::Cow<'a, str> {
        match path.strip_prefix(&self.root) {
            Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy(),
            _ => path.to_string_lossy(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Grouping {
    Tree,
    Projects,
    Artifacts,
}

impl Grouping {
    pub fn next(self) -> Self {
        match self {
            Self::Tree => Self::Projects,
            Self::Projects => Self::Artifacts,
            Self::Artifacts => Self::Tree,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tree => "tree",
            Self::Projects => "projects",
            Self::Artifacts => "artifacts",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Size,
    Age,
    Name,
}

impl SortKey {
    pub fn next(self) -> Self {
        match self {
            Self::Size => Self::Age,
            Self::Age => Self::Name,
            Self::Name => Self::Size,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Size => "size",
            Self::Age => "age",
            Self::Name => "name",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum RowKey {
    Dir(PathBuf),
    Project(ProjectId),
    Artifact(ArtifactId),
    /// Artifacts not owned by any project (projects grouping).
    Orphans,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub reclaimable: u64,
    pub on_disk: u64,
    pub artifacts: u32,
    /// Artifacts that may be marked (live, not protected).
    pub markable: u32,
    /// Markable artifacts the default policy would select.
    pub ready: u32,
    pub marked: u32,
    pub marked_bytes: u64,
    pub unmeasured: u32,
    pub estimated: bool,
}

impl Totals {
    fn add(&mut self, other: &Totals) {
        self.reclaimable += other.reclaimable;
        self.on_disk += other.on_disk;
        self.artifacts += other.artifacts;
        self.markable += other.markable;
        self.ready += other.ready;
        self.marked += other.marked;
        self.marked_bytes += other.marked_bytes;
        self.unmeasured += other.unmeasured;
        self.estimated |= other.estimated;
    }
}

#[derive(Debug, Clone)]
pub struct Row {
    pub key: RowKey,
    pub depth: u16,
    pub label: String,
    pub expandable: bool,
    pub expanded: bool,
    pub totals: Totals,
    pub newest: Option<SystemTime>,
    pub ecosystems: Vec<EcoId>,
    /// This row's artifacts as a range of [`Rows::order`].
    pub span: Range<usize>,
}

/// Rows for display plus the artifact order they index into.
#[derive(Debug, Default)]
pub struct Rows {
    pub rows: Vec<Row>,
    pub order: Vec<ArtifactId>,
    pub totals: Totals,
    /// Largest reclaimable size among top-level rows, for proportional bars.
    pub max_top: u64,
}

impl Rows {
    pub fn artifacts(&self, row: &Row) -> &[ArtifactId] {
        &self.order[row.span.clone()]
    }

    pub fn position(&self, key: &RowKey) -> Option<usize> {
        self.rows.iter().position(|r| &r.key == key)
    }
}

/// Inputs to row derivation.
pub struct View<'a> {
    pub grouping: Grouping,
    pub sort: SortKey,
    pub query: &'a Query,
    pub fuzzy: &'a mut Fuzzy,
    pub marked: &'a HashSet<ArtifactId>,
    pub expanded: &'a HashSet<RowKey>,
    pub policy: &'a Policy,
    pub now: SystemTime,
}

#[derive(Debug)]
struct Node {
    key: RowKey,
    label: String,
    children: Vec<usize>,
    totals: Totals,
    newest: Option<SystemTime>,
    /// Reclaimable bytes per ecosystem in this subtree; badges are ordered by it.
    eco_bytes: Vec<(EcoId, u64)>,
}

impl Node {
    fn new(key: RowKey, label: String) -> Self {
        Self {
            key,
            label,
            children: Vec::new(),
            totals: Totals::default(),
            newest: None,
            eco_bytes: Vec::new(),
        }
    }

    fn add_eco(&mut self, eco: EcoId, bytes: u64) {
        match self.eco_bytes.iter_mut().find(|(e, _)| *e == eco) {
            Some((_, total)) => *total += bytes,
            None => self.eco_bytes.push((eco, bytes)),
        }
    }

    fn ecosystems(&self) -> Vec<EcoId> {
        let mut ranked = self.eco_bytes.clone();
        ranked.sort_by_key(|&(_, bytes)| std::cmp::Reverse(bytes));
        ranked.into_iter().map(|(eco, _)| eco).collect()
    }
}

pub fn hold_for(entry: &Entry, policy: &Policy, now: SystemTime) -> Option<Hold> {
    let mut artifact = entry.artifact.clone();
    if artifact.size.is_none() {
        artifact.size = entry.cached;
    }
    policy.hold(&artifact, now)
}

fn artifact_totals(entry: &Entry, hold: Option<Hold>, marked: bool) -> Totals {
    let size = entry.size();
    let reclaimable = size.map_or(0, |s| s.reclaimable);
    let markable = entry.status == EntryStatus::Present && hold != Some(Hold::Protected);
    Totals {
        reclaimable,
        on_disk: size.map_or(0, |s| s.on_disk),
        artifacts: 1,
        markable: u32::from(markable),
        ready: u32::from(markable && hold.is_none()),
        marked: u32::from(marked),
        marked_bytes: if marked { reclaimable } else { 0 },
        unmeasured: u32::from(size.is_none()),
        estimated: entry.is_estimate(),
    }
}

/// Derives display rows from the store.
pub fn build(store: &Store, view: &mut View<'_>) -> Rows {
    let mut nodes: Vec<Node> = vec![Node::new(RowKey::Dir(store.root.clone()), String::new())];
    let mut by_path: HashMap<PathBuf, usize> = HashMap::new();
    by_path.insert(store.root.clone(), 0);
    let mut orphans: Option<usize> = None;

    for entry in store.entries.iter().filter(|e| e.is_live()) {
        let artifact = &entry.artifact;
        let hold = hold_for(entry, view.policy, view.now);
        let marked = view.marked.contains(&artifact.id);
        let project = artifact.project.and_then(|id| store.project(id));
        let rel = store.relative(&artifact.path);
        let haystack = match project {
            Some(p) => format!("{rel} {}", p.name),
            None => rel.to_string(),
        };
        let candidate = Candidate {
            haystack: &haystack,
            ecosystem: artifact.ecosystem,
            kind: artifact.kind,
            reclaimable: entry.size().map(|s| s.reclaimable),
            age: entry
                .last_modified()
                .map(|t| view.now.duration_since(t).unwrap_or_default()),
            hold,
            marked,
        };
        if !view.query.matches(&candidate, view.fuzzy) {
            continue;
        }

        let parent = match view.grouping {
            Grouping::Artifacts => 0,
            Grouping::Projects => match project {
                Some(p) => *by_path.entry(p.path.clone()).or_insert_with(|| {
                    let index = nodes.len();
                    nodes.push(Node::new(
                        RowKey::Project(p.id),
                        store.relative(&p.path).into_owned(),
                    ));
                    nodes[0].children.push(index);
                    index
                }),
                None => *orphans.get_or_insert_with(|| {
                    let index = nodes.len();
                    nodes.push(Node::new(RowKey::Orphans, "(outside projects)".into()));
                    nodes[0].children.push(index);
                    index
                }),
            },
            Grouping::Tree => {
                let (container, key) = match project {
                    Some(p) => (p.path.clone(), RowKey::Project(p.id)),
                    None => {
                        let dir = artifact.path.parent().unwrap_or(&store.root).to_path_buf();
                        (dir.clone(), RowKey::Dir(dir))
                    }
                };
                tree_container(&mut nodes, &mut by_path, &store.root, &container, key)
            }
        };

        let label = match (view.grouping, project) {
            (Grouping::Artifacts, _) => rel.into_owned(),
            (_, Some(p)) if artifact.path.starts_with(&p.path) && artifact.path != p.path => {
                artifact
                    .path
                    .strip_prefix(&p.path)
                    .map(|r| r.to_string_lossy().into_owned())
                    .unwrap_or_else(|_| rel.into_owned())
            }
            _ => artifact
                .path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| rel.into_owned()),
        };
        let mut node = Node::new(RowKey::Artifact(artifact.id), label);
        node.totals = artifact_totals(entry, hold, marked);
        node.newest = entry.last_modified();
        if let Some(eco) = artifact.ecosystem {
            node.add_eco(eco, node.totals.reclaimable);
        }
        nodes.push(node);
        let index = nodes.len() - 1;
        nodes[parent].children.push(index);
    }

    aggregate(&mut nodes, 0);
    for node in &mut nodes {
        if let RowKey::Project(id) = node.key
            && let Some(project) = store.project(id)
        {
            // Ecosystems without artifacts still identify the project, after the others.
            for &eco in &project.ecosystems {
                node.add_eco(eco, 0);
            }
        }
    }
    if view.grouping == Grouping::Tree {
        compress(&mut nodes, 0);
    }
    sort(&mut nodes, 0, view.sort);

    let mut rows = Rows {
        totals: nodes[0].totals,
        max_top: nodes[0]
            .children
            .iter()
            .map(|&c| nodes[c].totals.reclaimable)
            .max()
            .unwrap_or(0),
        ..Rows::default()
    };
    let expand_all = !view.query.is_empty();
    let children = nodes[0].children.clone();
    for child in children {
        flatten(&nodes, child, 0, true, expand_all, view.expanded, &mut rows);
    }
    rows
}

/// Finds or creates the tree node for `container`, creating directory nodes along the way.
fn tree_container(
    nodes: &mut Vec<Node>,
    by_path: &mut HashMap<PathBuf, usize>,
    root: &Path,
    container: &Path,
    key: RowKey,
) -> usize {
    if let Some(&index) = by_path.get(container) {
        if matches!(key, RowKey::Project(_)) {
            nodes[index].key = key;
        }
        return index;
    }
    let Ok(relative) = container.strip_prefix(root) else {
        // Above the scan root (an enclosing workspace): its own top-level node.
        nodes.push(Node::new(key, format!("↑ {}", container.display())));
        let index = nodes.len() - 1;
        nodes[0].children.push(index);
        by_path.insert(container.to_path_buf(), index);
        return index;
    };
    let mut parent = 0;
    let mut path = root.to_path_buf();
    let components: Vec<_> = relative.components().collect();
    for (i, component) in components.iter().enumerate() {
        path.push(component);
        let last = i + 1 == components.len();
        parent = match by_path.get(&path) {
            Some(&index) => {
                if last && matches!(key, RowKey::Project(_)) {
                    nodes[index].key = key.clone();
                }
                index
            }
            None => {
                let node_key = if last {
                    key.clone()
                } else {
                    RowKey::Dir(path.clone())
                };
                nodes.push(Node::new(
                    node_key,
                    component.as_os_str().to_string_lossy().into_owned(),
                ));
                let index = nodes.len() - 1;
                nodes[parent].children.push(index);
                by_path.insert(path.clone(), index);
                index
            }
        };
    }
    parent
}

fn aggregate(nodes: &mut [Node], index: usize) {
    let children = nodes[index].children.clone();
    for &child in &children {
        aggregate(nodes, child);
    }
    if children.is_empty() {
        return;
    }
    let mut totals = Totals::default();
    let mut newest: Option<SystemTime> = None;
    let mut eco_bytes: Vec<(EcoId, u64)> = Vec::new();
    for &child in &children {
        totals.add(&nodes[child].totals);
        newest = carwash_core::measure::newest(newest, nodes[child].newest);
        eco_bytes.extend(nodes[child].eco_bytes.iter().copied());
    }
    let node = &mut nodes[index];
    node.totals.add(&totals);
    node.newest = carwash_core::measure::newest(node.newest, newest);
    for (eco, bytes) in eco_bytes {
        node.add_eco(eco, bytes);
    }
}

/// Merges directory chains with a single child (`a` → `b` → `c` becomes `a/b/c`).
fn compress(nodes: &mut Vec<Node>, index: usize) {
    let children = nodes[index].children.clone();
    for child in children {
        loop {
            let node = &nodes[child];
            let mergeable = matches!(node.key, RowKey::Dir(_))
                && node.children.len() == 1
                && !matches!(nodes[node.children[0]].key, RowKey::Artifact(_));
            if !mergeable {
                break;
            }
            let only = nodes[child].children[0];
            let label = format!("{}/{}", nodes[child].label, nodes[only].label);
            let key = nodes[only].key.clone();
            let grandchildren = std::mem::take(&mut nodes[only].children);
            let node = &mut nodes[child];
            node.label = label;
            node.key = key;
            node.children = grandchildren;
        }
        compress(nodes, child);
    }
}

fn sort(nodes: &mut [Node], index: usize, key: SortKey) {
    let mut children = std::mem::take(&mut nodes[index].children);
    match key {
        SortKey::Size => children.sort_by(|&a, &b| {
            nodes[b]
                .totals
                .reclaimable
                .cmp(&nodes[a].totals.reclaimable)
                .then_with(|| nodes[a].label.cmp(&nodes[b].label))
        }),
        SortKey::Age => children.sort_by(|&a, &b| {
            let age = |n: &Node| n.newest.unwrap_or(SystemTime::UNIX_EPOCH);
            age(&nodes[a])
                .cmp(&age(&nodes[b]))
                .then_with(|| nodes[a].label.cmp(&nodes[b].label))
        }),
        SortKey::Name => children.sort_by(|&a, &b| nodes[a].label.cmp(&nodes[b].label)),
    }
    for &child in &children {
        sort(nodes, child, key);
    }
    nodes[index].children = children;
}

fn flatten(
    nodes: &[Node],
    index: usize,
    depth: u16,
    visible: bool,
    expand_all: bool,
    expanded: &HashSet<RowKey>,
    rows: &mut Rows,
) {
    let node = &nodes[index];
    let start = rows.order.len();
    let is_expanded = expand_all || expanded.contains(&node.key);
    let row_index = if visible {
        rows.rows.push(Row {
            key: node.key.clone(),
            depth,
            label: node.label.clone(),
            expandable: !node.children.is_empty(),
            expanded: is_expanded && !node.children.is_empty(),
            totals: node.totals,
            newest: node.newest,
            ecosystems: node.ecosystems(),
            span: 0..0,
        });
        Some(rows.rows.len() - 1)
    } else {
        None
    };
    if let RowKey::Artifact(id) = node.key {
        rows.order.push(id);
    }
    for &child in &node.children {
        flatten(
            nodes,
            child,
            depth + 1,
            visible && is_expanded,
            expand_all,
            expanded,
            rows,
        );
    }
    if let Some(row) = row_index {
        rows.rows[row].span = start..rows.order.len();
    }
}

/// Relative age helper for display.
pub fn age(newest: Option<SystemTime>, now: SystemTime) -> Option<Duration> {
    newest.map(|t| now.duration_since(t).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use carwash_core::{ArtifactKind, Detection, GitState, Registry, RuleId};

    fn project(id: u32, path: &str) -> Project {
        Project {
            id: ProjectId(id),
            path: PathBuf::from(path),
            name: Path::new(path)
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            ecosystems: vec![EcoId(0)],
            parent: None,
            member_of: None,
            is_workspace: false,
            repo: None,
            last_activity: None,
            outside_root: false,
        }
    }

    fn artifact(id: u32, project: Option<u32>, path: &str, bytes: u64) -> Artifact {
        Artifact {
            id: ArtifactId(id),
            path: PathBuf::from(path),
            project: project.map(ProjectId),
            ecosystem: Some(EcoId(0)),
            kind: ArtifactKind::Build,
            detection: Detection::Rule {
                rule: RuleId(0),
                confirmed: true,
            },
            ambiguous: false,
            modified: Some(SystemTime::UNIX_EPOCH),
            git: GitState::NotInRepo,
            repo: None,
            size: Some(Size {
                reclaimable: bytes,
                on_disk: bytes,
                ..Size::default()
            }),
            outside_root: false,
        }
    }

    fn store() -> Store {
        let mut store = Store::new(PathBuf::from("/w"));
        let cache = SizeCache::default();
        for event in [
            ScanEvent::Project(project(0, "/w/big")),
            ScanEvent::Project(project(1, "/w/group/deep/small")),
            ScanEvent::Project(project(2, "/w/group/other")),
            ScanEvent::Artifact(artifact(0, Some(0), "/w/big/target", 1000)),
            ScanEvent::Artifact(artifact(1, Some(1), "/w/group/deep/small/node_modules", 10)),
            ScanEvent::Artifact(artifact(2, Some(1), "/w/group/deep/small/.next", 5)),
            ScanEvent::Artifact(artifact(3, Some(2), "/w/group/other/dist", 50)),
            ScanEvent::Artifact(artifact(4, None, "/w/loose/__pycache__", 1)),
        ] {
            store.apply(event, &cache);
        }
        store
    }

    fn rows(store: &Store, grouping: Grouping, expanded: &HashSet<RowKey>, query: &str) -> Rows {
        let registry = Registry::builtin();
        let query = Query::parse(query, &registry);
        let mut fuzzy = Fuzzy::default();
        let marked = HashSet::new();
        let policy = Policy {
            include_review: false,
            recent: None,
        };
        build(
            store,
            &mut View {
                grouping,
                sort: SortKey::Size,
                query: &query,
                fuzzy: &mut fuzzy,
                marked: &marked,
                expanded,
                policy: &policy,
                now: SystemTime::now(),
            },
        )
    }

    fn labels(rows: &Rows) -> Vec<(u16, String)> {
        rows.rows
            .iter()
            .map(|r| (r.depth, r.label.clone()))
            .collect()
    }

    #[test]
    fn tree_groups_sorts_and_aggregates() {
        let store = store();
        let collapsed = rows(&store, Grouping::Tree, &HashSet::new(), "");
        assert_eq!(
            labels(&collapsed),
            vec![(0, "big".into()), (0, "group".into()), (0, "loose".into())]
        );
        assert_eq!(collapsed.totals.reclaimable, 1066);
        assert_eq!(collapsed.max_top, 1000);
        let group = &collapsed.rows[1];
        assert_eq!(group.totals.reclaimable, 65);
        assert_eq!(collapsed.artifacts(group).len(), 3);
    }

    #[test]
    fn single_child_directories_are_compressed() {
        let store = store();
        let mut expanded = HashSet::new();
        expanded.insert(RowKey::Dir(PathBuf::from("/w/group")));
        let rows = rows(&store, Grouping::Tree, &expanded, "");
        let labels = labels(&rows);
        assert!(labels.contains(&(1, "other".into())));
        assert!(labels.contains(&(1, "deep/small".into())), "{labels:?}");
    }

    #[test]
    fn search_expands_matches_and_filters() {
        let store = store();
        let rows = rows(&store, Grouping::Tree, &HashSet::new(), "next");
        let labels = labels(&rows);
        assert_eq!(labels.last().unwrap().1, ".next");
        assert_eq!(rows.totals.artifacts, 1);
    }

    #[test]
    fn projects_and_artifacts_groupings() {
        let store = store();
        let projects = rows(&store, Grouping::Projects, &HashSet::new(), "");
        assert_eq!(projects.rows[0].label, "big");
        assert!(projects.rows.iter().any(|r| r.key == RowKey::Orphans));
        let flat = rows(&store, Grouping::Artifacts, &HashSet::new(), "");
        assert_eq!(flat.rows.len(), 5);
        assert_eq!(flat.rows[0].label, "big/target");
    }

    #[test]
    fn deleted_entries_disappear() {
        let mut store = store();
        store.set_status(ArtifactId(0), EntryStatus::Deleted);
        let rows = rows(&store, Grouping::Artifacts, &HashSet::new(), "");
        assert_eq!(rows.rows.len(), 4);
        assert_eq!(rows.totals.reclaimable, 66);
    }
}
