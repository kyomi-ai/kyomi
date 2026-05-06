// SPDX-License-Identifier: AGPL-3.0-or-later

//! Time series forecasting — stateless pure computation module.
//!
//! Implements 5 forecasting models:
//!   - Linear regression (OLS with Student's t prediction intervals)
//!   - Exponential growth (log-transform linear regression)
//!   - Logistic growth (S-curve with delta method prediction intervals)
//!   - ETS (Exponential smoothing: Holt-Winters with additive seasonality)
//!   - Auto-selection (cross-validation across candidate models)
//!
//! This is a pure computation module — no async, no DB, no external services.
//! All functions take `&[f64]` and return results synchronously.
//!
//! Implements the same algorithms as `~/repos/quackstats/` (our Rust DuckDB
//! extension), which is the authoritative reference for forecast behavior.
//! Key alignment: Gauss-Newton fitting with post-hoc L adjustment for logistic,
//! delta method prediction intervals, Student's t-distribution throughout.

use std::f64::consts::PI;

use statrs::distribution::{ContinuousCDF, StudentsT};
use tracing::info;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Minimum number of data points required for any forecast.
const MIN_DATA_POINTS: usize = 4;

/// Minimum data points required for seasonality detection.
const MIN_SEASONALITY_POINTS: usize = 8;

/// Minimum spectral power (fraction of max) to consider a period as candidate.
const PERIODOGRAM_POWER_THRESHOLD: f64 = 0.01;

/// Maximum number of candidate periods to validate with autocorrelation.
const MAX_SEASONALITY_CANDIDATES: usize = 20;

/// Minimum autocorrelation strength to report a detected seasonal period.
const MIN_SEASONALITY_STRENGTH: f64 = 0.1;

/// Minimum strength for seasonality to be used in ETS model selection.
const SEASONALITY_STRENGTH_THRESHOLD: f64 = 0.3;

/// Second-half growth must be below this fraction of first-half to indicate deceleration.
const DECELERATION_RATIO: f64 = 0.7;

/// For upward trends, first forecast must not drop below this fraction of last obs.
const FORECAST_DROP_THRESHOLD: f64 = 0.9;

/// Valid model names.
const VALID_MODELS: &[&str] = &["auto", "ets", "linear", "exponential", "logistic"];

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// A single forecast point with prediction interval.
#[derive(Debug, Clone)]
pub struct ForecastPoint {
    /// Step number (1-based).
    pub step: u32,
    /// Point forecast value.
    pub forecast: f64,
    /// Lower bound of prediction interval.
    pub lower_bound: f64,
    /// Upper bound of prediction interval.
    pub upper_bound: f64,
}

/// Result of a forecast operation.
#[derive(Debug, Clone)]
pub struct ForecastResult {
    /// Name of the model used (e.g., "linear", "ets").
    pub model_used: String,
    /// Forecast points with prediction intervals.
    pub forecast: Vec<ForecastPoint>,
    /// Number of input data points.
    pub data_points: usize,
    /// Error message if forecast failed.
    pub error: Option<String>,
}

impl ForecastResult {
    /// Create an error result.
    fn err(msg: impl Into<String>) -> Self {
        Self {
            model_used: String::new(),
            forecast: Vec::new(),
            data_points: 0,
            error: Some(msg.into()),
        }
    }
}

