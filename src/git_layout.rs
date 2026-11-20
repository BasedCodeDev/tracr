//! Deterministic, prefix-stable logical lanes. No glyphs, I/O or UI state.
//!
//! Input is child-before-parent. Vacant columns are reused, never compacted.
//! A lane targets an unresolved commit; multiple child connections can converge
//! on that target without losing their separate (child OID, parent index) identity.
//! With a mainline, a side lane may temporarily share its target with column zero;
//! they converge immediately above that target, never at an intervening commit.
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Commit {
    pub oid: String,
    pub parents: Vec<String>,
}

/// Slots in the existing cyan/magenta/yellow/green/blue/red palette.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LaneColor {
    Cyan,
    Magenta,
    Yellow,
    Green,
    Blue,
    Red,
}
impl LaneColor {
    fn for_id(id: usize) -> Self {
        [
            Self::Cyan,
            Self::Magenta,
            Self::Yellow,
            Self::Green,
            Self::Blue,
            Self::Red,
        ][id % 6]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Lane {
    /// Stable identity, independent of the reusable column and repeating colour.
    pub id: usize,
    pub column: usize,
    pub color: LaneColor,
    pub target: Arc<str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub oid: Arc<str>,
    pub column: usize,
    pub lane_id: usize,
    pub color: LaneColor,
}

/// Origin is the containing row's node; parent_index identifies this edge.
/// Crossing geometry must never be used to merge these logical connections.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParentConnection {
    pub parent: Arc<str>,
    pub parent_index: usize,
    pub target_column: usize,
    pub lane_id: usize,
    pub color: LaneColor,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayoutRow {
    /// Incoming side lanes that converge immediately above this commit.
    pub joins: Vec<Lane>,
    pub node: Node,
    /// Column-sorted active lanes before and after this commit's transition.
    pub incoming: Vec<Lane>,
    pub outgoing: Vec<Lane>,
    pub parents: Vec<ParentConnection>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub rows: Vec<LayoutRow>,
    pub positions: BTreeMap<Arc<str>, usize>,
    /// Maximum column index used anywhere in this layout, plus one.
    pub width: usize,
    /// Open targets at the current prefix boundary, ordered by column.
    pub unresolved: Vec<Lane>,
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum LayoutError {
    #[error("Layout cancelled")]
    Cancelled,
    #[error("Duplicate commit {0}")]
    DuplicateCommit(String),
    #[error("Commit {0} is its own parent")]
    SelfParent(String),
    #[error("Commit {oid} repeats parent {parent}")]
    RepeatedParent { oid: String, parent: String },
    #[error("Parent {parent} was processed before child {oid}")]
    ParentAlreadyProcessed { oid: String, parent: String },
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Builder {
    layout: Layout,
    slots: Vec<Option<Lane>>,
    targets: HashMap<Arc<str>, usize>,
    next_lane: usize,
    mainline: HashSet<String>,
}
impl Builder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_mainline(mainline: HashSet<String>) -> Self {
        let mut builder = Self {
            mainline,
            ..Self::default()
        };
        if !builder.mainline.is_empty() {
            builder.slots.push(None);
        }
        builder
    }
    fn main_lane(&mut self, target: Arc<str>) -> Lane {
        self.layout.width = self.layout.width.max(1);
        Lane {
            id: usize::MAX,
            column: 0,
            color: LaneColor::Cyan,
            target,
        }
    }
    pub fn snapshot(&self) -> &Layout {
        &self.layout
    }
    pub fn into_layout(self) -> Layout {
        self.layout
    }

    fn active(&self) -> Vec<Lane> {
        self.slots.iter().flatten().cloned().collect()
    }
    fn allocate(&mut self, target: Arc<str>, reserved: Option<usize>) -> Lane {
        let column = self
            .slots
            .iter()
            .enumerate()
            .find(|(i, lane)| {
                lane.is_none() && Some(*i) != reserved && (self.mainline.is_empty() || *i != 0)
            })
            .map(|(i, _)| i)
            .unwrap_or(self.slots.len());
        if column == self.slots.len() {
            self.slots.push(None);
        }
        let id = self.next_lane;
        self.next_lane += 1;
        self.layout.width = self.layout.width.max(column + 1);
        Lane {
            id,
            column,
            color: if self.mainline.is_empty() {
                LaneColor::for_id(id)
            } else {
                [
                    LaneColor::Magenta,
                    LaneColor::Yellow,
                    LaneColor::Green,
                    LaneColor::Blue,
                    LaneColor::Red,
                ][id % 5]
            },
            target,
        }
    }
    fn place(&mut self, lane: Lane) {
        self.targets.insert(lane.target.clone(), lane.column);
        let column = lane.column;
        self.slots[column] = Some(lane);
    }

    /// Append one commit. Invalid input leaves every part of the builder intact.
    pub fn append(&mut self, commit: &Commit) -> Result<&LayoutRow, LayoutError> {
        self.append_cancellable(commit, &|| false)
    }

    /// Cancellation invalidates this builder; callers must discard it.
    pub fn append_cancellable(
        &mut self,
        commit: &Commit,
        cancelled: &impl Fn() -> bool,
    ) -> Result<&LayoutRow, LayoutError> {
        if cancelled() {
            return Err(LayoutError::Cancelled);
        }
        if self.layout.positions.contains_key(commit.oid.as_str()) {
            return Err(LayoutError::DuplicateCommit(commit.oid.clone()));
        }
        let mut unique = HashSet::new();
        for parent in &commit.parents {
            if cancelled() {
                return Err(LayoutError::Cancelled);
            }
            if parent == &commit.oid {
                return Err(LayoutError::SelfParent(commit.oid.clone()));
            }
            if !unique.insert(parent) {
                return Err(LayoutError::RepeatedParent {
                    oid: commit.oid.clone(),
                    parent: parent.clone(),
                });
            }
            if self.layout.positions.contains_key(parent.as_str()) {
                return Err(LayoutError::ParentAlreadyProcessed {
                    oid: commit.oid.clone(),
                    parent: parent.clone(),
                });
            }
        }
        // All fallible input validation is complete before any mutation.
        let incoming = self.active();
        let oid: Arc<str> = Arc::from(commit.oid.as_str());
        let is_main = self.mainline.contains(&commit.oid);
        let mut joins = Vec::new();
        let node_lane = if is_main {
            self.targets.remove(commit.oid.as_str());
            for slot in &mut self.slots {
                if slot.as_ref().is_some_and(|lane| lane.target == oid) {
                    let lane = slot.take().unwrap();
                    if lane.column != 0 {
                        joins.push(lane);
                    }
                }
            }
            self.main_lane(oid.clone())
        } else if let Some(column) = self.targets.remove(commit.oid.as_str()) {
            self.slots[column]
                .take()
                .expect("target map and slots agree")
        } else {
            self.allocate(oid.clone(), None)
        };
        let mut parents = Vec::with_capacity(commit.parents.len());
        for (parent_index, parent) in commit.parents.iter().enumerate() {
            if cancelled() {
                return Err(LayoutError::Cancelled);
            }
            let lane = if is_main && parent_index == 0 {
                let lane = self.main_lane(Arc::from(parent.as_str()));
                self.place(lane.clone());
                lane
            } else if let Some(&column) = self.targets.get(parent.as_str()) {
                self.slots[column]
                    .as_ref()
                    .expect("active target has a lane")
                    .clone()
            } else {
                let target = Arc::from(parent.as_str());
                let lane = if parent_index == 0 {
                    Lane {
                        target,
                        ..node_lane.clone()
                    }
                } else {
                    self.allocate(target, Some(node_lane.column))
                };
                self.place(lane.clone());
                lane
            };
            parents.push(ParentConnection {
                parent: lane.target.clone(),
                parent_index,
                target_column: lane.column,
                lane_id: lane.id,
                color: lane.color,
            });
        }
        let outgoing = self.active();
        self.layout
            .positions
            .insert(oid.clone(), self.layout.rows.len());
        self.layout.unresolved = outgoing.clone();
        self.layout.rows.push(LayoutRow {
            joins,
            node: Node {
                oid,
                column: node_lane.column,
                lane_id: node_lane.id,
                color: node_lane.color,
            },
            incoming,
            outgoing,
            parents,
        });
        Ok(self.layout.rows.last().expect("row just inserted"))
    }
}

pub fn layout(commits: &[Commit]) -> Result<Layout, LayoutError> {
    let mut builder = Builder::new();
    for commit in commits {
        builder.append(commit)?;
    }
    Ok(builder.into_layout())
}
