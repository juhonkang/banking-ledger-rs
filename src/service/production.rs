//! Production hardening — post-mortems, performance tuning, stress testing,
//! security hardening, and thundering herd mitigation.

use std::time::{Duration, Instant};

// ━━━ Post-Mortem ━━━

/// Structured post-mortem document for incident analysis.
/// Blameless: focuses on systems, not people.
#[derive(Debug, Clone)]
pub struct PostMortem {
    pub incident_id: String,
    pub title: String,
    pub severity: IncidentSeverity,
    pub detected_at: chrono::DateTime<chrono::Utc>,
    pub resolved_at: chrono::DateTime<chrono::Utc>,
    pub duration: Duration,
    pub authors: Vec<String>,

    /// What happened — objective timeline
    pub timeline: Vec<TimelineEntry>,

    /// Root cause analysis (5 Whys)
    pub root_causes: Vec<String>,

    /// What went well
    pub what_went_well: Vec<String>,

    /// What went wrong
    pub what_went_wrong: Vec<String>,

    /// Action items with owners
    pub action_items: Vec<ActionItem>,

    /// Lessons learned
    pub lessons_learned: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncidentSeverity {
    Sev1Critical, // System down, data loss
    Sev2Major,    // Major feature broken
    Sev3Minor,    // Minor impact
    Sev4Cosmetic, // No user impact
}

#[derive(Debug, Clone)]
pub struct TimelineEntry {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub event: String,
    pub actor: String,
}

#[derive(Debug, Clone)]
pub struct ActionItem {
    pub description: String,
    pub owner: String,
    pub deadline: Option<chrono::DateTime<chrono::Utc>>,
    pub status: ActionStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionStatus {
    Todo,
    InProgress,
    Done,
    WontFix,
}

impl PostMortem {
    /// Generate a full post-mortem report in markdown
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        md.push_str(&format!("# Post-Mortem: {}\n\n", self.title));
        md.push_str(&format!("- **Incident**: {}\n", self.incident_id));
        md.push_str(&format!("- **Severity**: {:?}\n", self.severity));
        md.push_str(&format!("- **Duration**: {}s\n", self.duration.as_secs()));
        md.push_str(&format!("- **Detected**: {}\n", self.detected_at));
        md.push_str(&format!("- **Resolved**: {}\n\n", self.resolved_at));

        md.push_str("## Timeline\n\n");
        for entry in &self.timeline {
            md.push_str(&format!(
                "- **{}** — {} (by {})\n",
                entry.timestamp.format("%H:%M:%S"),
                entry.event,
                entry.actor
            ));
        }

        md.push_str("\n## Root Cause Analysis\n\n");
        for (i, cause) in self.root_causes.iter().enumerate() {
            md.push_str(&format!("{}. {}\n", i + 1, cause));
        }

        md.push_str("\n## Action Items\n\n");
        for item in &self.action_items {
            let status = match item.status {
                ActionStatus::Todo => "🔴 TODO",
                ActionStatus::InProgress => "🟡 IN PROGRESS",
                ActionStatus::Done => "🟢 DONE",
                ActionStatus::WontFix => "⚫ WONTFIX",
            };
            md.push_str(&format!(
                "- [{}] {} — Owner: {}\n",
                status, item.description, item.owner
            ));
        }