/// A detected seasonal period candidate.
#[derive(Debug, Clone)]
pub struct SeasonalityCandidate {
    /// Period length in data points.
    pub period: usize,
    /// Strength of the seasonal signal (0.0 to 1.0).
    pub strength: f64,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Run time series forecasting.
///
/// # Arguments
///
/// * `values` — Time series values (evenly spaced, chronological order).
/// * `horizon` — Number of future periods to forecast.
/// * `confidence_level` — Width of prediction intervals (0–1), e.g., 0.95.
/// * `model` — One of "auto", "ets", "linear", "exponential", "logistic".
/// * `seasonal_period` — Override seasonal period (`None` = auto-detect).
pub fn forecast(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
    model: &str,
    seasonal_period: Option<usize>,
) -> ForecastResult {
    // --- Input validation ---
    if !VALID_MODELS.contains(&model) {
        let valid = VALID_MODELS.to_vec().join(", ");
        return ForecastResult::err(format!(
            "Unknown model '{model}'. Valid models: {valid}"
        ));
    }

    if horizon < 1 {
        return ForecastResult::err("Horizon must be at least 1.");
    }

    if !(0.01..=0.99).contains(&confidence_level) {
        return ForecastResult::err(
            "Confidence level must be between 0.01 and 0.99 (1% to 99%).",
        );
    }

    // Clean input: reject NaN and Inf
    match clean_values(values) {
        Ok(arr) => {
            let n = arr.len();
            if n < MIN_DATA_POINTS {
                return ForecastResult::err(format!(
                    "Need at least {MIN_DATA_POINTS} data points for forecasting, got {n}."
                ));
            }

            // --- Dispatch to model ---
            let result = match model {
                "linear" => forecast_linear(&arr, horizon, confidence_level),
                "exponential" => forecast_exponential(&arr, horizon, confidence_level),
                "logistic" => forecast_logistic(&arr, horizon, confidence_level),
                "ets" => forecast_ets_dispatch(&arr, horizon, confidence_level, seasonal_period),
                "auto" => forecast_auto(&arr, horizon, confidence_level, seasonal_period),
                _ => unreachable!("validated above"),
            };

            match result {
                Ok((model_used, points)) => ForecastResult {
                    model_used,
                    forecast: points,
                    data_points: n,
                    error: None,
                },
                Err(msg) => ForecastResult {
                    model_used: String::new(),
                    forecast: Vec::new(),
                    data_points: n,
                    error: Some(msg),
                },
            }
        }
        Err(msg) => ForecastResult::err(msg),
    }
}

// ---------------------------------------------------------------------------
// Input cleaning
// ---------------------------------------------------------------------------

/// Convert input slice to a cleaned `Vec<f64>`, rejecting NaN and Inf.
fn clean_values(values: &[f64]) -> Result<Vec<f64>, String> {
    if values.is_empty() {
        return Err("Input values list is empty.".into());
    }

    for (i, &v) in values.iter().enumerate() {
        if v.is_nan() {
            return Err(format!(
                "Input contains NaN values (at index {i}). \
                 Remove or interpolate missing data before forecasting."
            ));
        }
        if v.is_infinite() {
            return Err(format!("Input contains infinite values (at index {i})."));
        }
    }

    Ok(values.to_vec())
}

// ---------------------------------------------------------------------------
// OLS Linear Regression helpers
// ---------------------------------------------------------------------------

/// Result of an OLS linear regression: y = intercept + slope * x.
struct LinRegResult {
    slope: f64,
    intercept: f64,
    /// Residual standard error.
    se: f64,
    /// Mean of x values.
    x_mean: f64,
    /// Sum of squared deviations of x from its mean.
    ss_x: f64,
    /// Degrees of freedom (n - 2).
    dof: usize,
}

/// Compute OLS linear regression on (x=0..n-1, y=values).
///
/// Returns `None` if degrees of freedom < 1 (need at least 3 data points).
fn ols_regression(values: &[f64]) -> Option<LinRegResult> {
    let n = values.len();
    if n < 3 {
        return None;
    }

    let n_f = n as f64;

    // x = 0, 1, ..., n-1
    let sum_x: f64 = (0..n).map(|i| i as f64).sum();
    let sum_y: f64 = values.iter().sum();
    let sum_xy: f64 = values
        .iter()
        .enumerate()
        .map(|(i, &y)| i as f64 * y)
        .sum();
    let sum_x2: f64 = (0..n).map(|i| (i as f64) * (i as f64)).sum();

    let x_mean = sum_x / n_f;
    let ss_x = sum_x2 - sum_x * sum_x / n_f;

    if ss_x.abs() < 1e-15 {
        // All x values are the same (impossible for 0..n-1, but defensive)
        return None;
    }

    let slope = (sum_xy - sum_x * sum_y / n_f) / ss_x;
    let intercept = sum_y / n_f - slope * x_mean;

    // Residual standard error
    let mut ss_res = 0.0;
    for (i, &y) in values.iter().enumerate() {
        let y_hat = intercept + slope * i as f64;
        let residual = y - y_hat;
        ss_res += residual * residual;
    }

    let dof = n - 2;
    let se = (ss_res / dof as f64).sqrt();

    Some(LinRegResult {
        slope,
        intercept,
        se,
        x_mean,
        ss_x,
        dof,
    })
}

// ---------------------------------------------------------------------------
// Model: Linear Regression
// ---------------------------------------------------------------------------

/// OLS linear regression: y = intercept + slope * x.
///
/// Prediction intervals use Student's t-distribution:
///     PI = y_hat +/- t * se * sqrt(1 + 1/n + (x_pred - x_mean)^2 / SS_x)
fn forecast_linear(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
) -> Result<(String, Vec<ForecastPoint>), String> {
    let n = values.len();
    let reg = ols_regression(values)
        .ok_or("Not enough data points for linear regression (need at least 3).")?;

    // t-value for the given confidence level
    let alpha = 1.0 - confidence_level;
    let t_dist = StudentsT::new(0.0, 1.0, reg.dof as f64)
        .map_err(|e| format!("Failed to create t-distribution: {e}"))?;
    let t_val = t_dist.inverse_cdf(1.0 - alpha / 2.0);

    let mut forecasts = Vec::with_capacity(horizon);
    for i in 1..=horizon {
        let x_pred = (n - 1 + i) as f64;
        let y_pred = reg.intercept + reg.slope * x_pred;

        // Prediction interval (for a new observation, not the mean)
        let margin = t_val
            * reg.se
            * (1.0 + 1.0 / n as f64 + (x_pred - reg.x_mean).powi(2) / reg.ss_x).sqrt();

        forecasts.push(ForecastPoint {
            step: i as u32,
            forecast: y_pred,
            lower_bound: y_pred - margin,
            upper_bound: y_pred + margin,
        });
    }

    Ok(("linear".into(), forecasts))
}

// ---------------------------------------------------------------------------
// Model: Exponential Growth
// ---------------------------------------------------------------------------

/// Exponential growth: y = a * exp(b * x).
///
/// Fitted via log-transform OLS: ln(y) = ln(a) + b*x.
/// All values must be > 0. Prediction intervals are back-transformed
/// from log space, producing naturally asymmetric intervals.
fn forecast_exponential(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
) -> Result<(String, Vec<ForecastPoint>), String> {
    if values.iter().any(|&v| v <= 0.0) {
        return Err("Exponential model requires all values to be positive (> 0).".into());
    }

    let n = values.len();
    let log_values: Vec<f64> = values.iter().map(|&v| v.ln()).collect();

    let reg = ols_regression(&log_values)
        .ok_or("Not enough data points for exponential model (need at least 3).")?;

    let alpha = 1.0 - confidence_level;
    let t_dist = StudentsT::new(0.0, 1.0, reg.dof as f64)
        .map_err(|e| format!("Failed to create t-distribution: {e}"))?;
    let t_val = t_dist.inverse_cdf(1.0 - alpha / 2.0);

    let mut forecasts = Vec::with_capacity(horizon);
    for i in 1..=horizon {
        let x_pred = (n - 1 + i) as f64;
        let log_y_pred = reg.intercept + reg.slope * x_pred;
        let margin_log = t_val
            * reg.se
            * (1.0 + 1.0 / n as f64 + (x_pred - reg.x_mean).powi(2) / reg.ss_x).sqrt();

        // Back-transform from log space (naturally asymmetric intervals)
        let y_pred = log_y_pred.exp();
        let lower = (log_y_pred - margin_log).exp();
        let upper = (log_y_pred + margin_log).exp();

        forecasts.push(ForecastPoint {
            step: i as u32,
            forecast: y_pred,
            lower_bound: lower,
            upper_bound: upper,
        });
    }

    Ok(("exponential".into(), forecasts))
}

// ---------------------------------------------------------------------------
// Model: Logistic Growth (S-curve)
// ---------------------------------------------------------------------------

/// Logistic function: y = L / (1 + exp(-k * (x - x0))).
fn logistic_func(x: f64, l: f64, k: f64, x0: f64) -> f64 {
    l / (1.0 + (-k * (x - x0)).exp())
}

/// Partial derivatives of the logistic function with respect to (L, k, x0).
fn logistic_jacobian(x: f64, l: f64, k: f64, x0: f64) -> [f64; 3] {
    let exp_term = (-k * (x - x0)).exp();
    let denom = 1.0 + exp_term;
    let denom_sq = denom * denom;

    let dl = 1.0 / denom;
    let dk = l * (x - x0) * exp_term / denom_sq;
    let dx0 = -l * k * exp_term / denom_sq;

    [dl, dk, dx0]
}

/// Fit logistic curve using Gauss-Newton with line search.
///
/// Returns `(L, k, x0, residual_variance)` on success.
fn fit_logistic(
    values: &[f64],
) -> Result<(f64, f64, f64, f64), String> {
    let n = values.len();
    let x_vals: Vec<f64> = (0..n).map(|i| i as f64).collect();

    let y_max = values.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let y_min = values.iter().cloned().fold(f64::INFINITY, f64::min);

    if (y_max - y_min).abs() < 1e-10 {
        return Err("Logistic model cannot fit constant data.".into());
    }

    // Initial guesses (data-driven)
    let mut l = 2.0 * y_max;
    let mut x0 = n as f64 / 2.0;

    // Estimate k from steepest growth
    let diffs: Vec<f64> = values.windows(2).map(|w| w[1] - w[0]).collect();
    let max_growth = diffs.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mut k = if l > 0.0 {
        (4.0 * max_growth / l).max(0.01)
    } else {
        0.1
    };

    // Bounds
    let l_lower = y_max * 1.001;
    let l_upper = y_max * 100.0;
    let k_lower = 1e-6;
    let k_upper = 10.0;
    let x0_lower = -(n as f64) * 2.0;
    let x0_upper = n as f64 * 3.0;

    // Clamp initial guesses
    l = l.clamp(l_lower, l_upper);
    k = k.clamp(k_lower, k_upper);
    x0 = x0.clamp(x0_lower, x0_upper);

    // Gauss-Newton iterations
    let max_iter = 200;
    let tol = 1e-10;

    for _iter in 0..max_iter {
        // Build J^T J and J^T r
        let mut jtj = [[0.0f64; 3]; 3];
        let mut jtr = [0.0f64; 3];
        let mut sse = 0.0;

        for (i, &y_obs) in values.iter().enumerate() {
            let x = x_vals[i];
            let y_pred = logistic_func(x, l, k, x0);
            let residual = y_obs - y_pred;
            sse += residual * residual;

            let jac = logistic_jacobian(x, l, k, x0);

            for a in 0..3 {
                jtr[a] += jac[a] * residual;
                for b in 0..3 {
                    jtj[a][b] += jac[a] * jac[b];
                }
            }
        }

        // Add Levenberg-Marquardt damping
        let lambda = 1e-4 * (jtj[0][0] + jtj[1][1] + jtj[2][2]) / 3.0;
        for (i, row) in jtj.iter_mut().enumerate() {
            row[i] += lambda.max(1e-10);
        }

        // Solve 3x3 system via Cramer's rule
        let delta = match solve_3x3(&jtj, &jtr) {
            Some(d) => d,
            None => break, // Singular matrix
        };

        // Line search: try full step, then halve
        let mut step_size = 1.0;
        let mut best_params = (l, k, x0);

        for _ in 0..10 {
            let l_new = (l + step_size * delta[0]).clamp(l_lower, l_upper);
            let k_new = (k + step_size * delta[1]).clamp(k_lower, k_upper);
            let x0_new = (x0 + step_size * delta[2]).clamp(x0_lower, x0_upper);

            let new_sse: f64 = values
                .iter()
                .enumerate()
                .map(|(i, &y)| {
                    let r = y - logistic_func(x_vals[i], l_new, k_new, x0_new);
                    r * r
                })
                .sum();

            if new_sse < sse {
                best_params = (l_new, k_new, x0_new);
                break;
            }
            step_size *= 0.5;
        }

        let (l_new, k_new, x0_new) = best_params;

        // Convergence check
        let param_change = (l_new - l).powi(2) + (k_new - k).powi(2) + (x0_new - x0).powi(2);
        l = l_new;
        k = k_new;
        x0 = x0_new;

        if param_change.sqrt() < tol {
            break;
        }
    }

    // Compute residual variance
    let sse: f64 = values
        .iter()
        .enumerate()
        .map(|(i, &y)| {
            let r = y - logistic_func(x_vals[i], l, k, x0);
            r * r
        })
        .sum();
    let residual_var = if n > 3 { sse / (n - 3) as f64 } else { sse };

    Ok((l, k, x0, residual_var))
}

/// Solve a 3x3 linear system Ax = b using Cramer's rule.
fn solve_3x3(a: &[[f64; 3]; 3], b: &[f64; 3]) -> Option<[f64; 3]> {
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);

    if det.abs() < 1e-30 {
        return None;
    }

    let x0 = (b[0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (b[1] * a[2][2] - a[1][2] * b[2])
        + a[0][2] * (b[1] * a[2][1] - a[1][1] * b[2]))
        / det;

    let x1 = (a[0][0] * (b[1] * a[2][2] - a[1][2] * b[2])
        - b[0] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * b[2] - b[1] * a[2][0]))
        / det;

    let x2 = (a[0][0] * (a[1][1] * b[2] - b[1] * a[2][1])
        - a[0][1] * (a[1][0] * b[2] - b[1] * a[2][0])
        + b[0] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]))
        / det;

    Some([x0, x1, x2])
}

