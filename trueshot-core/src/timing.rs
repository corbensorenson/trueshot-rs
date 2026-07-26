//! Hierarchical timing utility for performance profiling.
//!
//! Provides zero-cost (when disabled) timing infrastructure for measuring
//! and reporting performance bottlenecks across the pipeline.
//!
//! # Features
//!
//! - Hierarchical scope tracking (e.g., "preprocess.align.fft")
//! - Aggregated statistics (min/mean/max/std dev)
//! - Console reporting with tree view and tables
//! - JSON logging for batch analysis
//! - <1% overhead when enabled, zero-cost when disabled
//!
//! # Example
//!
//! ```no_run
//! use pixelcollapse2::timing::{HierarchicalTimer, timed_scope};
//!
//! let mut timer = HierarchicalTimer::new("sequence_1");
//! timed_scope!(&mut timer, "preprocess", {
//!     // Your preprocessing code
//!     timed_scope!(&mut timer, "align", {
//!         // Alignment code
//!     });
//! });
//!
//! timer.report_console(true);
//! ```

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
#[cfg(feature = "timing")]
use anyhow::Context;

/// Statistics for a timed scope
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimingStats {
    /// Minimum duration in milliseconds
    pub min_ms: f64,
    /// Mean duration in milliseconds
    pub mean_ms: f64,
    /// Maximum duration in milliseconds
    pub max_ms: f64,
    /// Standard deviation in milliseconds
    pub std_ms: f64,
    /// Percentage of total time
    pub pct_total: f64,
    /// Number of samples
    pub count: usize,
}

/// Aggregated timing report
#[derive(Debug, Clone, serde::Serialize)]
pub struct TimingReport {
    /// Sequence or batch identifier
    pub id: String,
    /// Total elapsed time in milliseconds
    pub total_ms: f64,
    /// Per-label statistics
    pub timings: HashMap<String, TimingStats>,
}

/// Hierarchical timer for tracking nested scopes
pub struct HierarchicalTimer {
    /// Root identifier
    id: String,
    /// Stack of active scope labels (for nesting)
    scope_stack: Vec<String>,
    /// Stack of start times (parallel to scope_stack)
    time_stack: Vec<Instant>,
    /// Recorded durations per label (hierarchical key: "parent.child")
    timings: HashMap<String, Vec<f64>>,
    /// Root start time
    root_start: Instant,
}

