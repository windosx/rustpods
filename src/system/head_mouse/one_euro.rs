use std::f32::consts::PI;

/// 1-Euro adaptive low-pass filter (Casiez et al., 2012).
///
/// Filters noisy signals by adjusting cutoff frequency based on rate of change:
/// low cutoff at low velocity (strong filtering, zero jitter), high cutoff
/// during quick motions (low lag, no overshoot).
#[derive(Debug, Clone)]
pub struct OneEuroFilter {
    min_cutoff: f32,
    beta: f32,
    d_cutoff: f32,
    x_prev: Option<f32>,
    dx_prev: f32,
}

impl OneEuroFilter {
    pub fn new(min_cutoff: f32, beta: f32) -> Self {
        Self {
            min_cutoff,
            beta,
            d_cutoff: 1.0,
            x_prev: None,
            dx_prev: 0.0,
        }
    }

    /// Creates a filter tuned from a normalized smoothing slider in [0.0, 1.0].
    /// smoothing = 0.0 => min_cutoff = 8.0 (responsive)
    /// smoothing = 1.0 => min_cutoff = 0.6 (ultra smooth)
    pub fn from_smoothing(smoothing: f32) -> Self {
        let s = smoothing.clamp(0.0, 1.0);
        let min_cutoff = 8.0 + (0.6 - 8.0) * s;
        Self::new(min_cutoff, 0.02)
    }

    fn alpha(cutoff: f32, dt: f32) -> f32 {
        if dt <= 0.0 || cutoff <= 0.0 {
            return 1.0;
        }
        let tau = 1.0 / (2.0 * PI * cutoff);
        1.0 / (1.0 + tau / dt)
    }

    pub fn filter(&mut self, x: f32, dt: f32) -> f32 {
        if dt <= 0.0 {
            return self.x_prev.unwrap_or(x);
        }
        let Some(x_prev) = self.x_prev else {
            self.x_prev = Some(x);
            self.dx_prev = 0.0;
            return x;
        };

        // Filter the derivative
        let dx = (x - x_prev) / dt;
        let a_d = Self::alpha(self.d_cutoff, dt);
        let dx_hat = self.dx_prev + a_d * (dx - self.dx_prev);
        self.dx_prev = dx_hat;

        // Dynamic cutoff based on speed
        let cutoff = self.min_cutoff + self.beta * dx_hat.abs();
        let a = Self::alpha(cutoff, dt);
        let x_hat = x_prev + a * (x - x_prev);
        self.x_prev = Some(x_hat);

        x_hat
    }

    pub fn reset(&mut self) {
        self.x_prev = None;
        self.dx_prev = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_initializes_with_first_value() {
        let mut filter = OneEuroFilter::new(1.0, 0.01);
        let out = filter.filter(42.0, 0.01);
        assert!((out - 42.0).abs() < 1e-4);
    }

    #[test]
    fn filter_smooths_jitter_at_rest() {
        let mut filter = OneEuroFilter::from_smoothing(0.5);
        let dt = 0.01;
        // Warm up at 0.0
        for _ in 0..10 {
            filter.filter(0.0, dt);
        }
        // Small jitter oscillation around 0
        let mut max_abs = 0.0f32;
        for i in 0..20 {
            let noise = if i % 2 == 0 { 0.5 } else { -0.5 };
            let val = filter.filter(noise, dt);
            max_abs = max_abs.max(val.abs());
        }
        // Filter should significantly attenuate the 0.5 noise
        assert!(max_abs < 0.25, "filter did not attenuate jitter: max was {}", max_abs);
    }

    #[test]
    fn filter_tracks_fast_motion_with_low_lag() {
        let mut filter = OneEuroFilter::from_smoothing(0.35);
        let dt = 0.01;
        filter.filter(0.0, dt);
        // Fast sweep to 100.0 over 10 steps (speed = 1000/s)
        let mut last = 0.0;
        for i in 1..=10 {
            let target = i as f32 * 10.0;
            last = filter.filter(target, dt);
        }
        // High velocity raises cutoff, output should closely follow the high target
        assert!(last > 80.0, "fast motion lagged too much: got {}", last);
    }
}