/// Logistic growth forecast with delta method prediction intervals.
///
/// Uses the same approach as QuackStats: first-order Taylor expansion of
/// parameter uncertainty plus observation noise. Falls back to residual-only
/// intervals if the covariance matrix is singular.
fn forecast_logistic(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
) -> Result<(String, Vec<ForecastPoint>), String> {
    let n = values.len();

    let (mut l_fit, k_fit, x0_fit, _) = fit_logistic(values)?;

    // Post-hoc capacity adjustment (matches QuackStats):
    // Ensure the model doesn't predict below the last observation.
    // The logistic function asymptotically approaches L but never reaches it,
    // so if L ≈ y_max the forecast can visually "drop" below the last point.
    let x_last = (n - 1) as f64;
    let exp_last = (-k_fit * (x_last - x0_fit)).exp();
    let f_last = l_fit / (1.0 + exp_last);
    let y_last = values[n - 1];
    if f_last < y_last {
        // Solve: y_last = L_new / (1 + exp_last) → L_new = y_last * (1 + exp_last)
        // Add tiny margin to avoid floating-point edge case
        l_fit = y_last * (1.0 + exp_last) * 1.001;
    }

    // Recompute residual variance with the (potentially adjusted) L
    let x_vals: Vec<f64> = (0..n).map(|i| i as f64).collect();
    let df = n as f64 - 3.0;
    if df <= 0.0 {
        return Err("Need more than 3 data points for logistic prediction intervals.".into());
    }

    let ss_res: f64 = x_vals
        .iter()
        .zip(values.iter())
        .map(|(&xi, &yi)| {
            let pred = logistic_func(xi, l_fit, k_fit, x0_fit);
            (yi - pred).powi(2)
        })
        .sum();
    let s_sq = ss_res / df;

    // Build the data Jacobian w.r.t. [L, k, x0] using the final (adjusted) L.
    let mut jtj = [[0.0f64; 3]; 3];
    for &xi in &x_vals {
        let jac = logistic_jacobian(xi, l_fit, k_fit, x0_fit);
        for a in 0..3 {
            for b in 0..3 {
                jtj[a][b] += jac[a] * jac[b];
            }
        }
    }

    // If J^T*J is singular (near-perfect fit or degenerate data), fall back to
    // residual-only prediction intervals (no parameter uncertainty component).
    let param_cov = invert_3x3(&jtj).map(|inv| {
        let mut cov = [[0.0f64; 3]; 3];
        for a in 0..3 {
            for b in 0..3 {
                cov[a][b] = inv[a][b] * s_sq;
            }
        }
        cov
    });

    // Check that covariance entries are finite; discard if not
    let param_cov = param_cov.filter(|cov| {
        cov.iter().all(|row| row.iter().all(|&val| val.is_finite()))
    });

    // Student's t-distribution with n-3 degrees of freedom
    let t_dist = StudentsT::new(0.0, 1.0, df)
        .map_err(|e| format!("Failed to create t-distribution: {e}"))?;
    let ci_alpha = 1.0 - confidence_level;
    let t_value = t_dist.inverse_cdf(1.0 - ci_alpha / 2.0);

    let mut forecasts = Vec::with_capacity(horizon);
    for i in 1..=horizon {
        let x_pred = (n - 1 + i) as f64;
        let exp_term = (-k_fit * (x_pred - x0_fit)).exp();
        let denom = 1.0 + exp_term;
        let denom_sq = denom * denom;
        let y_hat = l_fit / denom;

        // Prediction variance via delta method: Var = J_pred^T * Cov * J_pred + s²
        // If covariance unavailable, use residual-only: Var = s²
        let pred_var = if let Some(ref cov) = param_cov {
            // Jacobian of f w.r.t. [L, k, x0] at this forecast point
            let j_pred = [
                1.0 / denom,                                  // ∂f/∂L
                l_fit * (x_pred - x0_fit) * exp_term / denom_sq, // ∂f/∂k
                -l_fit * k_fit * exp_term / denom_sq,          // ∂f/∂x0
            ];
            // j_pred^T * Cov * j_pred
            let mut param_var = 0.0;
            for a in 0..3 {
                for b in 0..3 {
                    param_var += j_pred[a] * cov[a][b] * j_pred[b];
                }
            }
            param_var + s_sq
        } else {
            // Residual-only: no parameter uncertainty, just observation noise
            s_sq
        };

        let pred_se = pred_var.max(0.0).sqrt();
        let pi_width = t_value * pred_se;

        if y_hat.is_nan() || pi_width.is_nan() || pi_width.is_infinite() {
            return Err(
                "Logistic prediction interval computation produced invalid values.".into(),
            );
        }

        forecasts.push(ForecastPoint {
            step: i as u32,
            forecast: y_hat,
            lower_bound: y_hat - pi_width,
            upper_bound: y_hat + pi_width,
        });
    }

    Ok(("logistic".into(), forecasts))
}

/// Invert a 3x3 matrix using the adjugate method.
fn invert_3x3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);

    if det.abs() < 1e-30 {
        return None;
    }

    let inv_det = 1.0 / det;

    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ])
}

// ---------------------------------------------------------------------------
// Model: ETS (Exponential Smoothing)
// ---------------------------------------------------------------------------

/// ETS model specification.
#[derive(Debug, Clone)]
struct EtsSpec {
    /// Error type: "add" or "mul".
    error_type: &'static str,
    /// Trend type: None, "add".
    trend: Option<&'static str>,
    /// Whether trend is damped.
    damped_trend: bool,
    /// Seasonal type: None, "add".
    seasonal: Option<&'static str>,
    /// Seasonal period length.
    seasonal_period: Option<usize>,
}

/// Result of ETS fitting.
///
/// All fields are populated during fitting. The smoothing parameters (alpha,
/// beta, gamma) are stored for diagnostics and potential future use in
/// forecast reporting. The `ets_forecast` function uses the state fields
/// (level, trend, seasonal) plus phi, sse, and n_params.
struct EtsFitResult {
    /// Smoothed level.
    level: f64,
    /// Smoothed trend (0.0 if no trend).
    trend: f64,
    /// Seasonal components (empty if non-seasonal).
    seasonal: Vec<f64>,
    /// Trend damping parameter.
    phi: f64,
    /// Sum of squared errors.
    sse: f64,
    /// Number of parameters (for AIC calculation).
    n_params: usize,
    /// Spec used.
    spec: EtsSpec,
}

/// Non-seasonal ETS candidate specifications.
/// Matches augurs AutoETS defaults: {add,mul} error x {None, add, add-damped} trend.
fn nonseasonal_candidates() -> Vec<EtsSpec> {
    vec![
        EtsSpec {
            error_type: "add",
            trend: None,
            damped_trend: false,
            seasonal: None,
            seasonal_period: None,
        },
        EtsSpec {
            error_type: "add",
            trend: Some("add"),
            damped_trend: false,
            seasonal: None,
            seasonal_period: None,
        },
        EtsSpec {
            error_type: "add",
            trend: Some("add"),
            damped_trend: true,
            seasonal: None,
            seasonal_period: None,
        },
        EtsSpec {
            error_type: "mul",
            trend: None,
            damped_trend: false,
            seasonal: None,
            seasonal_period: None,
        },
        EtsSpec {
            error_type: "mul",
            trend: Some("add"),
            damped_trend: false,
            seasonal: None,
            seasonal_period: None,
        },
        EtsSpec {
            error_type: "mul",
            trend: Some("add"),
            damped_trend: true,
            seasonal: None,
            seasonal_period: None,
        },
    ]
}

/// Build seasonal ETS candidate list: nonseasonal x {None, add} seasonal.
fn seasonal_candidates(period: usize) -> Vec<EtsSpec> {
    let mut candidates = Vec::new();
    for base in nonseasonal_candidates() {
        // Non-seasonal variant
        candidates.push(base.clone());
        // Additive seasonal variant
        candidates.push(EtsSpec {
            seasonal: Some("add"),
            seasonal_period: Some(period),
            ..base
        });
    }
    candidates
}

/// Fit a single ETS model to the data.
///
/// Uses grid search over smoothing parameters to minimize SSE.
/// Returns `None` if fitting fails.
fn fit_ets(values: &[f64], spec: &EtsSpec) -> Option<EtsFitResult> {
    let n = values.len();
    let has_trend = spec.trend.is_some();
    let has_seasonal = spec.seasonal.is_some();
    let period = spec.seasonal_period.unwrap_or(1);
    let is_mul_error = spec.error_type == "mul";

    if has_seasonal && n < 2 * period {
        return None;
    }

    // Multiplicative error requires all positive values
    if is_mul_error && values.iter().any(|&v| v <= 0.0) {
        return None;
    }

    // Grid search over smoothing parameters
    let alpha_grid: Vec<f64> = (1..=9).map(|i| i as f64 * 0.1).collect();
    let beta_grid: Vec<f64> = if has_trend {
        vec![0.01, 0.05, 0.1, 0.2, 0.3]
    } else {
        vec![0.0]
    };
    let gamma_grid: Vec<f64> = if has_seasonal {
        vec![0.01, 0.05, 0.1, 0.2, 0.3]
    } else {
        vec![0.0]
    };
    let phi_grid: Vec<f64> = if spec.damped_trend {
        vec![0.8, 0.85, 0.9, 0.95, 0.98]
    } else {
        vec![1.0]
    };

    let mut best_result: Option<EtsFitResult> = None;
    let mut best_sse = f64::INFINITY;

    for &alpha in &alpha_grid {
        for &beta in &beta_grid {
            for &gamma in &gamma_grid {
                for &phi in &phi_grid {
                    if let Some(result) =
                        run_ets(values, alpha, beta, gamma, phi, spec, is_mul_error)
                        && result.sse < best_sse
                        && result.sse.is_finite()
                    {
                        best_sse = result.sse;
                        best_result = Some(result);
                    }
                }
            }
        }
    }

    best_result
}