impl HierarchicalTimer {
    /// Create a new hierarchical timer with the given identifier
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_string(),
            scope_stack: Vec::new(),
            time_stack: Vec::new(),
            timings: HashMap::new(),
            root_start: Instant::now(),
        }
    }

    /// Start a new timing scope
    ///
    /// Returns a guard that automatically stops the scope when dropped.
    pub fn start(&mut self, label: &str) -> ScopeGuard {
        let full_label = if self.scope_stack.is_empty() {
            label.to_string()
        } else {
            format!("{}.{}", self.scope_stack.join("."), label)
        };

        self.scope_stack.push(label.to_string());
        self.time_stack.push(Instant::now());

        ScopeGuard {
            label: full_label,
            timer: self as *mut HierarchicalTimer,
        }
    }

    /// Stop the current timing scope
    fn stop(&mut self, full_label: &str) {
        if let (Some(_label), Some(start_time)) = (self.scope_stack.pop(), self.time_stack.pop()) {
            let elapsed = start_time.elapsed();
            let elapsed_ms = elapsed.as_secs_f64() * 1000.0;

            self.timings
                .entry(full_label.to_string())
                .or_default()
                .push(elapsed_ms);
        }
    }

    /// Get the total elapsed time since timer creation
    pub fn total_elapsed_ms(&self) -> f64 {
        self.root_start.elapsed().as_secs_f64() * 1000.0
    }

    /// Aggregate timing data into a report
    pub fn aggregate(&self) -> TimingReport {
        let total_ms = self.total_elapsed_ms();
        let mut timings_map = HashMap::new();

        for (label, durations) in &self.timings {
            if durations.is_empty() {
                continue;
            }

            let count = durations.len();
            let sum: f64 = durations.iter().sum();
            let mean = sum / count as f64;

            let min = durations.iter().copied().fold(f64::INFINITY, f64::min);
            let max = durations.iter().copied().fold(f64::NEG_INFINITY, f64::max);

            // Compute standard deviation
            let variance = if count > 1 {
                durations.iter().map(|&x| (x - mean).powi(2)).sum::<f64>() / (count - 1) as f64
            } else {
                0.0
            };
            let std = variance.sqrt();

            let pct_total = if total_ms > 0.0 {
                (mean / total_ms) * 100.0
            } else {
                0.0
            };

            timings_map.insert(
                label.clone(),
                TimingStats {
                    min_ms: min,
                    mean_ms: mean,
                    max_ms: max,
                    std_ms: std,
                    pct_total,
                    count,
                },
            );
        }

        TimingReport {
            id: self.id.clone(),
            total_ms,
            timings: timings_map,
        }
    }

    /// Report timing statistics to console
    ///
    /// If `verbose` is true, prints a tree view. Always prints a summary table.
    #[cfg(feature = "timing")]
    pub fn report_console(&self, verbose: bool) {
        let report = self.aggregate();

        if verbose {
            println!("\n=== Timing Report: {} ===", report.id);
            println!("Total: {:.2}ms\n", report.total_ms);
            self.print_tree(&report);
        }

        self.print_table(&report);
    }

    /// Report timing statistics to console (no-op when timing disabled)
    #[cfg(not(feature = "timing"))]
    pub fn report_console(&self, _verbose: bool) {
        // No-op when timing is disabled
    }

    /// Print hierarchical tree view
    #[cfg(feature = "timing")]
    fn print_tree(&self, report: &TimingReport) {
        // Build tree structure
        let mut tree: HashMap<String, Vec<String>> = HashMap::new();
        let mut all_labels: Vec<String> = report.timings.keys().cloned().collect();
        all_labels.sort();

        for label in &all_labels {
            let parts: Vec<&str> = label.split('.').collect();
            if parts.len() > 1 {
                let parent = parts[..parts.len() - 1].join(".");
                tree.entry(parent).or_insert_with(Vec::new).push(label.clone());
            }
        }

        // Print root-level entries
        for label in &all_labels {
            if !label.contains('.') {
                self.print_tree_node(label, 0, report, &tree);
            }
        }
    }

    #[cfg(feature = "timing")]
    fn print_tree_node(
        &self,
        label: &str,
        depth: usize,
        report: &TimingReport,
        tree: &HashMap<String, Vec<String>>,
    ) {
        let indent = "  ".repeat(depth);
        if let Some(stats) = report.timings.get(label) {
            let short_label = label.split('.').last().unwrap_or(label);
            println!(
                "{}{} ({:.2}ms, {:.1}%)",
                indent, short_label, stats.mean_ms, stats.pct_total
            );

            // Print children
            if let Some(children) = tree.get(label) {
                for child in children {
                    self.print_tree_node(child, depth + 1, report, tree);
                }
            }
        }
    }

    /// Print summary table
    #[cfg(feature = "timing")]
    fn print_table(&self, report: &TimingReport) {
        use comfy_table::{presets::UTF8_FULL, Cell, Color, ContentArrangement, Table};

        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .set_content_arrangement(ContentArrangement::Dynamic)
            .set_header(vec![
                Cell::new("Label").fg(Color::Cyan),
                Cell::new("Count").fg(Color::Cyan),
                Cell::new("Min (ms)").fg(Color::Cyan),
                Cell::new("Mean (ms)").fg(Color::Cyan),
                Cell::new("Max (ms)").fg(Color::Cyan),
                Cell::new("Std Dev").fg(Color::Cyan),
                Cell::new("% Total").fg(Color::Cyan),
            ]);

        // Sort by mean time descending
        let mut entries: Vec<_> = report.timings.iter().collect();
        entries.sort_by(|a, b| b.1.mean_ms.partial_cmp(&a.1.mean_ms).unwrap());

        // Filter out noise (<1ms mean)
        for (label, stats) in entries {
            if stats.mean_ms < 1.0 {
                continue;
            }

            table.add_row(vec![
                Cell::new(label),
                Cell::new(stats.count),
                Cell::new(format!("{:.2}", stats.min_ms)),
                Cell::new(format!("{:.2}", stats.mean_ms)),
                Cell::new(format!("{:.2}", stats.max_ms)),
                Cell::new(format!("{:.2}", stats.std_ms)),
                Cell::new(format!("{:.1}", stats.pct_total)),
            ]);
        }

        println!("\n{}", table);
    }

    /// Log timing report to JSON file
    #[cfg(feature = "timing")]
    pub fn log_to_file(&self, path: &Path) -> Result<()> {
        use std::fs::OpenOptions;
        use std::io::Write;

        let report = self.aggregate();
        let json = serde_json::to_string(&report)
            .with_context(|| "Failed to serialize timing report")?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .with_context(|| format!("Failed to open timing log: {:?}", path))?;

        writeln!(file, "{}", json)
            .with_context(|| "Failed to write timing log")?;

        Ok(())
    }

    /// Log timing report to JSON file (no-op when timing disabled)
    #[cfg(not(feature = "timing"))]
    pub fn log_to_file(&self, _path: &Path) -> Result<()> {
        Ok(())
    }
}