        md
    }
}

// ━━━ Performance Tuning ━━━

/// Configuration for low-latency system tuning.
/// Rust doesn't have a GC to tune, but we can pin threads and use huge pages.
#[derive(Debug, Clone)]
pub struct PerformanceTuning {
    /// Core pinning: which CPUs to pin worker threads to
    pub cpu_affinity: Vec<usize>,
    /// Enable transparent huge pages
    pub huge_pages: bool,
    /// Use memlock to prevent swapping
    pub memlock: bool,
    /// `SO_REUSEPORT` for multi-process
    pub reuse_port: bool,
    /// `TCP_NODELAY` for low-latency networking
    pub tcp_nodelay: bool,
    /// Socket buffer sizes
    pub send_buffer_size: usize,
    pub recv_buffer_size: usize,
}

impl Default for PerformanceTuning {
    fn default() -> Self {
        Self {
            cpu_affinity: vec![0],
            huge_pages: true,
            memlock: true,
            reuse_port: true,
            tcp_nodelay: true,
            send_buffer_size: 4_194_304, // 4MB
            recv_buffer_size: 4_194_304, // 4MB
        }
    }
}

impl PerformanceTuning {
    /// Apply network socket tuning
    pub fn apply_to_socket(&self, socket: &std::net::TcpStream) -> std::io::Result<()> {
        use std::os::unix::io::AsRawFd;

        let fd = socket.as_raw_fd();

        // TCP_NODELAY
        if self.tcp_nodelay {
            socket.set_nodelay(true)?;
        }

        // SO_REUSEPORT (via setsockopt)
        if self.reuse_port {
            unsafe {
                let optval: libc::c_int = 1;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_REUSEPORT,
                    (&raw const optval).cast::<libc::c_void>(),
                    std::mem::size_of::<libc::c_int>() as u32,
                );
            }
        }

        // Send/Recv buffer sizes
        if self.send_buffer_size > 0 {
            unsafe {
                let sz = self.send_buffer_size as libc::c_int;
                libc::setsockopt(
                    fd,
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&raw const sz).cast::<libc::c_void>(),
                    std::mem::size_of::<libc::c_int>() as u32,
                );
            }
        }

        Ok(())
    }

    /// Generate Linux kernel tuning recommendations
    pub fn kernel_recommendations(&self) -> Vec<String> {
        let mut recs = vec![
            "# CPU isolation (GRUB)".into(),
            "isolcpus=1-7 nohz_full=1-7 rcu_nocbs=1-7".into(),
        ];

        if self.huge_pages {
            recs.push("# Transparent Huge Pages".into());
            recs.push("echo always > /sys/kernel/mm/transparent_hugepage/enabled".into());
            recs.push("echo defer+madvise > /sys/kernel/mm/transparent_hugepage/defrag".into());
        }

        if self.memlock {
            recs.push("# Prevent swapping".into());
            recs.push("* soft memlock unlimited".into());
        }

        if self.reuse_port {
            recs.push("# Network tuning".into());
            recs.push("net.core.somaxconn = 65535".into());
            recs.push("net.ipv4.tcp_max_syn_backlog = 65535".into());
        }

        recs
    }
}

// ━━━ Thundering Herd ━━━

/// Simulates thundering herd problem and tests mitigation strategies.
pub struct ThunderingHerdSim {
    /// Number of concurrent "wakers"
    pub num_clients: usize,
    /// Jitter range (random delay before retry)
    pub jitter_ms: u64,
    /// Exponential backoff base
    pub backoff_base_ms: u64,
}

impl Default for ThunderingHerdSim {
    fn default() -> Self {
        Self {
            num_clients: 1000,
            jitter_ms: 50,
            backoff_base_ms: 100,
        }
    }
}

impl ThunderingHerdSim {
    /// Simulate thundering herd: all clients wake up simultaneously.
    /// Returns (`avg_latency`, `max_latency`, `success_rate`)
    pub fn simulate_synchronized(&self) -> (Duration, Duration, f64) {
        let start = Instant::now();
        let mut latencies = Vec::with_capacity(self.num_clients);

        // All clients hit simultaneously
        for _ in 0..self.num_clients {
            let req_start = Instant::now();
            // Simulate work
            std::thread::sleep(Duration::from_micros(100));
            latencies.push(req_start.elapsed());
        }

        let _total = start.elapsed();
        let avg: Duration = if latencies.is_empty() { Duration::ZERO } else { latencies.iter().sum::<Duration>() / latencies.len() as u32 };
        let max = latencies.iter().max().copied().unwrap_or(Duration::ZERO);

        (avg, max, 1.0)
    }

