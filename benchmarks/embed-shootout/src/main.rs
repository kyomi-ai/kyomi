// Embedding Shootout: ONNX Runtime (fastembed) vs Candle (pure Rust)
//
// Benchmarks BGE-small-en-v1.5 (384 dims) with identical inputs.
// Run: RUSTFLAGS="-C target-cpu=native" cargo run --release

use anyhow::Result;
use std::time::Instant;

mod candle_backend;
mod onnx_backend;

// ── Test corpus ──────────────────────────────────────────────────────────────

const SINGLE_TEXT: &str = "Represent this sentence for searching relevant passages: What is the monthly revenue by region?";

const BATCH_TEXTS: &[&str] = &[
    "Quarterly sales performance across all product lines in North America",
    "Customer retention rates have improved significantly since the new onboarding flow was deployed",
    "The engineering team is investigating a latency spike in the BigQuery connector",
    "Revenue forecast for Q3 shows a 15% increase over the previous quarter",
    "Database migration from PostgreSQL to CockroachDB was completed last weekend",
    "The marketing campaign generated 2,500 new signups in the first week",
    "API response times are averaging 45ms at the 95th percentile",
    "Board meeting presentation needs updated financial charts by Friday",
    "New feature: users can now export dashboards as PDF with custom branding",
    "Infrastructure costs decreased 22% after moving to spot instances",
    "The data pipeline processes approximately 50 million events per day",
    "User feedback indicates the search functionality needs improvement",
    "Compliance audit requires all PII data to be encrypted at rest",
    "The recommendation engine uses collaborative filtering with matrix factorization",
    "Sprint retrospective identified deployment frequency as a key bottleneck",
    "Annual recurring revenue crossed the $10M milestone this quarter",
];

// ── CPU utilization (Linux /proc/stat) ───────────────────────────────────────

#[derive(Clone)]
struct CpuSnapshot {
    user: u64,
    nice: u64,
    system: u64,
    idle: u64,
    iowait: u64,
    irq: u64,
    softirq: u64,
    steal: u64,
}

impl CpuSnapshot {
    fn read() -> Option<Self> {
        let stat = std::fs::read_to_string("/proc/stat").ok()?;
        let line = stat.lines().next()?;
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 9 || parts[0] != "cpu" {
            return None;
        }
        Some(Self {
            user: parts[1].parse().ok()?,
            nice: parts[2].parse().ok()?,
            system: parts[3].parse().ok()?,
            idle: parts[4].parse().ok()?,
            iowait: parts[5].parse().ok()?,
            irq: parts[6].parse().ok()?,
            softirq: parts[7].parse().ok()?,
            steal: parts[8].parse().ok()?,
        })
    }

    fn total(&self) -> u64 {
        self.user + self.nice + self.system + self.idle + self.iowait + self.irq + self.softirq + self.steal
    }

    fn busy(&self) -> u64 {
        self.user + self.nice + self.system + self.irq + self.softirq + self.steal
    }

    fn utilization_since(&self, before: &CpuSnapshot) -> f64 {
        let total_delta = self.total().saturating_sub(before.total());
        let busy_delta = self.busy().saturating_sub(before.busy());
        if total_delta == 0 {
            return 0.0;
        }
        (busy_delta as f64 / total_delta as f64) * 100.0
    }
}

// ── Benchmark harness ────────────────────────────────────────────────────────

struct BenchResult {
    name: String,
    load_ms: f64,
    single_ms: f64,
    batch_ms: f64,
    batch_size: usize,
    per_item_ms: f64,
    dims: usize,
    single_cpu_pct: f64,
    batch_cpu_pct: f64,
}

impl BenchResult {
    fn print(&self) {
        println!("\n═══ {} ═══", self.name);
        println!("  Model load:     {:>8.1} ms", self.load_ms);
        println!(
            "  Single embed:   {:>8.2} ms   (CPU: {:>5.1}%)",
            self.single_ms, self.single_cpu_pct
        );
        println!(
            "  Batch embed:    {:>8.2} ms   (CPU: {:>5.1}%)  ({} texts)",
            self.batch_ms, self.batch_cpu_pct, self.batch_size
        );
        println!("  Per-item:       {:>8.2} ms", self.per_item_ms);
        println!("  Dimensions:     {:>8}", self.dims);
    }
}