/// Run a single ETS pass with given parameters.
fn run_ets(
    values: &[f64],
    alpha: f64,
    beta: f64,
    gamma: f64,
    phi: f64,
    spec: &EtsSpec,
    is_mul_error: bool,
) -> Option<EtsFitResult> {
    let n = values.len();
    let has_trend = spec.trend.is_some();
    let has_seasonal = spec.seasonal.is_some();
    let period = spec.seasonal_period.unwrap_or(1);

    // Initialize level
    let mut level = if has_seasonal && period <= n {
        // Mean of first season
        values[..period].iter().sum::<f64>() / period as f64
    } else {
        values[0]
    };

    // Initialize trend
    let mut trend = if has_trend && n >= 2 {
        if has_seasonal && period <= n / 2 {
            // Average difference between seasons
            let season1_mean: f64 = values[..period].iter().sum::<f64>() / period as f64;
            let season2_mean: f64 = values[period..2 * period].iter().sum::<f64>() / period as f64;
            (season2_mean - season1_mean) / period as f64
        } else {
            values[1] - values[0]
        }
    } else {
        0.0
    };

    // Initialize seasonal components
    let mut seasonal = if has_seasonal {
        let mut s = vec![0.0; period];
        for i in 0..period.min(n) {
            s[i] = values[i] - level;
        }
        s
    } else {
        Vec::new()
    };

    let mut sse = 0.0;

    for (t, &y_obs) in values.iter().enumerate() {
        // One-step-ahead forecast
        let s_idx = if has_seasonal { t % period } else { 0 };
        let seasonal_component = if has_seasonal { seasonal[s_idx] } else { 0.0 };

        let forecast_val = level + phi * trend + seasonal_component;

        // Error
        let error = y_obs - forecast_val;

        if is_mul_error {
            // Multiplicative error: e_t = (y_t - forecast) / forecast
            if forecast_val.abs() < 1e-10 {
                return None; // Avoid division by zero
            }
            let rel_error = error / forecast_val;
            sse += rel_error * rel_error;
        } else {
            sse += error * error;
        }

        // Update components
        let prev_level = level;

        // Level update
        level = alpha * (y_obs - seasonal_component) + (1.0 - alpha) * (prev_level + phi * trend);

        // Trend update
        if has_trend {
            trend = beta * (level - prev_level) + (1.0 - beta) * phi * trend;
        }

        // Seasonal update
        if has_seasonal {
            seasonal[s_idx] =
                gamma * (y_obs - level) + (1.0 - gamma) * seasonal_component;
        }
    }

    // Count parameters
    let mut n_params = 1; // alpha
    if has_trend {
        n_params += 1; // beta
    }
    if spec.damped_trend {
        n_params += 1; // phi
    }
    if has_seasonal {
        n_params += 1; // gamma
        n_params += period; // seasonal components
    }
    n_params += 1; // level (initial state)
    if has_trend {
        n_params += 1; // trend (initial state)
    }

    // alpha, beta, gamma are computed but only level/trend/seasonal/phi
    // are needed for forecasting and AIC model selection.
    let _ = (alpha, beta, gamma);

    Some(EtsFitResult {
        level,
        trend,
        seasonal,
        phi,
        sse,
        n_params,
        spec: spec.clone(),
    })
}

/// Compute AIC for model selection.
fn compute_aic(sse: f64, n: usize, n_params: usize) -> f64 {
    if n <= n_params || sse <= 0.0 {
        return f64::INFINITY;
    }
    let n_f = n as f64;
    // AIC = n * ln(SSE/n) + 2k
    n_f * (sse / n_f).ln() + 2.0 * n_params as f64
}

/// Generate ETS forecasts from a fitted model.
fn ets_forecast(
    fit: &EtsFitResult,
    horizon: usize,
    confidence_level: f64,
    n: usize,
) -> Vec<ForecastPoint> {
    let has_trend = fit.spec.trend.is_some();
    let has_seasonal = fit.spec.seasonal.is_some();
    let period = fit.spec.seasonal_period.unwrap_or(1);

    // Compute residual standard deviation for prediction intervals
    let dof = if n > fit.n_params {
        n - fit.n_params
    } else {
        1
    };
    let residual_std = (fit.sse / dof as f64).sqrt();

    // Use Student's t-distribution for prediction intervals (consistent with
    // linear and exponential models). For large dof this converges to z≈1.96.
    let alpha_ci = 1.0 - confidence_level;
    let t_val = match StudentsT::new(0.0, 1.0, dof.max(1) as f64) {
        Ok(t) => t.inverse_cdf(1.0 - alpha_ci / 2.0),
        Err(_) => 1.96, // Fallback only if distribution construction fails
    };

    let level = fit.level;
    let trend = fit.trend;
    let seasonal = &fit.seasonal;

    let mut forecasts = Vec::with_capacity(horizon);
    for h in 1..=horizon {
        let seasonal_component = if has_seasonal {
            // Use the most recent seasonal values, cycling through
            seasonal[(n + h - 1) % period]
        } else {
            0.0
        };

        let damped_trend = if has_trend {
            // Sum of damped trend: phi + phi^2 + ... + phi^h
            let phi = fit.phi;
            if (phi - 1.0).abs() < 1e-10 {
                trend * h as f64
            } else {
                trend * phi * (1.0 - phi.powi(h as i32)) / (1.0 - phi)
            }
        } else {
            0.0
        };

        let forecast_val = level + damped_trend + seasonal_component;

        // Prediction interval widens with horizon
        let margin = t_val * residual_std * (h as f64).sqrt();

        forecasts.push(ForecastPoint {
            step: h as u32,
            forecast: forecast_val,
            lower_bound: forecast_val - margin,
            upper_bound: forecast_val + margin,
        });
    }

    forecasts
}

/// Non-seasonal ETS model with automatic specification selection via AIC.
fn forecast_ets_nonseasonal(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
) -> Result<(String, Vec<ForecastPoint>), String> {
    let n = values.len();
    let candidates = nonseasonal_candidates();

    let mut best_fit: Option<EtsFitResult> = None;
    let mut best_aic = f64::INFINITY;

    for spec in &candidates {
        if let Some(result) = fit_ets(values, spec) {
            let aic = compute_aic(result.sse, n, result.n_params);
            if aic < best_aic {
                best_aic = aic;
                best_fit = Some(result);
            }
        }
    }

    let fit = best_fit.ok_or(
        "ETS (non-seasonal) fitting failed: all candidate specifications failed.",
    )?;

    let forecasts = ets_forecast(&fit, horizon, confidence_level, n);
    Ok(("ets".into(), forecasts))
}

/// Seasonal ETS model with automatic specification selection via AIC.
fn forecast_ets_seasonal(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
    seasonal_period: usize,
) -> Result<(String, Vec<ForecastPoint>), String> {
    let n = values.len();

    if seasonal_period < 2 {
        return Err("Seasonal period must be at least 2.".into());
    }
    if n < 2 * seasonal_period {
        return Err(format!(
            "Need at least {} data points for seasonal period {seasonal_period}, got {n}.",
            2 * seasonal_period
        ));
    }

    let candidates = seasonal_candidates(seasonal_period);

    let mut best_fit: Option<EtsFitResult> = None;
    let mut best_aic = f64::INFINITY;

    for spec in &candidates {
        if let Some(result) = fit_ets(values, spec) {
            let aic = compute_aic(result.sse, n, result.n_params);
            if aic < best_aic {
                best_aic = aic;
                best_fit = Some(result);
            }
        }
    }

    let fit =
        best_fit.ok_or("ETS (seasonal) fitting failed: all candidate specifications failed.")?;

    let forecasts = ets_forecast(&fit, horizon, confidence_level, n);
    Ok(("ets".into(), forecasts))
}

