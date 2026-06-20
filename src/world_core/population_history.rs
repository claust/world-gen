//! Worldwide plant-population time series.
//!
//! The Population Lens reads a region around the camera *now*; this records the
//! whole world's plant counts *over time*, so the player can watch the stage mix
//! and per-species totals develop across a long run. It is a thin store: the
//! runtime takes a periodic census (per-species × per-stage counts) and appends
//! it here, keyed by the world clock's monotonic `total_hours`. The graph panel
//! is a pure reader of these samples.
//!
//! To stay bounded over an arbitrarily long game, the history caps its retained
//! sample count: when full it decimates (drops every other sample) and doubles
//! the sampling interval, so the curve keeps its shape while the memory and the
//! census cadence both stay flat.

use serde::{Deserialize, Serialize};

use crate::world_core::lifecycle::GrowthStage;

/// Growth stages tracked per sample, in canonical order.
pub const STAGE_COUNT: usize = 4;

/// Display labels for the four stages, in the [`STAGE_COUNT`] index order
/// ([`GrowthStage::Seedling`]=0 … [`GrowthStage::Dead`]=3).
pub const STAGE_LABELS: [&str; STAGE_COUNT] = ["Seedling", "Young", "Mature", "Dead"];

/// The fixed index a stage occupies in every per-species count array, so the
/// census, the sample accessors, and the panel all agree on column order.
pub const fn stage_index(stage: GrowthStage) -> usize {
    match stage {
        GrowthStage::Seedling => 0,
        GrowthStage::Young => 1,
        GrowthStage::Mature => 2,
        GrowthStage::Dead => 3,
    }
}

/// One worldwide census at a single sim-time instant. `per_species[s]` holds the
/// `[seedling, young, mature, dead]` counts for species id `s`; species absent
/// from the world still occupy a row of zeros so a row index always maps to the
/// same species across samples.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PopulationSample {
    /// World clock `total_hours` when this census was taken.
    pub hour: f64,
    pub per_species: Vec<[u32; STAGE_COUNT]>,
}

impl PopulationSample {
    /// Per-stage totals summed across every species.
    pub fn stage_totals(&self) -> [u64; STAGE_COUNT] {
        let mut totals = [0u64; STAGE_COUNT];
        for species in &self.per_species {
            for (acc, &c) in totals.iter_mut().zip(species.iter()) {
                *acc += c as u64;
            }
        }
        totals
    }

    /// Per-stage counts for one species id, or zeros if the id is out of range.
    pub fn species_stages(&self, species: usize) -> [u32; STAGE_COUNT] {
        self.per_species
            .get(species)
            .copied()
            .unwrap_or([0; STAGE_COUNT])
    }

    /// Total plants in this sample across all species and stages.
    pub fn total(&self) -> u64 {
        self.stage_totals().iter().sum()
    }
}

/// Default sim-hours between retained censuses. Two hours keeps early growth
/// legible; decimation widens it automatically on a long run.
const BASE_INTERVAL_HOURS: f64 = 2.0;

/// Cap on retained samples. Hitting it halves the resolution rather than growing
/// memory without bound; 1024 points is far more than a line chart can resolve.
const MAX_SAMPLES: usize = 1024;

/// A bounded, monotonically-spaced series of worldwide censuses.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PopulationHistory {
    samples: Vec<PopulationSample>,
    /// Current minimum sim-hour spacing between retained samples. Doubles each
    /// time the history decimates.
    interval_hours: f64,
}

impl Default for PopulationHistory {
    fn default() -> Self {
        Self::new()
    }
}

impl PopulationHistory {
    pub fn new() -> Self {
        Self {
            samples: Vec::new(),
            interval_hours: BASE_INTERVAL_HOURS,
        }
    }

    /// Whether a fresh census is due at sim-time `hour`. True when empty (so the
    /// first census seeds the series) or when at least `interval_hours` of sim
    /// time have elapsed since the last retained sample.
    pub fn is_due(&self, hour: f64) -> bool {
        match self.samples.last() {
            Some(last) => hour - last.hour >= self.interval_hours,
            None => true,
        }
    }

    /// Append a census at `hour`. The caller should gate this on [`is_due`] so the
    /// per-sample spacing stays regular; a sample at or before the last retained
    /// hour is ignored so the series never goes backwards. Decimates when full.
    ///
    /// [`is_due`]: Self::is_due
    pub fn record(&mut self, hour: f64, per_species: Vec<[u32; STAGE_COUNT]>) {
        if let Some(last) = self.samples.last() {
            if hour <= last.hour {
                return;
            }
        }
        self.samples.push(PopulationSample { hour, per_species });
        if self.samples.len() > MAX_SAMPLES {
            self.decimate();
        }
    }

    /// Halve the resolution: keep every other sample (always retaining the most
    /// recent, so the latest readout never jumps) and double the interval so
    /// future sampling matches the thinned spacing.
    fn decimate(&mut self) {
        // Walk from the newest so the last sample is always kept regardless of
        // parity, then restore chronological order.
        let mut kept: Vec<PopulationSample> = self
            .samples
            .drain(..)
            .rev()
            .enumerate()
            .filter(|(i, _)| i % 2 == 0)
            .map(|(_, s)| s)
            .collect();
        kept.reverse();
        self.samples = kept;
        self.interval_hours *= 2.0;
    }

