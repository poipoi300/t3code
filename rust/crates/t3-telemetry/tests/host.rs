use t3_telemetry::host::*;
#[test]
fn cpu_affinity_uses_real_cpu_ids_and_limits_capacity() {
    let allowed = parse_cpu_list("0-1,8,10-11").expect("cpus");
    assert_eq!(allowed.len(), 5);
    assert!(parse_cpu_list("garbage").is_none());
    let a = pinned_cpu_times(
        "cpu0 10 0 5 20 1 0 0 0\ncpu8 10 0 5 20 1 0 0 0\ncpu2 900 0 0 0 0 0 0 0",
        &allowed,
    )
    .expect("times");
    let b = CpuTimes {
        idle: a.idle + 5,
        total: a.total + 10,
        count: a.count,
    };
    assert_eq!(utilization(a, b), Some(0.5));
    assert_eq!(utilization(a, a), None);
    assert_eq!(
        constrained_cpu(Some(0.5), 4, 100_000, 100_000, Some(100_000), 200_000),
        (1, Some(0.5))
    );
    assert_eq!(
        constrained_cpu(Some(1.0), 4, 100_000, 100_000, Some(0), 200_000),
        (1, Some(1.0))
    );
}
#[test]
fn memory_includes_reclaimable_cache_but_never_exceeds_host() {
    assert_eq!(
        constrained_memory(1000, 600, Some(500), Some(400), 100),
        (500, 200)
    );
    assert_eq!(
        constrained_memory(1000, 600, Some(500), None, 0),
        (1000, 600)
    );
    assert_eq!(
        constrained_memory(1000, 600, Some(500), Some(900), 0),
        (500, 0)
    );
    assert_eq!(
        darwin_available_memory(
            "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free: 10.\nPages inactive: 20.\nPages speculative: 3.\nPages purgeable: 999."
        ),
        Some(33 * 16384)
    );
}
#[tokio::test(start_paused = true)]
async fn caches_one_host_sample_for_five_seconds() {
    let host = HostResources::default();
    let first = host.read().await.expect("host");
    let next = host.read().await.expect("cached");
    assert_eq!(first, next);
    assert!(first.total_memory_bytes > 0);
    assert!(first.available_memory_bytes <= first.total_memory_bytes);
}
