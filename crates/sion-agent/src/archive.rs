//! An archive of an agent's variants, for improvement that keeps its
//! stepping stones: every variant tried is kept with its parent, so that
//! one that did no better than its parent can still lead to one that does,
//! and the next to build on is drawn by how productive its line has been
//! rather than by its own score alone.
//!
//! The draw is the Huxley–Gödel Machine's (Wang et al., 2025, ICLR 2026).
//! A variant's *clade* is it and every variant descended from it, and the
//! clade's metaproductivity is estimated from every task run in it: the
//! runs passed over the runs made, each member weighted by its runs
//! ([`Archive::clade`]). Each variant that may be built on gets a draw from
//! Beta(τ(1 + passes), τ(1 + failures)) of its clade, and the highest draw
//! is built on next (Thompson sampling, [`Archive::pick`]). τ grows with
//! the rounds ([`temperature`]), so early draws explore and late ones
//! settle on the most productive line.
//!
//! The draws come from a seeded stream ([`Draws`]): the same seed draws
//! the same numbers, so a run can be replayed and its choices checked.

use serde::{Deserialize, Serialize};

/// One variant in an archive.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Node<T> {
    /// Its place in the archive.
    pub id: usize,
    /// The variant it was made from (none for the first).
    pub parent: Option<usize>,
    /// The variant.
    pub variant: T,
    /// Task runs it passed.
    pub passed: usize,
    /// Task runs it was tried on.
    pub total: usize,
    /// Whether it was confirmed better than the agent it was weighed
    /// against, on tasks it was not chosen on.
    #[serde(default)]
    pub confirmed: bool,
    /// What it changed, or why it was made, in a line.
    #[serde(default)]
    pub note: String,
}

impl<T> Node<T> {
    /// Whether it still fails a run of the tasks it was tried on: a
    /// variant that fails none leaves nothing on these tasks to learn from.
    pub fn failing(&self) -> bool {
        self.passed < self.total
    }

    /// Its pass rate (0 for no runs).
    pub fn rate(&self) -> f64 {
        if self.total == 0 {
            0.0
        } else {
            self.passed as f64 / self.total as f64
        }
    }
}

/// Every variant tried, each with its parent.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Archive<T> {
    /// The variants, in the order they were added: a parent always comes
    /// before its children.
    pub nodes: Vec<Node<T>>,
}

impl<T> Archive<T> {
    /// An archive holding `root`, which passed `passed` of `total` runs.
    pub fn new(root: T, passed: usize, total: usize) -> Archive<T> {
        Archive {
            nodes: vec![Node {
                id: 0,
                parent: None,
                variant: root,
                passed,
                total,
                confirmed: true,
                note: "the agent as it was".into(),
            }],
        }
    }

    /// Adds `variant`, made from `parent`; returns its place. A parent the
    /// archive does not hold is an error.
    pub fn add(
        &mut self,
        parent: usize,
        variant: T,
        passed: usize,
        total: usize,
        note: &str,
    ) -> Result<usize, String> {
        if parent >= self.nodes.len() {
            return Err(format!("the archive has no variant {parent}"));
        }
        let id = self.nodes.len();
        self.nodes.push(Node {
            id,
            parent: Some(parent),
            variant,
            passed,
            total,
            confirmed: false,
            note: note.to_string(),
        });
        Ok(id)
    }

    /// Whether `id` is `ancestor` or descends from it.
    pub fn descends(&self, id: usize, ancestor: usize) -> bool {
        let mut at = Some(id);
        while let Some(node) = at.and_then(|i| self.nodes.get(i)) {
            if node.id == ancestor {
                return true;
            }
            at = node.parent;
        }
        false
    }

    /// The runs passed and failed in `id`'s clade: it and every variant
    /// descended from it.
    pub fn clade(&self, id: usize) -> (usize, usize) {
        let mut passed = 0;
        let mut failed = 0;
        for node in &self.nodes {
            if self.descends(node.id, id) {
                passed += node.passed;
                failed += node.total.saturating_sub(node.passed);
            }
        }
        (passed, failed)
    }

    /// The variant to build on next, among those `eligible` allows: the
    /// highest of a draw from each one's clade, Beta(τ(1 + passes),
    /// τ(1 + failures)). None if none is eligible.
    pub fn pick(
        &self,
        draws: &mut Draws,
        tau: f64,
        eligible: &dyn Fn(&Node<T>) -> bool,
    ) -> Option<usize> {
        let tau = if tau.is_finite() && tau > 0.0 {
            tau
        } else {
            1.0
        };
        let mut best: Option<(usize, f64)> = None;
        for node in self.nodes.iter().filter(|n| eligible(n)) {
            let (passed, failed) = self.clade(node.id);
            let drawn = draws.beta(tau * (1.0 + passed as f64), tau * (1.0 + failed as f64));
            if best.map_or(true, |(_, b)| drawn > b) {
                best = Some((node.id, drawn));
            }
        }
        best.map(|(id, _)| id)
    }
}

/// τ for round `round` (from 0) of `rounds`: 1 at the start, rising by 1
/// a round to `rounds` at the last, so that the draws polarize as the
/// budget runs out.
pub fn temperature(round: usize, rounds: usize) -> f64 {
    1.0 + round.min(rounds.saturating_sub(1)) as f64
}

/// A seeded stream of random numbers: each is a digest of the seed and
/// its place in the stream, so the same seed always draws the same.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draws {
    seed: u64,
    drawn: u64,
}

impl Draws {
    /// A stream from `seed`.
    pub fn new(seed: u64) -> Draws {
        Draws { seed, drawn: 0 }
    }