/// ETS dispatch: uses seasonal ETS if a seasonal period is provided or detected,
/// otherwise uses non-seasonal ETS.
fn forecast_ets_dispatch(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
    seasonal_period: Option<usize>,
) -> Result<(String, Vec<ForecastPoint>), String> {
    if let Some(period) = seasonal_period
        && period >= 2
    {
        return forecast_ets_seasonal(values, horizon, confidence_level, period);
    }

    // Auto-detect seasonality
    let detected = detect_seasonality(values);
    if let Some(best) = detected.first()
        && best.strength >= SEASONALITY_STRENGTH_THRESHOLD
    {
        let period = best.period;
        match forecast_ets_seasonal(values, horizon, confidence_level, period) {
            Ok(result) => return Ok(result),
            Err(msg) => {
                info!(
                    "Seasonal ETS failed (period={period}), trying non-seasonal: {msg}"
                );
            }
        }
    }

    forecast_ets_nonseasonal(values, horizon, confidence_level)
}

// ---------------------------------------------------------------------------
// Seasonality Detection
// ---------------------------------------------------------------------------

/// Detect seasonal periods in a time series using spectral analysis + autocorrelation.
///
/// Two-stage approach:
///   1. Periodogram identifies candidate periods from the frequency domain.
///   2. Autocorrelation at each candidate validates and scores the period.
pub fn detect_seasonality(values: &[f64]) -> Vec<SeasonalityCandidate> {
    let n = values.len();
    if n < MIN_SEASONALITY_POINTS {
        return Vec::new();
    }

    // Check for constant data
    let mean = values.iter().sum::<f64>() / n as f64;
    let variance = values.iter().map(|&v| (v - mean).powi(2)).sum::<f64>() / n as f64;
    if variance < 1e-10 {
        return Vec::new();
    }

    // Stage 1: Compute periodogram via direct DFT
    // Detrend first (linear detrend)
    let detrended = linear_detrend(values);
    let power_spectrum = compute_periodogram(&detrended);

    if power_spectrum.is_empty() {
        return Vec::new();
    }

    let max_power = power_spectrum
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);

    if max_power <= 0.0 || !max_power.is_finite() {
        return Vec::new();
    }

    // Convert frequency indices to periods, filter by power threshold
    let max_period = n / 2;
    let mut period_power: std::collections::HashMap<usize, f64> = std::collections::HashMap::new();

    for (freq_idx, &pwr) in power_spectrum.iter().enumerate() {
        if pwr < max_power * PERIODOGRAM_POWER_THRESHOLD {
            continue;
        }
        // freq_idx corresponds to frequency = freq_idx / n
        // period = n / freq_idx
        if freq_idx == 0 {
            continue; // DC component
        }
        let period = ((n as f64 / freq_idx as f64).round() as usize).max(1);
        if (2..=max_period).contains(&period) {
            let existing = period_power.entry(period).or_insert(0.0);
            if pwr > *existing {
                *existing = pwr;
            }
        }
    }

    // Sort by power descending, limit candidates
    let mut sorted_candidates: Vec<(usize, f64)> = period_power.into_iter().collect();
    sorted_candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted_candidates.truncate(MAX_SEASONALITY_CANDIDATES);

    // Stage 2: Validate each candidate with autocorrelation
    let mut results = Vec::new();
    for (period, _pwr) in &sorted_candidates {
        let strength = autocorrelation_at_lag(values, *period, mean, variance);
        if strength > MIN_SEASONALITY_STRENGTH {
            results.push(SeasonalityCandidate {
                period: *period,
                strength: strength.clamp(0.0, 1.0),
            });
        }
    }

    // Sort by descending strength
    results.sort_by(|a, b| b.strength.partial_cmp(&a.strength).unwrap_or(std::cmp::Ordering::Equal));
    results
}

/// Remove linear trend from data (detrend).
fn linear_detrend(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    if n < 2 {
        return values.to_vec();
    }

    // Fit linear trend
    let n_f = n as f64;
    let sum_x: f64 = (0..n).map(|i| i as f64).sum();
    let sum_y: f64 = values.iter().sum();
    let sum_xy: f64 = values
        .iter()
        .enumerate()
        .map(|(i, &y)| i as f64 * y)
        .sum();
    let sum_x2: f64 = (0..n).map(|i| (i as f64).powi(2)).sum();
    let x_mean = sum_x / n_f;

    let ss_x = sum_x2 - sum_x * sum_x / n_f;
    if ss_x.abs() < 1e-15 {
        return values.to_vec();
    }

    let slope = (sum_xy - sum_x * sum_y / n_f) / ss_x;
    let intercept = sum_y / n_f - slope * x_mean;

    values
        .iter()
        .enumerate()
        .map(|(i, &y)| y - (intercept + slope * i as f64))
        .collect()
}

/// Compute the periodogram (power spectral density) of the signal.
///
/// Uses direct DFT computation (O(n^2)). For time series forecasting
/// with typical dataset sizes (< 10,000 points), this is fast enough.
fn compute_periodogram(values: &[f64]) -> Vec<f64> {
    let n = values.len();
    if n < 2 {
        return Vec::new();
    }

    let n_freqs = n / 2 + 1;
    let mut power = Vec::with_capacity(n_freqs);

    for k in 0..n_freqs {
        let mut re = 0.0;
        let mut im = 0.0;
        for (t, &v) in values.iter().enumerate() {
            let angle = 2.0 * PI * k as f64 * t as f64 / n as f64;
            re += v * angle.cos();
            im -= v * angle.sin();
        }
        power.push((re * re + im * im) / n as f64);
    }

    power
}

/// Compute autocorrelation at a specific lag.
///
/// Returns value between -1.0 and 1.0. Values near 1.0 indicate strong
/// periodicity at that lag.
fn autocorrelation_at_lag(values: &[f64], lag: usize, mean: f64, variance: f64) -> f64 {
    let n = values.len();
    if lag >= n || variance < 1e-10 {
        return 0.0;
    }

    let valid_pairs = n - lag;
    let covariance: f64 = (0..valid_pairs)
        .map(|i| (values[i] - mean) * (values[i + lag] - mean))
        .sum::<f64>()
        / valid_pairs as f64;

    covariance / variance
}

// ---------------------------------------------------------------------------
// Auto-selection (cross-validation)
// ---------------------------------------------------------------------------

/// Detect the best seasonal period length from the data.
fn detect_best_season_length(values: &[f64]) -> Option<usize> {
    let results = detect_seasonality(values);
    results
        .first()
        .filter(|c| c.strength > SEASONALITY_STRENGTH_THRESHOLD)
        .map(|c| c.period)
}

/// Check whether the data shows decelerating growth (S-curve behavior).
///
/// Compares average growth in the first half vs second half. Returns `true`
/// when second-half growth is meaningfully slower than first-half, suggesting
/// logistic saturation.
pub fn shows_deceleration(values: &[f64]) -> bool {
    let n = values.len();
    if n < 6 {
        return false;
    }

    let mid = n / 2;

    // Average change per step in each half
    let first_half_growth: f64 =
        values[..mid].windows(2).map(|w| w[1] - w[0]).sum::<f64>() / (mid - 1) as f64;

    if first_half_growth <= 0.0 {
        return false; // No growth in first half
    }

    let second_half_len = n - mid;
    let second_half_growth: f64 =
        values[mid..].windows(2).map(|w| w[1] - w[0]).sum::<f64>() / (second_half_len - 1) as f64;

    second_half_growth < first_half_growth * DECELERATION_RATIO
}

/// Validate that a forecast result is reasonable relative to the input data.
///
/// For upward-trending data, the first forecast point should not drop
/// significantly below the last observation. Also checks for NaN/Inf.
pub fn validate_forecast_result(
    values: &[f64],
    model_name: &str,
    forecast_points: &[ForecastPoint],
) -> bool {
    if forecast_points.is_empty() {
        return false;
    }

    let n = values.len();

    // Check for NaN/Inf in all forecast values
    for point in forecast_points {
        if !point.forecast.is_finite()
            || !point.lower_bound.is_finite()
            || !point.upper_bound.is_finite()
        {
            info!(
                "auto: {model_name} validation failed -- NaN/Inf in forecast"
            );
            return false;
        }
    }

    if n < 3 {
        return true; // Can't assess trend with too few points
    }

    // Check if data is trending upward (last 3 points)
    let trending_up = values[n - 1] > values[n - 3];

    if trending_up {
        let last_val = values[n - 1];
        let first_forecast = forecast_points[0].forecast;
        if first_forecast < last_val * FORECAST_DROP_THRESHOLD {
            info!(
                "auto: {model_name} validation failed -- forecast dropped >10% \
                 (last={last_val:.2}, forecast={first_forecast:.2})"
            );
            return false;
        }
    }

    true
}

