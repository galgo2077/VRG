//! Ephemeral, bounded semantic calculations for the read-only viewer.
//!
//! The caller supplies only vectors already authorized for one project and one
//! privacy domain. No inference, persistence, model loading, or network occurs.
use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

pub const MAX_VECTORS: usize = 2_000;
pub const MAX_DIMENSIONS: usize = 4_096;
const MAX_ITERATIONS: usize = 80;
const EPS: f64 = 1e-12;

#[derive(Clone, Debug)]
pub struct InputVector {
    pub id: String,
    /// Complete model identity including the native pipeline fingerprint.
    pub model_identity: String,
    pub values: Vec<f32>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Position {
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Similarity {
    pub source: String,
    pub target: String,
    pub score: f64,
    pub model_identity: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectionGroup {
    pub model_identity: String,
    pub dimensions: usize,
    pub count: usize,
    pub explained_variance: [f64; 2],
    pub degenerate: bool,
    pub converged: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct SemanticAnalysis {
    pub edges: Vec<Similarity>,
    pub positions: BTreeMap<String, Position>,
    pub components: BTreeMap<String, usize>,
    pub groups: Vec<ProjectionGroup>,
    pub excluded: usize,
    pub truncated: bool,
    pub k: usize,
    pub threshold: f64,
    pub note: String,
}

struct Normalized {
    id: String,
    values: Vec<f64>,
}

/// Exact top-k cosine within each compatible group of the selected sample.
/// Directed top-k neighborhoods are combined by union. Memory is O(N D + N k),
/// not an NxN matrix. PCA uses bounded deterministic covariance power iteration.
/// Groups are kept separate; coordinates from different models are not comparable.
pub fn analyze(vectors: &[InputVector], k: usize, threshold: f64) -> SemanticAnalysis {
    // Non-cancellable convenience entry point for bounded mathematical fixtures.
    analyze_cancellable(vectors, k, threshold, || false).expect("non-cancellable analysis")
}

/// Same calculation with cooperative cancellation. The caller owns the deadline
/// or cancellation flag; cancelled results are discarded atomically, never
/// exposed as complete PCA/top-k. Checks include covariance blocks and neighbors.
pub fn analyze_cancellable(
    vectors: &[InputVector],
    k: usize,
    threshold: f64,
    cancelled: impl Fn() -> bool,
) -> Option<SemanticAnalysis> {
    if cancelled() {
        return None;
    }
    let k = k.min(50);
    let threshold = if threshold.is_finite() {
        threshold.clamp(-1.0, 1.0)
    } else {
        0.75
    };
    let mut result = SemanticAnalysis {
        edges: Vec::new(),
        positions: BTreeMap::new(),
        components: BTreeMap::new(),
        groups: Vec::new(),
        excluded: 0,
        truncated: vectors.len() > MAX_VECTORS,
        k,
        threshold,
        note: "Coseno exacto en la muestra autorizada; unión de top-k dirigido. PCA 2D de vectores normalizados y centrados, iteración determinista aproximada (máximo 80 pasos por componente); distancias visuales no son scores. Modelos distintos no se comparan; coordenadas entre grupos no son comparables.".into(),
    };
    // Select by ID deterministically and disclose truncation, never claim a
    // representative random sample or exact neighbors over the entire store.
    let mut ordered: Vec<_> = vectors.iter().collect();
    ordered.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then(a.model_identity.cmp(&b.model_identity))
    });
    ordered.truncate(MAX_VECTORS);
    let mut duplicate_ids = BTreeSet::new();
    for pair in ordered.windows(2) {
        if pair[0].id == pair[1].id {
            duplicate_ids.insert(pair[0].id.as_str());
        }
    }
    let mut groups: BTreeMap<(String, usize), Vec<Normalized>> = BTreeMap::new();
    for vector in &ordered {
        if cancelled() {
            return None;
        }
        let identity_valid = vector
            .model_identity
            .rsplit_once('#')
            .is_some_and(|(name, hash)| {
                !name.is_empty() && hash.len() == 64 && hash.bytes().all(|x| x.is_ascii_hexdigit())
            });
        let norm = vector
            .values
            .iter()
            .map(|x| f64::from(*x).powi(2))
            .sum::<f64>()
            .sqrt();
        if vector.id.is_empty()
            || duplicate_ids.contains(vector.id.as_str())
            || !identity_valid
            || vector.values.is_empty()
            || vector.values.len() > MAX_DIMENSIONS
            || !norm.is_finite()
            || norm <= EPS
            || vector.values.iter().any(|x| !x.is_finite())
        {
            result.excluded += 1;
            continue;
        }
        groups
            .entry((vector.model_identity.clone(), vector.values.len()))
            .or_default()
            .push(Normalized {
                id: vector.id.clone(),
                values: vector.values.iter().map(|x| f64::from(*x) / norm).collect(),
            });
    }
    for ((model_identity, dimensions), group) in groups {
        let mut pairs: BTreeMap<(String, String), f64> = BTreeMap::new();
        for (index, vector) in group.iter().enumerate() {
            if cancelled() {
                return None;
            }
            let mut best: Vec<(usize, f64)> = Vec::with_capacity(k + 1);
            if k > 0 {
                for (other_index, other) in group.iter().enumerate() {
                    if other_index % 32 == 0 && cancelled() {
                        return None;
                    }
                    if index == other_index {
                        continue;
                    }
                    let score = dot(&vector.values, &other.values).clamp(-1.0, 1.0);
                    if score < threshold {
                        continue;
                    }
                    best.push((other_index, score));
                    best.sort_by(|a, b| {
                        b.1.total_cmp(&a.1).then(group[a.0].id.cmp(&group[b.0].id))
                    });
                    best.truncate(k);
                }
            }
            for (other_index, score) in best {
                let other = &group[other_index];
                let pair = if vector.id < other.id {
                    (vector.id.clone(), other.id.clone())
                } else {
                    (other.id.clone(), vector.id.clone())
                };
                pairs.insert(pair, score);
            }
        }
        for ((source, target), score) in pairs {
            result.edges.push(Similarity {
                source,
                target,
                score,
                model_identity: model_identity.clone(),
            });
        }
        let (positions, variance, converged) = pca(&group, &cancelled)?;
        let degenerate = variance[0] <= EPS || variance[1] <= EPS;
        result.positions.extend(positions);
        result.groups.push(ProjectionGroup {
            model_identity,
            dimensions,
            count: group.len(),
            explained_variance: variance,
            degenerate,
            converged,
        });
    }
    // Components depend only on the computed cosine graph, never PCA distance.
    let mut adjacent: BTreeMap<&str, Vec<&str>> = result
        .positions
        .keys()
        .map(|id| (id.as_str(), Vec::new()))
        .collect();
    for edge in &result.edges {
        adjacent.entry(&edge.source).or_default().push(&edge.target);
        adjacent.entry(&edge.target).or_default().push(&edge.source);
    }
    let mut component = 0;
    for id in result.positions.keys() {
        if result.components.contains_key(id) {
            continue;
        }
        component += 1;
        let mut pending = vec![id.as_str()];
        while let Some(node) = pending.pop() {
            if result.components.insert(node.into(), component).is_some() {
                continue;
            }
            pending.extend(
                adjacent
                    .get(node)
                    .into_iter()
                    .flatten()
                    .copied()
                    .filter(|n| !result.components.contains_key(*n)),
            );
        }
    }
    Some(result)
}

/// Full-index ANN over already-authorized embeddings. Unlike the legacy sample
/// analysis this never truncates vectors and never compares every vector pair.
/// Eight deterministic balanced random-projection trees give at most 120
/// candidate neighbors per row (16 rows per leaf, excluding self). Returned
/// scores are exact cosine on those candidates, not scores inferred from layout.
/// The caller must keep projects/privacy domains separate and may cache the
/// complete result by its generation, policy, vector/model and layout version.
pub fn analyze_full_cancellable(
    vectors: &[InputVector],
    k: usize,
    threshold: f64,
    cancelled: impl Fn() -> bool,
) -> Option<SemanticAnalysis> {
    if cancelled() {
        return None;
    }
    let k = k.min(6);
    let threshold = if threshold.is_finite() {
        threshold.clamp(-1.0, 1.0)
    } else {
        0.75
    };
    let mut result = SemanticAnalysis {
        edges: Vec::new(),
        positions: BTreeMap::new(),
        components: BTreeMap::new(),
        groups: Vec::new(),
        excluded: 0,
        truncated: false,
        k,
        threshold,
        note: "ANN aproximado mediante 8 árboles deterministas de proyección aleatoria, hojas de 16 y hasta 120 candidatos por chunk; unión de top-k dirigido (k ≤ 6), con coseno exacto verificado y umbral sobre los embeddings existentes. Se incluyen todos los vectores válidos, sin muestreo ni comparaciones cuadráticas. Modelos/dimensiones distintos no se comparan. Coordenadas de proyección aleatoria centrada; no son PCA, scores, ni una garantía de recuperar todos los vecinos exactos. explained_variance no se calcula para esta proyección.".into(),
    };
    let mut ordered: Vec<_> = vectors.iter().collect();
    ordered.sort_by(|a, b| {
        a.id.cmp(&b.id)
            .then(a.model_identity.cmp(&b.model_identity))
    });
    let duplicate_ids: BTreeSet<_> = ordered
        .windows(2)
        .filter(|pair| pair[0].id == pair[1].id)
        .map(|pair| pair[0].id.as_str())
        .collect();
    let mut groups: BTreeMap<(&str, usize), Vec<AnnVector<'_>>> = BTreeMap::new();
    for vector in ordered {
        if cancelled() {
            return None;
        }
        let identity_valid = vector
            .model_identity
            .rsplit_once('#')
            .is_some_and(|(name, hash)| {
                !name.is_empty() && hash.len() == 64 && hash.bytes().all(|x| x.is_ascii_hexdigit())
            });
        let norm = vector
            .values
            .iter()
            .map(|x| f64::from(*x).powi(2))
            .sum::<f64>()
            .sqrt();
        if vector.id.is_empty()
            || duplicate_ids.contains(vector.id.as_str())
            || !identity_valid
            || vector.values.is_empty()
            || vector.values.len() > MAX_DIMENSIONS
            || !norm.is_finite()
            || norm <= EPS
            || vector.values.iter().any(|x| !x.is_finite())
        {
            result.excluded += 1;
            continue;
        }
        groups
            .entry((&vector.model_identity, vector.values.len()))
            .or_default()
            .push(AnnVector {
                id: &vector.id,
                values: &vector.values,
                inv_norm: 1.0 / norm,
            });
    }
    let mut component_offset = 0;
    for ((model_identity, dimensions), group) in groups {
        let count = group.len();
        let seed = stable_seed(model_identity);
        let mut forest = Vec::with_capacity(8);
        if k > 0 {
            for tree in 0..8 {
                let mut partition = ProjectionPartition {
                    leaves: Vec::new(),
                    assignment: vec![0; count],
                };
                let mut indices: Vec<_> = (0..count).collect();
                partition_ann(
                    &group,
                    &mut indices,
                    seed ^ (tree + 1) * 0x100000001b3,
                    &mut partition,
                    &cancelled,
                )?;
                forest.push(partition);
            }
        }
        let mut pairs: BTreeMap<(usize, usize), f64> = BTreeMap::new();
        let mut components: Vec<_> = (0..count).collect();
        for (index, vector) in group.iter().enumerate() {
            if cancelled() {
                return None;
            }
            let mut candidates = Vec::with_capacity(128);
            for tree in &forest {
                candidates.extend_from_slice(&tree.leaves[tree.assignment[index]]);
            }
            candidates.sort_unstable();
            candidates.dedup();
            let mut best = Vec::with_capacity(k + 1);
            for (candidate_offset, other_index) in candidates.into_iter().enumerate() {
                if candidate_offset % 32 == 0 && cancelled() {
                    return None;
                }
                if index == other_index {
                    continue;
                }
                let other = &group[other_index];
                let score = ann_cosine(vector, other);
                if score < threshold {
                    continue;
                }
                best.push((other_index, score));
                best.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
                best.truncate(k);
            }
            for (other, score) in best {
                pairs.insert((index.min(other), index.max(other)), score);
            }
        }
        for ((source, target), score) in pairs {
            let a = ann_component(&mut components, source);
            let b = ann_component(&mut components, target);
            components[b] = a;
            result.edges.push(Similarity {
                source: group[source].id.into(),
                target: group[target].id.into(),
                score,
                model_identity: model_identity.into(),
            });
        }
        let axes = [
            sparse_axis(dimensions, seed ^ 0x51ed270b),
            sparse_axis(dimensions, seed ^ 0xa24baed4963ee407),
        ];
        let mut coordinates = Vec::with_capacity(count);
        let mut mean = [0.0; 2];
        for vector in &group {
            let xy = [
                ann_projection(vector, &axes[0]),
                ann_projection(vector, &axes[1]),
            ];
            mean[0] += xy[0] / count as f64;
            mean[1] += xy[1] / count as f64;
            coordinates.push(xy);
        }
        let mut variance = [0.0; 2];
        for xy in &coordinates {
            for d in 0..2 {
                variance[d] += (xy[d] - mean[d]).powi(2) / count as f64;
            }
        }
        let mut component_ids = BTreeMap::new();
        for (index, (vector, xy)) in group.iter().zip(coordinates).enumerate() {
            if index % 64 == 0 && cancelled() {
                return None;
            }
            result.positions.insert(
                vector.id.into(),
                Position {
                    x: (xy[0] - mean[0]) / variance[0].sqrt().max(EPS),
                    y: (xy[1] - mean[1]) / variance[1].sqrt().max(EPS),
                },
            );
            let root = ann_component(&mut components, index);
            let next = component_offset + component_ids.len() + 1;
            let component = *component_ids.entry(root).or_insert(next);
            result.components.insert(vector.id.into(), component);
        }
        component_offset += component_ids.len();
        result.groups.push(ProjectionGroup {
            model_identity: model_identity.into(),
            dimensions,
            count,
            explained_variance: [0.0, 0.0], // Not a PCA estimate.
            degenerate: variance.iter().any(|value| *value <= EPS),
            converged: true,
        });
    }
    Some(result)
}

struct AnnVector<'a> {
    id: &'a str,
    values: &'a [f32],
    inv_norm: f64,
}
struct ProjectionPartition {
    leaves: Vec<Vec<usize>>,
    assignment: Vec<usize>,
}

fn stable_seed(value: &str) -> u64 {
    value.bytes().fold(0xcbf29ce484222325, |state, byte| {
        (state ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    })
}

fn sparse_axis(dimensions: usize, mut state: u64) -> [(usize, f64); 16] {
    // Sparse signed projection keeps tree construction O(16 N log N),
    // independent of embedding dimensionality. Repeated dimensions are valid.
    std::array::from_fn(|_| {
        state = state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^= z >> 31;
        (
            (z as usize) % dimensions,
            if z & (1 << 63) == 0 { 1.0 } else { -1.0 },
        )
    })
}

fn ann_projection(vector: &AnnVector<'_>, axis: &[(usize, f64); 16]) -> f64 {
    axis.iter()
        .map(|(d, sign)| f64::from(vector.values[*d]) * sign)
        .sum::<f64>()
        * vector.inv_norm
}

fn partition_ann(
    group: &[AnnVector<'_>],
    indices: &mut [usize],
    seed: u64,
    partition: &mut ProjectionPartition,
    cancelled: &impl Fn() -> bool,
) -> Option<()> {
    if cancelled() {
        return None;
    }
    if indices.len() <= 16 {
        let leaf = partition.leaves.len();
        for index in indices.iter() {
            partition.assignment[*index] = leaf;
        }
        partition.leaves.push(indices.to_vec());
        return Some(());
    }
    let axis = sparse_axis(group[0].values.len(), seed);
    let mut projected: Vec<_> = indices
        .iter()
        .map(|index| (*index, ann_projection(&group[*index], &axis)))
        .collect();
    let middle = indices.len() / 2;
    projected.select_nth_unstable_by(middle, |a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    for (index, (native, _)) in indices.iter_mut().zip(projected) {
        *index = native;
    }
    let (left, right) = indices.split_at_mut(middle);
    partition_ann(
        group,
        left,
        seed.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(1),
        partition,
        cancelled,
    )?;
    partition_ann(
        group,
        right,
        seed.wrapping_mul(0x9e3779b97f4a7c15).wrapping_add(2),
        partition,
        cancelled,
    )
}

fn ann_cosine(a: &AnnVector<'_>, b: &AnnVector<'_>) -> f64 {
    // Four independent accumulators avoid a single long dependency chain.
    let mut sums = [0.0; 4];
    let mut left = a.values.chunks_exact(4);
    let mut right = b.values.chunks_exact(4);
    for (a, b) in left.by_ref().zip(right.by_ref()) {
        for d in 0..4 {
            sums[d] += f64::from(a[d]) * f64::from(b[d]);
        }
    }
    let tail: f64 = left
        .remainder()
        .iter()
        .zip(right.remainder())
        .map(|(a, b)| f64::from(*a) * f64::from(*b))
        .sum();
    ((sums.iter().sum::<f64>() + tail) * a.inv_norm * b.inv_norm).clamp(-1.0, 1.0)
}

fn ann_component(parents: &mut [usize], mut index: usize) -> usize {
    while parents[index] != index {
        parents[index] = parents[parents[index]];
        index = parents[index];
    }
    index
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(a, b)| a * b).sum()
}

fn normalize(a: &mut [f64]) -> bool {
    let norm = dot(a, a).sqrt();
    if !norm.is_finite() || norm <= EPS {
        return false;
    }
    for x in a {
        *x /= norm;
    }
    true
}

fn orthogonalize(a: &mut [f64], previous: &[Vec<f64>]) {
    // Reorthogonalization limits numerical leakage of PC1 into PC2.
    for _ in 0..2 {
        for axis in previous {
            let projection = dot(a, axis);
            for (x, y) in a.iter_mut().zip(axis) {
                *x -= projection * y;
            }
        }
    }
}

fn covariance_product(
    rows: &[Vec<f64>],
    axis: &[f64],
    cancelled: &impl Fn() -> bool,
) -> Option<Vec<f64>> {
    let mut output = vec![0.0; axis.len()];
    for (index, row) in rows.iter().enumerate() {
        if index % 32 == 0 && cancelled() {
            return None;
        }
        let score = dot(row, axis);
        for (out, x) in output.iter_mut().zip(row) {
            *out += score * x;
        }
    }
    Some(output)
}

fn pca(
    group: &[Normalized],
    cancelled: &impl Fn() -> bool,
) -> Option<(BTreeMap<String, Position>, [f64; 2], bool)> {
    let count = group.len();
    let dimensions = group[0].values.len();
    let mut mean = vec![0.0; dimensions];
    for vector in group {
        for (out, x) in mean.iter_mut().zip(&vector.values) {
            *out += x / count as f64;
        }
    }
    let rows: Vec<Vec<f64>> = group
        .iter()
        .map(|vector| {
            vector
                .values
                .iter()
                .zip(&mean)
                .map(|(x, mean)| x - mean)
                .collect()
        })
        .collect();
    let total_variance: f64 = rows.iter().map(|r| dot(r, r)).sum();
    let mut axes: Vec<Vec<f64>> = Vec::new();
    let mut explained_variance = [0.0; 2];
    let mut converged = true;
    for variance in &mut explained_variance {
        if axes.len() >= dimensions || total_variance <= EPS {
            break;
        }
        // Starts: maximum remaining diagonal variance plus two fixed dense
        // seeds. Multiple starts avoid an unlucky basis vector orthogonal to
        // the leading eigenspace. A residual check reports convergence honestly.
        let mut diagonal: Vec<(usize, f64)> = (0..dimensions)
            .map(|d| (d, rows.iter().map(|r| r[d] * r[d]).sum()))
            .collect();
        diagonal.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        let mut best_axis = vec![0.0; dimensions];
        let mut best_eigenvalue = 0.0;
        let mut best_converged = true;
        for seed in 0..3 {
            let mut axis = vec![0.0; dimensions];
            if seed == 0 {
                // Search a basis that survives deflation.
                for (index, _) in &diagonal {
                    axis.fill(0.0);
                    axis[*index] = 1.0;
                    orthogonalize(&mut axis, &axes);
                    if normalize(&mut axis) {
                        break;
                    }
                }
            } else {
                // Fixed integer PRNG, unrelated to user's bytes or OS entropy.
                let mut state: u64 = 0x9e3779b97f4a7c15 ^ seed as u64;
                for x in &mut axis {
                    state ^= state << 13;
                    state ^= state >> 7;
                    state ^= state << 17;
                    *x = (state >> 11) as f64 / ((1_u64 << 53) as f64) - 0.5;
                }
                orthogonalize(&mut axis, &axes);
                if !normalize(&mut axis) {
                    continue;
                }
            }
            for _ in 0..MAX_ITERATIONS {
                let mut next = covariance_product(&rows, &axis, cancelled)?;
                orthogonalize(&mut next, &axes);
                if !normalize(&mut next) {
                    break;
                }
                let difference = dot(&next, &axis).abs();
                axis = next;
                if 1.0 - difference < 1e-13 {
                    break;
                }
            }
            let mut product = covariance_product(&rows, &axis, cancelled)?;
            orthogonalize(&mut product, &axes);
            let eigenvalue = dot(&axis, &product).max(0.0);
            let residual = product
                .iter()
                .zip(&axis)
                .map(|(x, a)| (x - eigenvalue * a).powi(2))
                .sum::<f64>()
                .sqrt();
            if eigenvalue > best_eigenvalue {
                best_eigenvalue = eigenvalue;
                best_axis = axis;
                best_converged = residual <= 1e-6 * total_variance.max(EPS);
            }
        }
        if best_eigenvalue <= EPS {
            break;
        }
        // Sign convention: earliest largest-magnitude loading is positive.
        let pivot = (0..dimensions)
            .max_by(|a, b| {
                best_axis[*a]
                    .abs()
                    .total_cmp(&best_axis[*b].abs())
                    .then_with(|| b.cmp(a))
            })
            .unwrap_or(0);
        if best_axis[pivot] < 0.0 {
            for x in &mut best_axis {
                *x = -*x;
            }
        }
        *variance = (best_eigenvalue / total_variance).clamp(0.0, 1.0);
        converged &= best_converged;
        axes.push(best_axis);
    }
    let positions = group
        .iter()
        .zip(&rows)
        .map(|(vector, row)| {
            (
                vector.id.clone(),
                Position {
                    x: axes.first().map_or(0.0, |a| dot(row, a)),
                    y: axes.get(1).map_or(0.0, |a| dot(row, a)),
                },
            )
        })
        .collect();
    Some((positions, explained_variance, converged))
}