    /// Simulate with jitter mitigation: random delays spread the load.
    pub fn simulate_with_jitter(&self) -> (Duration, Duration, f64) {
        let mut latencies = Vec::with_capacity(self.num_clients);

        for _ in 0..self.num_clients {
            // Random jitter before request
            let jitter = rand::random::<u64>() % self.jitter_ms;
            std::thread::sleep(Duration::from_millis(jitter));

            let req_start = Instant::now();
            std::thread::sleep(Duration::from_micros(100));
            latencies.push(req_start.elapsed());
        }

        let avg: Duration = if latencies.is_empty() { Duration::ZERO } else { latencies.iter().sum::<Duration>() / latencies.len() as u32 };
        let max = latencies.iter().max().copied().unwrap_or(Duration::ZERO);
        (avg, max, 1.0)
    }

    /// Simulate with exponential backoff
    pub fn simulate_with_backoff(&self, max_retries: u32) -> (Duration, Duration, f64) {
        let mut successes = 0;
        let mut latencies = Vec::new();

        for _i in 0..self.num_clients {
            for attempt in 0..max_retries {
                if attempt > 0 {
                    let delay = self.backoff_base_ms * 2u64.pow(attempt);
                    std::thread::sleep(Duration::from_millis(delay));
                }

                let req_start = Instant::now();
                let success = rand::random::<f64>() > 0.3; // 70% success rate

                if success {
                    latencies.push(req_start.elapsed());
                    successes += 1;
                    break;
                }
            }
        }

        let avg: Duration = if latencies.is_empty() {
            Duration::ZERO
        } else {
            if latencies.is_empty() { Duration::ZERO } else { latencies.iter().sum::<Duration>() / latencies.len() as u32 }
        };
        let max = latencies.iter().max().copied().unwrap_or(Duration::ZERO);
        (avg, max, f64::from(successes) / self.num_clients as f64)
    }

    /// Full comparison report
    pub fn compare_strategies(&self) -> String {
        let (sync_avg, sync_max, _) = self.simulate_synchronized();
        let (jit_avg, jit_max, _) = self.simulate_with_jitter();
        let (bo_avg, bo_max, bo_rate) = self.simulate_with_backoff(5);

        format!(
            "Thundering Herd ({n} clients):\n\
             ┌─────────────────┬──────────────┬──────────────┐\n\
             │ Strategy        │ Avg Latency  │ Max Latency  │\n\
             ├─────────────────┼──────────────┼──────────────┤\n\
             │ Synchronized    │ {sa:>12.1?} │ {sm:>12.1?} │\n\
             │ Jitter ({j}ms)  │ {ja:>12.1?} │ {jm:>12.1?} │\n\
             │ Backoff (70% SR)│ {ba:>12.1?} │ {bm:>12.1?} │\n\
             └─────────────────┴──────────────┴──────────────┘\n\
             Backoff success rate: {br:.1}%",
            n = self.num_clients,
            j = self.jitter_ms,
            sa = sync_avg,
            sm = sync_max,
            ja = jit_avg,
            jm = jit_max,
            ba = bo_avg,
            bm = bo_max,
            br = bo_rate * 100.0,
        )
    }
}

// ━━━ Security Hardening ━━━

/// Security hardening checklist for production banking ledger
#[derive(Debug, Clone)]
pub struct SecurityHardening {
    pub input_validation: bool,
    pub sql_injection_prevention: bool, // N/A if no SQL
    pub rate_limiting: bool,
    pub tls_enabled: bool,
    pub authentication: bool,
    pub authorization: bool,
    pub audit_logging: bool,
    pub secret_rotation: bool,
    pub dependency_scanning: bool,
}

impl Default for SecurityHardening {
    fn default() -> Self {
        Self {
            input_validation: true,
            sql_injection_prevention: false, // No SQL in this ledger
            rate_limiting: true,
            tls_enabled: false,    // Enable in production
            authentication: false, // Add auth middleware
            authorization: false,  // Add RBAC
            audit_logging: true,
            secret_rotation: false, // Rotate HMAC keys
            dependency_scanning: true,
        }
    }
}