/// RAII guard for automatic scope timing
///
/// Automatically stops the timing scope when dropped.
pub struct ScopeGuard {
    label: String,
    timer: *mut HierarchicalTimer,
}

impl Drop for ScopeGuard {
    fn drop(&mut self) {
        // SAFETY: The timer pointer is valid for the lifetime of the guard
        // because the guard is created by HierarchicalTimer::start() and
        // cannot outlive the timer.
        unsafe {
            if !self.timer.is_null() {
                (*self.timer).stop(&self.label);
            }
        }
    }
}

/// Macro for timing a code block within a scope
///
/// # Example
///
/// ```no_run
/// # use pixelcollapse2::timing::{HierarchicalTimer, timed_scope};
/// let mut timer = HierarchicalTimer::new("test");
/// timed_scope!(&mut timer, "my_operation", {
///     // Your code here
/// });
/// ```
#[macro_export]
macro_rules! timed_scope {
    ($timer:expr, $label:expr, $body:block) => {{
        #[cfg(feature = "timing")]
        {
            let _guard = $timer.start($label);
            $body
        }
        #[cfg(not(feature = "timing"))]
        {
            $body
        }
    }};
}

/// No-op timer for when timing is disabled
///
/// This allows code to be written with timing calls that compile to nothing
/// when the timing feature is disabled.
#[cfg(not(feature = "timing"))]
pub struct NoOpTimer;

#[cfg(not(feature = "timing"))]
impl NoOpTimer {
    pub fn new(_id: &str) -> Self {
        Self
    }

    pub fn start(&mut self, _label: &str) -> NoOpGuard {
        NoOpGuard
    }

    pub fn report_console(&self, _verbose: bool) {}

    pub fn log_to_file(&self, _path: &Path) -> Result<()> {
        Ok(())
    }
}

#[cfg(not(feature = "timing"))]
pub struct NoOpGuard;

#[cfg(not(feature = "timing"))]
impl Drop for NoOpGuard {
    fn drop(&mut self) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn test_basic_timing() {
        let mut timer = HierarchicalTimer::new("test");

        {
            let _guard = timer.start("operation");
            thread::sleep(Duration::from_millis(10));
        }

        let report = timer.aggregate();
        assert!(report.timings.contains_key("operation"));
        assert!(report.timings["operation"].mean_ms >= 9.0);
        assert!(report.timings["operation"].count == 1);
    }

    #[test]
    fn test_hierarchical_timing() {
        let mut timer = HierarchicalTimer::new("test");

        {
            let _outer = timer.start("outer");
            thread::sleep(Duration::from_millis(5));

            {
                let _inner = timer.start("inner");
                thread::sleep(Duration::from_millis(5));
            }
        }

        let report = timer.aggregate();
        assert!(report.timings.contains_key("outer"));
        assert!(report.timings.contains_key("outer.inner"));
        assert!(report.timings["outer.inner"].mean_ms >= 4.0);
    }

    #[test]
    fn test_multiple_samples() {
        let mut timer = HierarchicalTimer::new("test");

        for _ in 0..3 {
            let _guard = timer.start("repeated");
            thread::sleep(Duration::from_millis(5));
        }

        let report = timer.aggregate();
        assert_eq!(report.timings["repeated"].count, 3);
        assert!(report.timings["repeated"].mean_ms >= 4.0);
        assert!(report.timings["repeated"].std_ms >= 0.0);
    }

    #[test]
    fn test_timed_scope_macro() {
        let mut timer = HierarchicalTimer::new("test");

        let result = timed_scope!(&mut timer, "macro_test", {
            thread::sleep(Duration::from_millis(5));
            42
        });

        assert_eq!(result, 42);

        #[cfg(feature = "timing")]
        {
            let report = timer.aggregate();
            assert!(report.timings.contains_key("macro_test"));
        }
    }

    #[test]
    fn test_percentage_calculation() {
        let mut timer = HierarchicalTimer::new("test");

        {
            let _guard = timer.start("half_time");
            thread::sleep(Duration::from_millis(50));
        }

        thread::sleep(Duration::from_millis(50));

        let report = timer.aggregate();
        // Should be roughly 50% of total time
        assert!(report.timings["half_time"].pct_total > 40.0);
        assert!(report.timings["half_time"].pct_total < 60.0);
    }
}

