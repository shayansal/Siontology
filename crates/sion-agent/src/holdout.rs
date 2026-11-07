//! A holdout that can be asked again and again: Thresholdout (Dwork,
//! Feldman, Hardt, Pitassi, Reingold and Roth, "The reusable holdout",
//! Science 2015).
//!
//! A fixed number of confirmations, each at its share of the error allowed
//! (Bonferroni), uses a holdout up quickly: every question asked of it is a
//! question spent. Thresholdout answers most questions from the tasks
//! changes are chosen on, and asks the holdout only when the two disagree:
//! for a statistic measured on both (a change's gain in pass rate), the
//! answer is the training value while it is within a noisy threshold of the
//! holdout's, and the holdout's value with noise added when it is not. Only
//! that second kind spends the budget, so a holdout of `n` tasks answers
//! many more questions than it has confirmations, each accurate to about
//! the threshold, before it must be replaced. Noise is what keeps an
//! adaptive questioner from learning the holdout through the answers.
//!
//! The defaults scale with the holdout: noise of `1/(2√n)` and a threshold
//! four times that, the paper's proportions. The noise on the comparison
//! also spends the budget on about one question in five where the two
//! agree.
//!
//! An answer taken from the tasks changes are chosen on says only that the
//! holdout agreed with them within a noisy threshold, not that the
//! statistic is what they say: with a holdout of hundreds of tasks the
//! noise is too large for that to bound how often a change that does
//! nothing would be answered as helping. So an answer is fit to turn a
//! change down (the holdout's own value, not above the threshold), and a
//! change it does not turn down is still to be confirmed as any is.

use serde::{Deserialize, Serialize};

/// Where an answer came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The tasks changes are chosen on: the holdout agreed, and was not
    /// spent.
    Train,
    /// The holdout, with noise: it disagreed, and one of its budget went.
    Holdout,
}

/// An answer to a question.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Answer {
    /// The value answered.
    pub value: f64,
    /// Where it came from.
    pub source: Source,
}

/// The state of a reusable holdout.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Thresholdout {
    /// The threshold, `T`.
    pub threshold: f64,
    /// The scale of the noise, `σ`.
    pub sigma: f64,
    /// How many times the holdout may disagree before it is spent.
    pub budget: u32,
    /// How many times it has.
    pub used: u32,
    /// The threshold with its noise, `T + γ`, drawn again each time the
    /// holdout is spent.
    pub noisy: f64,
    /// How many questions it has been asked.
    #[serde(default)]
    pub asked: u32,
}

/// A draw from the Laplace distribution of scale `b`, from `u`, uniform in
/// (0, 1).
pub fn laplace(b: f64, u: f64) -> f64 {
    let u = u.clamp(1e-12, 1.0 - 1e-12) - 0.5;
    -b * u.signum() * (1.0 - 2.0 * u.abs()).ln()
}

impl Thresholdout {
    /// A holdout of `n` tasks that may disagree `budget` times, its noise
    /// drawn with `uniform` (values in (0, 1)).
    pub fn for_holdout(n: usize, budget: u32, uniform: &mut dyn FnMut() -> f64) -> Thresholdout {
        let sigma = 0.5 / (n.max(1) as f64).sqrt();
        let threshold = 4.0 * sigma;
        Thresholdout {
            threshold,
            sigma,
            budget,
            used: 0,
            noisy: threshold + laplace(2.0 * sigma, uniform()),
            asked: 0,
        }
    }

    /// The budget left.
    pub fn left(&self) -> u32 {
        self.budget.saturating_sub(self.used)
    }

    /// Answers a statistic measured as `train` on the tasks changes are
    /// chosen on and as `holdout` on the holdout, its noise drawn with
    /// `uniform`: none once the budget is spent.
    pub fn answer(
        &mut self,
        train: f64,
        holdout: f64,
        uniform: &mut dyn FnMut() -> f64,
    ) -> Option<Answer> {
        if self.left() == 0 {
            return None;
        }
        self.asked = self.asked.saturating_add(1);
        let eta = laplace(4.0 * self.sigma, uniform());
        if (holdout - train).abs() > self.noisy + eta {
            self.used += 1;
            self.noisy = self.threshold + laplace(2.0 * self.sigma, uniform());
            Some(Answer {
                value: holdout + laplace(self.sigma, uniform()),
                source: Source::Holdout,
            })
        } else {
            Some(Answer {
                value: train,
                source: Source::Train,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic stream of uniforms in (0, 1).
    fn uniforms(seed: u64) -> impl FnMut() -> f64 {
        let mut state = seed;
        move || {
            state = state
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            ((state >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        }
    }

    #[test]
    fn laplace_draws_have_the_scale_asked() {
        let mut u = uniforms(7);
        let n = 20_000;
        let mean_abs: f64 = (0..n).map(|_| laplace(0.3, u()).abs()).sum::<f64>() / n as f64;
        // The mean of |X| for Laplace(b) is b.
        assert!((mean_abs - 0.3).abs() < 0.02, "{mean_abs}");
        assert_eq!(laplace(1.0, 0.5), 0.0);
        assert!(laplace(1.0, 0.9) > 0.0 && laplace(1.0, 0.1) < 0.0);
    }

    #[test]
    fn agreement_is_answered_from_training_and_spends_little() {
        let mut u = uniforms(1);
        let mut holdout = Thresholdout::for_holdout(400, 1_000, &mut u);
        assert!((holdout.sigma - 0.025).abs() < 1e-12);
        assert!((holdout.threshold - 0.1).abs() < 1e-12);
        for _ in 0..1_000 {
            let answer = holdout.answer(0.20, 0.19, &mut u).unwrap();
            if answer.source == Source::Train {
                assert_eq!(answer.value, 0.20);
            }
        }
        // Agreement within the threshold spends the budget only when the
        // noise says otherwise: about one question in five, where every
        // question would spend a fixed confirmation.
        assert!(holdout.used < 300, "{}", holdout.used);
        let mut far = Thresholdout::for_holdout(400, 1_000, &mut u);
        for _ in 0..1_000 {
            far.answer(0.5, 0.0, &mut u).unwrap();
        }
        assert!(far.used > 950, "{}", far.used);
    }

    #[test]
    fn overfitting_is_answered_from_the_holdout_until_the_budget_is_spent() {
        let mut u = uniforms(2);
        let mut holdout = Thresholdout::for_holdout(400, 2, &mut u);
        // Better by 0.5 on training, no better on the holdout.
        let first = holdout.answer(0.5, 0.0, &mut u).unwrap();
        assert_eq!(first.source, Source::Holdout);
        assert!(first.value.abs() < 0.25, "{}", first.value);
        assert_eq!(holdout.left(), 1);
        holdout.answer(0.5, 0.0, &mut u).unwrap();
        assert_eq!(holdout.left(), 0);
        assert!(holdout.answer(0.5, 0.0, &mut u).is_none());
    }
}