    /// The next 64 random bits.
    pub fn next_u64(&mut self) -> u64 {
        let digest = sion_core::hash::hash_parts(
            "sion-agent/archive/draw/v1",
            &[&self.seed.to_be_bytes(), &self.drawn.to_be_bytes()],
        );
        self.drawn += 1;
        let mut bytes = [0u8; 8];
        bytes.copy_from_slice(&digest.0[..8]);
        u64::from_be_bytes(bytes)
    }

    /// A number drawn uniformly from (0, 1), neither end included.
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// A draw from the standard normal distribution (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let u = self.uniform();
        let v = self.uniform();
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }

    /// A draw from Gamma(`shape`, 1) (Marsaglia and Tsang's method; a
    /// shape below 1 is drawn as Gamma(shape + 1) times U^(1/shape)).
    pub fn gamma(&mut self, shape: f64) -> f64 {
        if !(shape.is_finite() && shape > 0.0) {
            return 0.0;
        }
        if shape < 1.0 {
            let boost = self.uniform().powf(1.0 / shape);
            return self.gamma(shape + 1.0) * boost;
        }
        let d = shape - 1.0 / 3.0;
        let c = 1.0 / (9.0 * d).sqrt();
        loop {
            let x = self.normal();
            let v = 1.0 + c * x;
            if v <= 0.0 {
                continue;
            }
            let v = v * v * v;
            let u = self.uniform();
            if u < 1.0 - 0.0331 * x.powi(4) || u.ln() < 0.5 * x * x + d * (1.0 - v + v.ln()) {
                return d * v;
            }
        }
    }

    /// A draw from Beta(`a`, `b`).
    pub fn beta(&mut self, a: f64, b: f64) -> f64 {
        let x = self.gamma(a);
        let y = self.gamma(b);
        if x + y == 0.0 {
            0.5
        } else {
            x / (x + y)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_draws_the_same_numbers() {
        let mut a = Draws::new(7);
        let mut b = Draws::new(7);
        let mut c = Draws::new(8);
        let from_a: Vec<u64> = (0..5).map(|_| a.next_u64()).collect();
        let from_b: Vec<u64> = (0..5).map(|_| b.next_u64()).collect();
        let from_c: Vec<u64> = (0..5).map(|_| c.next_u64()).collect();
        assert_eq!(from_a, from_b);
        assert_ne!(from_a, from_c);
    }

    #[test]
    fn beta_draws_centre_on_their_mean() {
        let mut draws = Draws::new(1);
        let n = 4_000;
        let mean: f64 = (0..n).map(|_| draws.beta(3.0, 7.0)).sum::<f64>() / n as f64;
        assert!((mean - 0.3).abs() < 0.02, "{mean}");
        let small: f64 = (0..n).map(|_| draws.beta(0.5, 0.5)).sum::<f64>() / n as f64;
        assert!((small - 0.5).abs() < 0.03, "{small}");
        let u: Vec<f64> = (0..n).map(|_| draws.uniform()).collect();
        assert!(u.iter().all(|x| *x > 0.0 && *x < 1.0));
    }

    #[test]
    fn a_clade_counts_every_run_of_its_descendants() {
        let mut archive = Archive::new("root", 2, 10);
        let a = archive.add(0, "a", 3, 10, "a").unwrap();
        let b = archive.add(a, "b", 9, 10, "b").unwrap();
        let c = archive.add(0, "c", 1, 10, "c").unwrap();
        assert_eq!(archive.clade(b), (9, 1));
        assert_eq!(archive.clade(a), (12, 8));
        assert_eq!(archive.clade(c), (1, 9));
        assert_eq!(archive.clade(0), (15, 25));
        assert!(archive.descends(b, 0) && archive.descends(b, a) && !archive.descends(b, c));
        assert!(archive.add(99, "x", 0, 0, "").is_err());
    }

    #[test]
    fn a_productive_line_is_built_on_more_often_than_its_score_alone_says() {
        // "a" scores no better than "c" itself, but its child "b" did
        // well: a's clade is the more productive, so a is drawn more often
        // than c, and b most of all.
        let mut archive = Archive::new("root", 2, 10);
        let a = archive.add(0, "a", 2, 10, "a").unwrap();
        let b = archive.add(a, "b", 9, 10, "b").unwrap();
        let c = archive.add(0, "c", 2, 10, "c").unwrap();
        let mut draws = Draws::new(42);
        let mut counts = [0usize; 4];
        for _ in 0..1_000 {
            let picked = archive
                .pick(&mut draws, 1.0, &|n| n.id == a || n.id == c)
                .unwrap();
            counts[picked] += 1;
        }
        assert!(counts[a] > counts[c] * 2, "{counts:?}");
        // With every variant eligible, the best line's tip is drawn most.
        let mut counts = [0usize; 4];
        for _ in 0..1_000 {
            counts[archive.pick(&mut draws, 1.0, &|_| true).unwrap()] += 1;
        }
        assert!(counts[b] > counts[a] && counts[b] > counts[c], "{counts:?}");
        // None eligible: nothing is drawn.
        assert_eq!(archive.pick(&mut draws, 1.0, &|_| false), None);
    }

    #[test]
    fn the_temperature_rises_over_the_rounds() {
        assert_eq!(temperature(0, 1), 1.0);
        assert_eq!(temperature(0, 5), 1.0);
        assert!(temperature(4, 5) > temperature(1, 5));
        assert_eq!(temperature(9, 5), temperature(4, 5));
    }
}