impl SecurityHardening {
    /// Validate transaction input (prevent injection, overflow, etc.)
    pub fn validate_input(amount_cents: i64, account_id: &str) -> Result<(), String> {
        // Amount validation
        if amount_cents <= 0 {
            return Err("Amount must be positive".into());
        }
        if amount_cents > 1_000_000_000_000 {
            // $10B max per transaction
            return Err("Amount exceeds maximum".into());
        }

        // UUID validation
        if uuid::Uuid::parse_str(account_id).is_err() {
            return Err("Invalid account ID format".into());
        }

        Ok(())
    }

    /// Audit log entry
    pub fn audit_log(action: &str, actor: &str, details: &str) {
        let timestamp = chrono::Utc::now();
        eprintln!(
            "[AUDIT {}] {} | {} | {}",
            timestamp.format("%Y-%m-%dT%H:%M:%S%.3fZ"),
            action,
            actor,
            details
        );
    }

    /// Generate security report
    pub fn report(&self) -> String {
        format!(
            "Security Hardening Status:\n\
             ┌──────────────────────────┬────────┐\n\
             │ Check                    │ Status │\n\
             ├──────────────────────────┼────────┤\n\
             │ Input Validation         │ {:>6} │\n\
             │ Rate Limiting            │ {:>6} │\n\
             │ TLS (HTTPS)              │ {:>6} │\n\
             │ Audit Logging            │ {:>6} │\n\
             │ Dependency Scanning      │ {:>6} │\n\
             │ Secret Rotation          │ {:>6} │\n\
             │ RBAC Authorization       │ {:>6} │\n\
             └──────────────────────────┴────────┘",
            self.checkmark(self.input_validation),
            self.checkmark(self.rate_limiting),
            self.checkmark(self.tls_enabled),
            self.checkmark(self.audit_logging),
            self.checkmark(self.dependency_scanning),
            self.checkmark(self.secret_rotation),
            self.checkmark(self.authorization),
        )
    }

    fn checkmark(&self, v: bool) -> &str {
        if v {
            "✅"
        } else {
            "❌"
        }
    }
}

// ━━━ Stress Test ━━━

/// Benchmark harness for stress-testing the ledger at scale.
pub struct StressTest {
    pub num_accounts: usize,
    pub num_threads: usize,
    pub ops_per_thread: usize,
    pub report_interval_ms: u64,
}

impl Default for StressTest {
    fn default() -> Self {
        Self {
            num_accounts: 10_000,
            num_threads: 16,
            ops_per_thread: 100_000,
            report_interval_ms: 1000,
        }
    }
}

impl StressTest {
    /// Run a throughput benchmark.
    /// Returns operations per second (throughput) and average latency.
    pub fn run_benchmark<F>(&self, operation: F) -> StressTestResult
    where
        F: Fn() + Send + Sync + Clone + 'static,
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        use std::sync::Arc;

        let operation = Arc::new(operation);
        let counter = Arc::new(AtomicU64::new(0));
        let start = Instant::now();
        let mut handles = vec![];

        for _ in 0..self.num_threads {
            let counter = Arc::clone(&counter);
            let operation = Arc::clone(&operation);
            let ops = self.ops_per_thread;
            handles.push(std::thread::spawn(move || {
                for _ in 0..ops {
                    operation();
                    counter.fetch_add(1, Ordering::Relaxed);
                }
            }));
        }

        for h in handles {
            h.join().unwrap();
        }

        let elapsed = start.elapsed();
        let total_ops = counter.load(Ordering::Relaxed);
        let throughput = total_ops as f64 / elapsed.as_secs_f64();
        let avg_latency = if total_ops > 0 { elapsed / total_ops as u32 } else { Duration::ZERO };