fn bench_single<F>(f: F, warmup: usize, iters: usize) -> (f64, f64)
where
    F: Fn() -> Result<Vec<f32>>,
{
    for _ in 0..warmup {
        f().unwrap();
    }

    let cpu_before = CpuSnapshot::read();
    let mut times = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        let _ = f().unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let cpu_after = CpuSnapshot::read();

    let cpu_pct = match (cpu_before, cpu_after) {
        (Some(b), Some(a)) => a.utilization_since(&b),
        _ => 0.0,
    };

    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[times.len() / 2], cpu_pct)
}

fn bench_batch<F>(f: F, warmup: usize, iters: usize) -> (f64, f64)
where
    F: Fn() -> Result<Vec<Vec<f32>>>,
{
    for _ in 0..warmup {
        f().unwrap();
    }

    let cpu_before = CpuSnapshot::read();
    let mut times = Vec::with_capacity(iters);
    for _ in 0..iters {
        let start = Instant::now();
        let _ = f().unwrap();
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let cpu_after = CpuSnapshot::read();

    let cpu_pct = match (cpu_before, cpu_after) {
        (Some(b), Some(a)) => a.utilization_since(&b),
        _ => 0.0,
    };

    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (times[times.len() / 2], cpu_pct)
}

// ── Main ─────────────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    println!("╔══════════════════════════════════════════════════════════╗");
    println!("║  Embedding Shootout: ONNX Runtime vs Candle (pure Rust) ║");
    println!("║  Model: BGE-small-en-v1.5 (384 dims)                   ║");
    println!("╚══════════════════════════════════════════════════════════╝");

    // Print CPU info
    if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
        if let Some(model) = info.lines().find(|l| l.starts_with("model name")) {
            println!("\n  {}", model.trim());
        }
    }
    println!("  Cores: {}", std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0));
    println!("  CPU utilization is across ALL cores (100% = 1 core fully used, 800% max on 8 cores)");

    let warmup = 5;
    let iters = 20;

    println!("\nConfig: {warmup} warmup, {iters} iterations, median timing");
    println!("Batch size: {} texts", BATCH_TEXTS.len());

    // ── ONNX Runtime ─────────────────────────────────────────────────────

    let start = Instant::now();
    let onnx = onnx_backend::OnnxEmbedder::new()?;
    let onnx_load_ms = start.elapsed().as_secs_f64() * 1000.0;

    let (onnx_single_ms, onnx_single_cpu) =
        bench_single(|| onnx.embed_single(SINGLE_TEXT), warmup, iters);
    let (onnx_batch_ms, onnx_batch_cpu) =
        bench_batch(|| onnx.embed_batch(BATCH_TEXTS), warmup, iters);
    let onnx_dims = onnx.embed_single(SINGLE_TEXT)?.len();

    let onnx_result = BenchResult {
        name: "ONNX Runtime (fastembed)".into(),
        load_ms: onnx_load_ms,
        single_ms: onnx_single_ms,
        batch_ms: onnx_batch_ms,
        batch_size: BATCH_TEXTS.len(),
        per_item_ms: onnx_batch_ms / BATCH_TEXTS.len() as f64,
        dims: onnx_dims,
        single_cpu_pct: onnx_single_cpu,
        batch_cpu_pct: onnx_batch_cpu,
    };

    // Drop ONNX to free memory before loading Candle
    drop(onnx);

    // ── Candle (default threading) ──────────────────────────────────────

    let start = Instant::now();
    let candle = candle_backend::CandleEmbedder::new()?;
    let candle_load_ms = start.elapsed().as_secs_f64() * 1000.0;

    let (candle_single_ms, candle_single_cpu) =
        bench_single(|| candle.embed_single(SINGLE_TEXT), warmup, iters);
    let (candle_batch_ms, candle_batch_cpu) =
        bench_batch(|| candle.embed_batch(BATCH_TEXTS), warmup, iters);
    let candle_dims = candle.embed_single(SINGLE_TEXT)?.len();

    let candle_result = BenchResult {
        name: "Candle (default threading)".into(),
        load_ms: candle_load_ms,
        single_ms: candle_single_ms,
        batch_ms: candle_batch_ms,
        batch_size: BATCH_TEXTS.len(),
        per_item_ms: candle_batch_ms / BATCH_TEXTS.len() as f64,
        dims: candle_dims,
        single_cpu_pct: candle_single_cpu,
        batch_cpu_pct: candle_batch_cpu,
    };

    // ── Candle (aggressive threading — threshold=0) ──────────────────────

    println!("\n  [Lowering gemm threading threshold to 0 — force all matmuls to use threads]");
    gemm::set_threading_threshold(0);

    let (candle_tuned_single_ms, candle_tuned_single_cpu) =
        bench_single(|| candle.embed_single(SINGLE_TEXT), warmup, iters);
    let (candle_tuned_batch_ms, candle_tuned_batch_cpu) =
        bench_batch(|| candle.embed_batch(BATCH_TEXTS), warmup, iters);

    let candle_tuned_result = BenchResult {
        name: "Candle (threshold=0, all matmuls threaded)".into(),
        load_ms: candle_load_ms,
        single_ms: candle_tuned_single_ms,
        batch_ms: candle_tuned_batch_ms,
        batch_size: BATCH_TEXTS.len(),
        per_item_ms: candle_tuned_batch_ms / BATCH_TEXTS.len() as f64,
        dims: candle_dims,
        single_cpu_pct: candle_tuned_single_cpu,
        batch_cpu_pct: candle_tuned_batch_cpu,
    };

    // ── Results ──────────────────────────────────────────────────────────

    onnx_result.print();
    candle_result.print();
    candle_tuned_result.print();

    // ── Comparison ───────────────────────────────────────────────────────

    fn print_comparison(label: &str, candle_ms: f64, onnx_ms: f64) {
        let ratio = candle_ms / onnx_ms;
        println!(
            "  {:<18} Candle is {:.1}x {} than ONNX",
            label,
            if ratio > 1.0 { ratio } else { 1.0 / ratio },
            if ratio > 1.0 { "slower" } else { "faster" }
        );
    }

    println!("\n═══ Comparison: Default Candle vs ONNX ═══");
    print_comparison("Model load:", candle_result.load_ms, onnx_result.load_ms);
    print_comparison("Single embed:", candle_result.single_ms, onnx_result.single_ms);
    print_comparison("Batch embed:", candle_result.batch_ms, onnx_result.batch_ms);

    println!("\n═══ Comparison: Tuned Candle (threshold=0) vs ONNX ═══");
    print_comparison("Single embed:", candle_tuned_result.single_ms, onnx_result.single_ms);
    print_comparison("Batch embed:", candle_tuned_result.batch_ms, onnx_result.batch_ms);

    println!("\n═══ Tuning impact (default → threshold=0) ═══");
    let single_improvement = (1.0 - candle_tuned_result.single_ms / candle_result.single_ms) * 100.0;
    let batch_improvement = (1.0 - candle_tuned_result.batch_ms / candle_result.batch_ms) * 100.0;
    println!("  Single embed: {:.1}% {}", single_improvement.abs(),
        if single_improvement > 0.0 { "faster" } else { "slower" });
    println!("  Batch embed:  {:.1}% {}", batch_improvement.abs(),
        if batch_improvement > 0.0 { "faster" } else { "slower" });

    // ── Correctness check ────────────────────────────────────────────────

    println!("\n═══ Correctness ═══");
    let onnx_emb = onnx_backend::OnnxEmbedder::new()?.embed_single("hello world")?;
    let candle_emb = candle.embed_single("hello world")?;

    let dot: f32 = onnx_emb.iter().zip(candle_emb.iter()).map(|(a, b)| a * b).sum();
    let norm_a: f32 = onnx_emb.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = candle_emb.iter().map(|x| x * x).sum::<f32>().sqrt();
    let cosine_sim = dot / (norm_a * norm_b);

    println!("  Cosine similarity (same input): {:.6}", cosine_sim);
    if cosine_sim > 0.99 {
        println!("  ✓ Embeddings are equivalent (>0.99 cosine similarity)");
    } else {
        println!("  ⚠ Embeddings diverge — check pooling/normalization");
    }

    Ok(())
}