/// Compute MSE of a model on held-out data.
fn evaluate_model_mse(
    values: &[f64],
    holdout: usize,
    model_name: &str,
    season_length: Option<usize>,
) -> Option<f64> {
    let n = values.len();
    if holdout >= n || n - holdout < MIN_DATA_POINTS {
        return None;
    }

    let train = &values[..n - holdout];
    let actual = &values[n - holdout..];

    // Fixed confidence for CV (doesn't affect point forecasts)
    let confidence = 0.95;

    let result = match model_name {
        "seasonal_ets" => {
            let period = season_length.filter(|&p| p > 1)?;
            forecast_ets_seasonal(train, holdout, confidence, period).ok()?
        }
        "ets" => forecast_ets_nonseasonal(train, holdout, confidence).ok()?,
        "exponential" => forecast_exponential(train, holdout, confidence).ok()?,
        "logistic" => forecast_logistic(train, holdout, confidence).ok()?,
        "linear" => forecast_linear(train, holdout, confidence).ok()?,
        _ => return None,
    };

    let (_, forecast_points) = result;
    if forecast_points.len() != holdout {
        return None;
    }

    // Compute MSE
    let mse: f64 = forecast_points
        .iter()
        .zip(actual.iter())
        .map(|(pred, &act)| (pred.forecast - act).powi(2))
        .sum::<f64>()
        / holdout as f64;

    if mse.is_finite() { Some(mse) } else { None }
}

/// Automatic model selection via cross-validation.
fn forecast_auto(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
    seasonal_period: Option<usize>,
) -> Result<(String, Vec<ForecastPoint>), String> {
    let n = values.len();

    // Step 1: Detect seasonality
    let season_length = match seasonal_period {
        Some(p) if p >= 2 => Some(p),
        Some(_) => None,
        None => detect_best_season_length(values),
    };

    // Step 2: Small data -- not enough for cross-validation
    if n < 2 * MIN_DATA_POINTS {
        info!("auto: insufficient data for CV (n={n}), using small-data selection");
        return auto_small_data(values, horizon, confidence_level, season_length);
    }

    // Step 3: Holdout split
    let holdout = ((n as f64 * 0.2).ceil() as usize)
        .max(MIN_DATA_POINTS)
        .min(n - MIN_DATA_POINTS);

    // Step 4: Build candidate list
    let all_positive = values.iter().all(|&v| v > 0.0);
    let deceleration = shows_deceleration(values);

    let mut candidates: Vec<&str> = Vec::new();
    if season_length.is_some_and(|p| p > 1) {
        candidates.push("seasonal_ets");
    }
    candidates.push("ets");
    if all_positive {
        candidates.push("exponential");
    }
    if deceleration {
        candidates.push("logistic");
    }
    candidates.push("linear");

    // Step 5: Evaluate each candidate, rank by MSE
    let mut ranked: Vec<(&str, f64)> = Vec::new();
    for &model_name in &candidates {
        match evaluate_model_mse(values, holdout, model_name, season_length) {
            Some(mse) => {
                info!("auto CV: model={model_name} MSE={mse:.4}");
                ranked.push((model_name, mse));
            }
            None => {
                info!("auto CV: model={model_name} failed/skipped");
            }
        }
    }

    ranked.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    if ranked.is_empty() {
        info!("auto: all candidates failed CV, last resort = linear");
        return forecast_linear(values, horizon, confidence_level);
    }

    // Step 6: Try models in MSE order, validate each refit on full data
    for (model_name, mse) in &ranked {
        info!("auto: trying model={model_name} (CV MSE={mse:.4}) on full data");

        let result = fit_named_model(values, horizon, confidence_level, model_name, season_length);

        match result {
            Err(msg) => {
                info!("auto: model={model_name} refit failed: {msg}");
                continue;
            }
            Ok((fitted_name, forecast_points)) => {
                if validate_forecast_result(values, model_name, &forecast_points) {
                    info!("auto: selected model={model_name}");
                    return Ok((fitted_name, forecast_points));
                }
                info!("auto: model={model_name} refit failed validation, trying next");
            }
        }
    }

    // Step 7: Last resort -- linear always works
    info!("auto: all models failed validation, last resort = linear");
    forecast_linear(values, horizon, confidence_level)
}

/// Small-data auto-selection (n < 2 * MIN_DATA_POINTS).
fn auto_small_data(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
    season_length: Option<usize>,
) -> Result<(String, Vec<ForecastPoint>), String> {
    // Try seasonal ETS first if seasonality was detected
    if let Some(period) = season_length
        && period > 1
        && let Ok(result) = forecast_ets_seasonal(values, horizon, confidence_level, period)
    {
        return Ok(result);
    }

    // Try non-seasonal ETS
    if let Ok(result) = forecast_ets_nonseasonal(values, horizon, confidence_level) {
        return Ok(result);
    }

    // Linear as last explicit choice
    forecast_linear(values, horizon, confidence_level)
}