    pub fn samples(&self) -> &[PopulationSample] {
        &self.samples
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// The most recent census, if any.
    pub fn latest(&self) -> Option<&PopulationSample> {
        self.samples.last()
    }

    /// Sim-hour span `(first, last)` covered by the retained samples.
    pub fn span_hours(&self) -> Option<(f64, f64)> {
        match (self.samples.first(), self.samples.last()) {
            (Some(a), Some(b)) => Some((a.hour, b.hour)),
            _ => None,
        }
    }

    /// Change in total world population over roughly the last `window` sim-hours:
    /// the newest sample's total minus the total of the newest sample at or before
    /// `last.hour - window`. If the series is shorter than the window, the earliest
    /// sample is used as the baseline, so a young world reports its growth-so-far.
    /// `None` only when there are no samples at all.
    ///
    /// `window` must be positive and finite. A non-positive window puts the cutoff at or
    /// after the newest sample, which would report no change (or a future cutoff); it is
    /// debug-asserted rather than handled, since the only caller passes a fixed week.
    pub fn delta_over_hours(&self, window: f64) -> Option<i64> {
        debug_assert!(
            window.is_finite() && window > 0.0,
            "delta_over_hours expects a positive, finite window in sim-hours, got {window}",
        );
        let last = self.samples.last()?;
        let cutoff = last.hour - window;
        let baseline = self
            .samples
            .iter()
            .rev()
            .find(|s| s.hour <= cutoff)
            .or_else(|| self.samples.first())?;
        Some(last.total() as i64 - baseline.total() as i64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(hour: f64, counts: &[[u32; STAGE_COUNT]]) -> Vec<[u32; STAGE_COUNT]> {
        let _ = hour;
        counts.to_vec()
    }

    #[test]
    fn stage_totals_and_species_accessors() {
        let s = PopulationSample {
            hour: 0.0,
            per_species: vec![[1, 2, 3, 4], [10, 20, 30, 40]],
        };
        assert_eq!(s.stage_totals(), [11, 22, 33, 44]);
        assert_eq!(s.species_stages(1), [10, 20, 30, 40]);
        assert_eq!(s.species_stages(5), [0, 0, 0, 0]);
        assert_eq!(s.total(), 110);
    }

    #[test]
    fn first_record_is_always_due_then_respects_interval() {
        let mut h = PopulationHistory::new();
        assert!(h.is_due(0.0));
        h.record(0.0, sample(0.0, &[[1, 0, 0, 0]]));
        assert!(!h.is_due(1.0));
        assert!(h.is_due(BASE_INTERVAL_HOURS));
    }

    #[test]
    fn out_of_order_samples_are_dropped() {
        let mut h = PopulationHistory::new();
        h.record(10.0, sample(10.0, &[[1, 0, 0, 0]]));
        h.record(5.0, sample(5.0, &[[9, 0, 0, 0]]));
        h.record(10.0, sample(10.0, &[[9, 0, 0, 0]]));
        assert_eq!(h.len(), 1);
        assert_eq!(h.latest().unwrap().hour, 10.0);
    }

    #[test]
    fn decimation_bounds_length_and_keeps_latest() {
        let mut h = PopulationHistory::new();
        // Force the interval to zero so every record lands regardless of spacing;
        // this test only checks the length cap and latest-retention. The interval
        // widening is covered by `decimation_widens_interval_from_nonzero_base`.
        h.interval_hours = 0.0;
        for i in 0..(MAX_SAMPLES + 1) {
            let hour = i as f64;
            h.record(hour, sample(hour, &[[i as u32, 0, 0, 0]]));
        }
        assert!(h.len() <= MAX_SAMPLES);
        // The newest sample survives decimation.
        assert_eq!(h.latest().unwrap().hour, MAX_SAMPLES as f64);
    }

    #[test]
    fn delta_over_window_uses_baseline_at_or_before_cutoff() {
        let mut h = PopulationHistory::new();
        // One sample per "day" (24h), totals climbing by 10 each day.
        for day in 0..10 {
            let hour = day as f64 * 24.0;
            let total = (day + 1) as u32 * 10;
            h.record(hour, sample(hour, &[[total, 0, 0, 0]]));
        }
        // Latest is day 9 (total 100) at hour 216; one week back is hour 48 (day 2,
        // total 30). 100 - 30 = 70.
        assert_eq!(h.delta_over_hours(24.0 * 7.0), Some(70));
    }

    #[test]
    fn delta_over_window_falls_back_to_earliest_when_series_is_young() {
        let mut h = PopulationHistory::new();
        h.record(0.0, sample(0.0, &[[5, 0, 0, 0]]));
        h.record(48.0, sample(48.0, &[[12, 0, 0, 0]]));
        // No sample a week before hour 48, so the earliest (total 5) is the baseline.
        assert_eq!(h.delta_over_hours(24.0 * 7.0), Some(7));
        assert_eq!(PopulationHistory::new().delta_over_hours(24.0 * 7.0), None);
    }

    #[test]
    fn decimation_widens_interval_from_nonzero_base() {
        let mut h = PopulationHistory::new();
        for i in 0..(MAX_SAMPLES + 1) {
            // Space samples wide enough that each is always due.
            let hour = i as f64 * BASE_INTERVAL_HOURS;
            h.record(hour, sample(hour, &[[1, 0, 0, 0]]));
        }
        assert!(h.len() <= MAX_SAMPLES);
        assert_eq!(h.interval_hours, BASE_INTERVAL_HOURS * 2.0);
    }
}