        StressTestResult {
            total_ops,
            duration: elapsed,
            throughput_rps: throughput,
            avg_latency,
        }
    }

    /// Projection: what throughput we'd need for 100M RPS
    pub fn project_100m_rps(&self, current_throughput: f64) -> String {
        let needed = 100_000_000.0 / current_throughput;
        format!(
            "100M RPS Projection:\n\
             Current: {:.0} ops/sec\n\
             To reach 100M RPS: need {:.0}x more throughput\n\
             Options: shard across {:.0} nodes, or optimize {:.1}x per node",
            current_throughput,
            needed,
            needed.ceil(),
            needed.sqrt()
        )
    }

    /// Capstone demo — runs all systems and prints report
    pub fn capstone_demo<F>(&self, name: &str, operation: F) -> String
    where
        F: Fn() + Send + Sync + Clone + 'static,
    {
        let result = self.run_benchmark(operation);
        let projection = self.project_100m_rps(result.throughput_rps);

        format!(
            "═══ CAPSTONE: {} ═══\n\
             Total Operations:  {:>12}\n\
             Duration:          {:>12.2?}\n\
             Throughput:        {:>12.0} ops/sec\n\
             Avg Latency:       {:>12.2?}\n\
             \n{}\n\
             ═══════════════════════════════",
            name,
            result.total_ops,
            result.duration,
            result.throughput_rps,
            result.avg_latency,
            projection,
        )
    }
}

#[derive(Debug, Clone)]
pub struct StressTestResult {
    pub total_ops: u64,
    pub duration: Duration,
    pub throughput_rps: f64,
    pub avg_latency: Duration,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_post_mortem_markdown() {
        let pm = PostMortem {
            incident_id: "INC-001".into(),
            title: "Ledger API Latency Spike".into(),
            severity: IncidentSeverity::Sev2Major,
            detected_at: chrono::Utc::now(),
            resolved_at: chrono::Utc::now(),
            duration: Duration::from_secs(300),
            authors: vec!["oncall".into()],
            timeline: vec![TimelineEntry {
                timestamp: chrono::Utc::now(),
                event: "P99 latency spiked to 500ms".into(),
                actor: "monitoring".into(),
            }],
            root_causes: vec!["Connection pool exhaustion".into()],
            what_went_well: vec!["Circuit breaker prevented cascade".into()],
            what_went_wrong: vec!["Alert fired 5min after impact".into()],
            action_items: vec![ActionItem {
                description: "Increase connection pool size".into(),
                owner: "platform-team".into(),
                deadline: None,
                status: ActionStatus::Todo,
            }],
            lessons_learned: vec!["Monitor connection pool saturation".into()],
        };

        let md = pm.to_markdown();
        assert!(md.contains("INC-001"));
        assert!(md.contains("Latency Spike"));
        assert!(md.contains("Connection pool"));
    }

    #[test]
    fn test_security_input_validation() {
        // Invalid UUID format
        assert!(SecurityHardening::validate_input(1000, "not-a-uuid").is_err());
        assert!(SecurityHardening::validate_input(1000, "bad").is_err());
        // Negative amount
        assert!(
            SecurityHardening::validate_input(-100, "67e55044-10b1-426f-9247-bb680e5fe0c8")
                .is_err()
        );
        // Valid input
        assert!(
            SecurityHardening::validate_input(100, "67e55044-10b1-426f-9247-bb680e5fe0c8").is_ok()
        );
        // Amount exceeds max
        assert!(SecurityHardening::validate_input(
            2_000_000_000_000,
            "67e55044-10b1-426f-9247-bb680e5fe0c8"
        )
        .is_err());
    }

    #[test]
    fn test_stress_benchmark() {
        let stress = StressTest {
            num_threads: 2,
            ops_per_thread: 1000,
            ..Default::default()
        };

        let result = stress.run_benchmark(|| {
            // Simulate a CAS operation
            std::hint::spin_loop();
        });

        assert!(result.total_ops >= 2000);
        assert!(result.throughput_rps > 0.0);
    }

    #[test]
    fn test_capstone_demo() {
        let stress = StressTest {
            num_threads: 2,
            ops_per_thread: 100,
            ..Default::default()
        };

        let report = stress.capstone_demo("CAS Balance Update", || {
            std::hint::spin_loop();
        });

        assert!(report.contains("CAPSTONE"));
        assert!(report.contains("Throughput"));
        assert!(report.contains("100M RPS Projection"));
    }
}