/// Dispatch to a named model for the auto-selection refit step.
fn fit_named_model(
    values: &[f64],
    horizon: usize,
    confidence_level: f64,
    model_name: &str,
    season_length: Option<usize>,
) -> Result<(String, Vec<ForecastPoint>), String> {
    match model_name {
        "seasonal_ets" => {
            let period = season_length
                .filter(|&p| p >= 2)
                .ok_or("No valid seasonal period for seasonal ETS.")?;
            forecast_ets_seasonal(values, horizon, confidence_level, period)
        }
        "ets" => forecast_ets_nonseasonal(values, horizon, confidence_level),
        "exponential" => forecast_exponential(values, horizon, confidence_level),
        "logistic" => forecast_logistic(values, horizon, confidence_level),
        "linear" => forecast_linear(values, horizon, confidence_level),
        _ => Err(format!("Unknown model name: {model_name}")),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: check that all forecast points have finite values and correct ordering
    fn assert_valid_forecasts(points: &[ForecastPoint], expected_steps: usize) {
        assert_eq!(points.len(), expected_steps);
        for (i, p) in points.iter().enumerate() {
            assert_eq!(p.step, (i + 1) as u32);
            assert!(p.forecast.is_finite(), "forecast is not finite at step {}", p.step);
            assert!(p.lower_bound.is_finite(), "lower_bound is not finite at step {}", p.step);
            assert!(p.upper_bound.is_finite(), "upper_bound is not finite at step {}", p.step);
        }
    }

    // -----------------------------------------------------------------------
    // Linear model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forecast_linear_basic() {
        // Ascending data: 1, 2, 3, 4, 5
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert_eq!(result.model_used, "linear");
        assert_eq!(result.data_points, 5);
        assert_valid_forecasts(&result.forecast, 3);

        // Forecasts should continue upward (approximately 6, 7, 8)
        assert!(result.forecast[0].forecast > 5.0);
        assert!(result.forecast[1].forecast > result.forecast[0].forecast);
        assert!(result.forecast[2].forecast > result.forecast[1].forecast);
    }

    #[test]
    fn test_forecast_linear_constant() {
        // Flat data: 5, 5, 5, 5, 5
        let values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert_eq!(result.model_used, "linear");
        assert_valid_forecasts(&result.forecast, 3);

        // Forecasts should be approximately 5.0
        for point in &result.forecast {
            assert!(
                (point.forecast - 5.0).abs() < 0.01,
                "Expected ~5.0, got {}",
                point.forecast
            );
        }
    }

    // -----------------------------------------------------------------------
    // Exponential model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forecast_exponential_basic() {
        // Geometric growth: 1, 2, 4, 8, 16
        let values = vec![1.0, 2.0, 4.0, 8.0, 16.0];
        let result = forecast(&values, 3, 0.95, "exponential", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert_eq!(result.model_used, "exponential");
        assert_valid_forecasts(&result.forecast, 3);

        // Forecasts should continue growing
        assert!(result.forecast[0].forecast > 16.0);
    }

    #[test]
    fn test_forecast_exponential_negative_values_fails() {
        let values = vec![1.0, -2.0, 3.0, 4.0, 5.0];
        let result = forecast(&values, 3, 0.95, "exponential", None);
        assert!(result.error.is_some());
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("positive"));
    }

    // -----------------------------------------------------------------------
    // Logistic model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forecast_logistic_basic() {
        // S-curve data (approaching asymptote around 100)
        let values: Vec<f64> = (0..20)
            .map(|i| 100.0 / (1.0 + (-0.5 * (i as f64 - 10.0)).exp()))
            .collect();
        let result = forecast(&values, 3, 0.95, "logistic", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert_eq!(result.model_used, "logistic");
        assert_valid_forecasts(&result.forecast, 3);

        // Forecasts should be near 100 (the asymptote)
        for point in &result.forecast {
            assert!(
                point.forecast > 80.0 && point.forecast <= 110.0,
                "Expected forecast near 100, got {}",
                point.forecast
            );
        }
    }

    // -----------------------------------------------------------------------
    // ETS model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forecast_ets_basic() {
        // Simple time series with some noise
        let values = vec![10.0, 12.0, 14.0, 13.0, 15.0, 17.0, 16.0, 18.0];
        let result = forecast(&values, 3, 0.95, "ets", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert_eq!(result.model_used, "ets");
        assert_valid_forecasts(&result.forecast, 3);
    }

    // -----------------------------------------------------------------------
    // Auto model tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forecast_auto_selects_model() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let result = forecast(&values, 3, 0.95, "auto", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert!(!result.model_used.is_empty());
        // auto should pick a valid model name
        assert!(
            ["linear", "exponential", "logistic", "ets"]
                .contains(&result.model_used.as_str()),
            "Unexpected model: {}",
            result.model_used
        );
        assert_valid_forecasts(&result.forecast, 3);
    }

    // -----------------------------------------------------------------------
    // Input validation tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forecast_min_data_points() {
        let values = vec![1.0, 2.0, 3.0]; // Only 3 points, need 4
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_some());
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("at least 4"));
    }

    #[test]
    fn test_forecast_invalid_model() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = forecast(&values, 3, 0.95, "invalid_model", None);
        assert!(result.error.is_some());
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("Unknown model"));
    }

    #[test]
    fn test_forecast_empty_input() {
        let values: Vec<f64> = vec![];
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_some());
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("empty"));
    }

    #[test]
    fn test_forecast_nan_input() {
        let values = vec![1.0, f64::NAN, 3.0, 4.0, 5.0];
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_some());
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("NaN"));
    }

    #[test]
    fn test_forecast_invalid_confidence() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = forecast(&values, 3, 1.5, "linear", None);
        assert!(result.error.is_some());
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("Confidence level"));
    }

    #[test]
    fn test_forecast_zero_horizon() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = forecast(&values, 0, 0.95, "linear", None);
        assert!(result.error.is_some());
        assert!(result
            .error
            .as_ref()
            .unwrap()
            .contains("Horizon"));
    }

    // -----------------------------------------------------------------------
    // Seasonality detection tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_detect_seasonality_periodic() {
        // Create a signal with period 7 (weekly pattern)
        let values: Vec<f64> = (0..56)
            .map(|i| 10.0 + 5.0 * (2.0 * PI * i as f64 / 7.0).sin())
            .collect();

        let candidates = detect_seasonality(&values);
        assert!(
            !candidates.is_empty(),
            "Should detect seasonality in periodic data"
        );

        // Should find period 7 as one of the top candidates
        let has_period_7 = candidates.iter().any(|c| c.period == 7);
        assert!(
            has_period_7,
            "Should detect period 7. Found: {:?}",
            candidates.iter().map(|c| c.period).collect::<Vec<_>>()
        );
    }

    #[test]
    fn test_detect_seasonality_no_pattern() {
        // Linear trend with no seasonal pattern
        let values: Vec<f64> = (0..50).map(|i| i as f64 * 2.0).collect();
        let candidates = detect_seasonality(&values);
        // Should be empty or have very low strength candidates
        let strong = candidates
            .iter()
            .filter(|c| c.strength > SEASONALITY_STRENGTH_THRESHOLD)
            .count();
        assert_eq!(
            strong, 0,
            "Should not detect strong seasonality in linear data"
        );
    }

    // -----------------------------------------------------------------------
    // Validation helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_shows_deceleration_true() {
        // Fast growth then slow growth
        let values = vec![1.0, 5.0, 10.0, 16.0, 23.0, 31.0, 33.0, 34.0, 35.0, 35.5, 35.8, 36.0];
        assert!(shows_deceleration(&values));
    }

    #[test]
    fn test_shows_deceleration_false() {
        // Constant growth rate
        let values: Vec<f64> = (0..12).map(|i| i as f64 * 3.0).collect();
        assert!(!shows_deceleration(&values));
    }

    #[test]
    fn test_validate_forecast_no_nan() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let bad_points = vec![ForecastPoint {
            step: 1,
            forecast: f64::NAN,
            lower_bound: 5.0,
            upper_bound: 7.0,
        }];
        assert!(!validate_forecast_result(&values, "test", &bad_points));
    }

    #[test]
    fn test_confidence_intervals_ordered() {
        // For all models, lower_bound < forecast < upper_bound
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        for model in &["linear", "exponential", "ets"] {
            let result = forecast(&values, 3, 0.95, model, None);
            if result.error.is_none() {
                for point in &result.forecast {
                    assert!(
                        point.lower_bound <= point.forecast,
                        "{model}: lower_bound ({}) > forecast ({})",
                        point.lower_bound,
                        point.forecast
                    );
                    assert!(
                        point.forecast <= point.upper_bound,
                        "{model}: forecast ({}) > upper_bound ({})",
                        point.forecast,
                        point.upper_bound
                    );
                }
            }
        }
    }

    // -----------------------------------------------------------------------
    // Edge case tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_forecast_linear_perfect_fit() {
        // Perfect linear data: y = 2x + 1
        let values: Vec<f64> = (0..10).map(|i| 2.0 * i as f64 + 1.0).collect();
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_none());

        // Perfect fit means very small prediction intervals
        // Forecast should be approximately 21, 23, 25
        assert!((result.forecast[0].forecast - 21.0).abs() < 0.01);
        assert!((result.forecast[1].forecast - 23.0).abs() < 0.01);
        assert!((result.forecast[2].forecast - 25.0).abs() < 0.01);
    }

    #[test]
    fn test_forecast_ets_seasonal() {
        // Data with seasonal period 4
        let base: Vec<f64> = vec![10.0, 15.0, 20.0, 12.0]; // One season
        let values: Vec<f64> = base
            .iter()
            .cycle()
            .take(16) // 4 seasons
            .cloned()
            .collect();

        let result = forecast(&values, 4, 0.95, "ets", Some(4));
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert_eq!(result.model_used, "ets");
        assert_valid_forecasts(&result.forecast, 4);
    }

    #[test]
    fn test_ols_regression_basic() {
        // y = 3x + 2
        let values = vec![2.0, 5.0, 8.0, 11.0, 14.0];
        let reg = ols_regression(&values).expect("regression should succeed");
        assert!((reg.slope - 3.0).abs() < 0.01);
        assert!((reg.intercept - 2.0).abs() < 0.01);
    }

    #[test]
    fn test_auto_with_small_data() {
        // Just 5 data points — should use small-data path
        let values = vec![1.0, 3.0, 5.0, 7.0, 9.0];
        let result = forecast(&values, 2, 0.95, "auto", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        assert!(!result.model_used.is_empty());
        assert_valid_forecasts(&result.forecast, 2);
    }

    #[test]
    fn test_logistic_constant_data_fails() {
        let values = vec![5.0, 5.0, 5.0, 5.0, 5.0];
        let result = forecast(&values, 3, 0.95, "logistic", None);
        assert!(result.error.is_some());
    }

    #[test]
    fn test_clean_values_inf() {
        let values = vec![1.0, f64::INFINITY, 3.0];
        let result = clean_values(&values);
        assert!(result.is_err());
    }
}

// ---------------------------------------------------------------------------
// Contract tests — behavioral contracts across forecast models
// ---------------------------------------------------------------------------

#[cfg(test)]
mod contract_tests {
    use super::*;

    // -- Prediction interval ordering: lower < forecast < upper ---------------

    #[test]
    fn prediction_intervals_ordered_for_linear() {
        let values = vec![1.0, 3.0, 5.0, 7.0, 9.0, 11.0, 13.0, 15.0];
        let result = forecast(&values, 5, 0.95, "linear", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        for pt in &result.forecast {
            assert!(
                pt.lower_bound <= pt.forecast,
                "linear: lower ({}) > forecast ({})",
                pt.lower_bound,
                pt.forecast
            );
            assert!(
                pt.forecast <= pt.upper_bound,
                "linear: forecast ({}) > upper ({})",
                pt.forecast,
                pt.upper_bound
            );
        }
    }

    #[test]
    fn prediction_intervals_ordered_for_exponential() {
        let values = vec![1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
        let result = forecast(&values, 5, 0.95, "exponential", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        for pt in &result.forecast {
            assert!(
                pt.lower_bound <= pt.forecast,
                "exponential: lower ({}) > forecast ({})",
                pt.lower_bound,
                pt.forecast
            );
            assert!(
                pt.forecast <= pt.upper_bound,
                "exponential: forecast ({}) > upper ({})",
                pt.forecast,
                pt.upper_bound
            );
        }
    }

    #[test]
    fn prediction_intervals_ordered_for_logistic() {
        let values: Vec<f64> = (0..20)
            .map(|i| 100.0 / (1.0 + (-0.5 * (i as f64 - 10.0)).exp()))
            .collect();
        let result = forecast(&values, 5, 0.95, "logistic", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        for pt in &result.forecast {
            assert!(
                pt.lower_bound <= pt.forecast,
                "logistic: lower ({}) > forecast ({})",
                pt.lower_bound,
                pt.forecast
            );
            assert!(
                pt.forecast <= pt.upper_bound,
                "logistic: forecast ({}) > upper ({})",
                pt.forecast,
                pt.upper_bound
            );
        }
    }

    #[test]
    fn prediction_intervals_ordered_for_ets() {
        let values = vec![10.0, 12.0, 14.0, 13.0, 15.0, 17.0, 16.0, 18.0];
        let result = forecast(&values, 5, 0.95, "ets", None);
        assert!(result.error.is_none(), "Error: {:?}", result.error);
        for pt in &result.forecast {
            assert!(
                pt.lower_bound <= pt.forecast,
                "ets: lower ({}) > forecast ({})",
                pt.lower_bound,
                pt.forecast
            );
            assert!(
                pt.forecast <= pt.upper_bound,
                "ets: forecast ({}) > upper ({})",
                pt.forecast,
                pt.upper_bound
            );
        }
    }

    // -- Confidence level scaling: wider intervals at higher confidence --------

    #[test]
    fn higher_confidence_produces_wider_intervals() {
        // Data must have noise/residuals so prediction intervals are non-zero.
        // Perfect linear data produces zero residuals and zero-width intervals.
        let values = vec![1.0, 2.3, 2.8, 4.1, 5.2, 5.9, 7.1, 8.3];
        let narrow = forecast(&values, 3, 0.50, "linear", None);
        let wide = forecast(&values, 3, 0.99, "linear", None);

        assert!(narrow.error.is_none());
        assert!(wide.error.is_none());

        for (n, w) in narrow.forecast.iter().zip(wide.forecast.iter()) {
            let narrow_width = n.upper_bound - n.lower_bound;
            let wide_width = w.upper_bound - w.lower_bound;
            assert!(
                wide_width > narrow_width,
                "99% interval ({wide_width:.4}) should be wider than 50% ({narrow_width:.4})"
            );
        }
    }

    // -- Horizon contract: number of forecast points matches requested horizon

    #[test]
    fn horizon_contract_for_all_models() {
        let values = vec![1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
        for horizon in [1, 3, 5, 10] {
            for model in &["linear", "exponential", "ets"] {
                let result = forecast(&values, horizon, 0.95, model, None);
                if result.error.is_none() {
                    assert_eq!(
                        result.forecast.len(),
                        horizon,
                        "model={model} horizon={horizon}: got {} points",
                        result.forecast.len()
                    );
                }
            }
        }
    }

    // -- Linear regression known values: exact slope/intercept ----------------

    #[test]
    fn linear_regression_known_slope_and_intercept() {
        // y = 3x + 2: values at x=0,1,2,3,4 => 2, 5, 8, 11, 14
        let values = vec![2.0, 5.0, 8.0, 11.0, 14.0];
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_none());

        // Next x values: 5, 6, 7 => expected y: 17, 20, 23
        assert!(
            (result.forecast[0].forecast - 17.0).abs() < 0.01,
            "Expected 17.0, got {}",
            result.forecast[0].forecast
        );
        assert!(
            (result.forecast[1].forecast - 20.0).abs() < 0.01,
            "Expected 20.0, got {}",
            result.forecast[1].forecast
        );
        assert!(
            (result.forecast[2].forecast - 23.0).abs() < 0.01,
            "Expected 23.0, got {}",
            result.forecast[2].forecast
        );
    }

    #[test]
    fn linear_regression_negative_slope() {
        // y = -2x + 10: values at x=0,1,2,3,4 => 10, 8, 6, 4, 2
        let values = vec![10.0, 8.0, 6.0, 4.0, 2.0];
        let result = forecast(&values, 2, 0.95, "linear", None);
        assert!(result.error.is_none());

        // Next x values: 5, 6 => expected y: 0, -2
        assert!(
            (result.forecast[0].forecast - 0.0).abs() < 0.01,
            "Expected ~0.0, got {}",
            result.forecast[0].forecast
        );
        assert!(
            (result.forecast[1].forecast - (-2.0)).abs() < 0.01,
            "Expected ~-2.0, got {}",
            result.forecast[1].forecast
        );
    }

    // -- Auto-selection stability: same input produces same model --------

    #[test]
    fn auto_selection_is_deterministic() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        let result1 = forecast(&values, 3, 0.95, "auto", None);
        let result2 = forecast(&values, 3, 0.95, "auto", None);

        assert!(result1.error.is_none());
        assert!(result2.error.is_none());
        assert_eq!(
            result1.model_used, result2.model_used,
            "Auto should select the same model for identical input"
        );

        // Point forecasts should be identical too
        for (p1, p2) in result1.forecast.iter().zip(result2.forecast.iter()) {
            assert!(
                (p1.forecast - p2.forecast).abs() < 1e-10,
                "Forecasts should be identical for same input"
            );
        }
    }

    // -- Large dataset handling: no panic on 1000+ data points ----------------

    #[test]
    fn large_dataset_linear_does_not_panic() {
        let values: Vec<f64> = (0..1000).map(|i| i as f64 * 1.5 + 10.0).collect();
        let result = forecast(&values, 5, 0.95, "linear", None);
        assert!(result.error.is_none());
        assert_eq!(result.forecast.len(), 5);
        assert_eq!(result.data_points, 1000);
    }

    #[test]
    fn large_dataset_ets_does_not_panic() {
        let values: Vec<f64> = (0..500)
            .map(|i| 100.0 + 5.0 * (i as f64 * 0.1).sin())
            .collect();
        let result = forecast(&values, 5, 0.95, "ets", None);
        assert!(result.error.is_none());
        assert_eq!(result.forecast.len(), 5);
    }

    #[test]
    fn large_dataset_auto_does_not_panic() {
        let values: Vec<f64> = (0..1000).map(|i| i as f64 * 2.0 + 50.0).collect();
        let result = forecast(&values, 5, 0.95, "auto", None);
        assert!(result.error.is_none());
        assert_eq!(result.forecast.len(), 5);
    }

    // -- Edge case data -------------------------------------------------------

    #[test]
    fn all_identical_values_linear() {
        let values = vec![42.0; 10];
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_none());
        // Forecast should be approximately 42
        for pt in &result.forecast {
            assert!(
                (pt.forecast - 42.0).abs() < 1.0,
                "Expected ~42.0, got {}",
                pt.forecast
            );
        }
    }

    #[test]
    fn alternating_up_down_pattern() {
        // Zigzag: 10, 20, 10, 20, 10, 20, 10, 20
        let values = vec![10.0, 20.0, 10.0, 20.0, 10.0, 20.0, 10.0, 20.0];
        let result = forecast(&values, 4, 0.95, "ets", None);
        assert!(result.error.is_none());
        assert_eq!(result.forecast.len(), 4);
        // All forecast values should be finite
        for pt in &result.forecast {
            assert!(pt.forecast.is_finite());
        }
    }

    #[test]
    fn single_large_outlier_does_not_crash() {
        let mut values = vec![10.0; 20];
        values[10] = 10000.0; // Massive outlier
        let result = forecast(&values, 3, 0.95, "linear", None);
        assert!(result.error.is_none());
        assert_eq!(result.forecast.len(), 3);
    }

    // -- Step numbering -------------------------------------------------------

    #[test]
    fn forecast_steps_are_one_based_sequential() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let result = forecast(&values, 5, 0.95, "linear", None);
        assert!(result.error.is_none());
        for (i, pt) in result.forecast.iter().enumerate() {
            assert_eq!(
                pt.step,
                (i + 1) as u32,
                "Step should be 1-based sequential"
            );
        }
    }

    // -- Data points count is reported correctly ------------------------------

    #[test]
    fn data_points_count_is_accurate() {
        for n in [4, 10, 50, 100] {
            let values: Vec<f64> = (0..n).map(|i| i as f64).collect();
            let result = forecast(&values, 3, 0.95, "linear", None);
            assert!(result.error.is_none());
            assert_eq!(result.data_points, n);
        }
    }

    // -- Seasonality detection ------------------------------------------------

    #[test]
    fn seasonality_not_detected_in_constant_data() {
        let values = vec![5.0; 100];
        let candidates = detect_seasonality(&values);
        assert!(candidates.is_empty());
    }

    #[test]
    fn seasonality_not_detected_in_short_data() {
        let values = vec![1.0, 2.0, 3.0]; // Below MIN_SEASONALITY_POINTS
        let candidates = detect_seasonality(&values);
        assert!(candidates.is_empty());
    }

    // -- Valid models list -----------------------------------------------------

    #[test]
    fn all_valid_model_names_are_accepted() {
        let values = vec![1.0, 2.0, 4.0, 8.0, 16.0, 32.0, 64.0, 128.0];
        for model in VALID_MODELS {
            let result = forecast(&values, 2, 0.95, model, None);
            assert!(
                result.error.is_none(),
                "Model '{model}' should succeed for positive exponential data: {:?}",
                result.error
            );
        }
    }

    // -- Shows deceleration ---------------------------------------------------

    #[test]
    fn shows_deceleration_too_few_points() {
        let values = vec![1.0, 2.0, 3.0, 4.0]; // < 6 points
        assert!(!shows_deceleration(&values));
    }

    #[test]
    fn shows_deceleration_linear_growth_is_false() {
        // Perfect linear growth (y = 5x) has constant growth rate,
        // so second half growth should equal first half growth (no deceleration).
        let values: Vec<f64> = (0..20).map(|i| i as f64 * 5.0).collect();
        assert!(
            !shows_deceleration(&values),
            "Perfect linear data should show no deceleration"
        );
    }
}
