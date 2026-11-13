use t3_telemetry::observability::*;

#[test]
fn rpc_labels_cover_contract_methods_and_keep_diagnostic_queries_untraced() {
    for method in t3_contracts::rpc::WS_METHODS {assert_ne!(t3_telemetry::rpc::aggregate(method.tag),"unknown","{}",method.tag);}
    assert_eq!(t3_telemetry::rpc::aggregate("orchestration.launchThread"),"orchestration");
    assert!(t3_telemetry::rpc::tracing_disabled("server.getTraceDiagnostics"));
    assert!(!t3_telemetry::rpc::tracing_disabled("server.getHostResources"));
}
#[test]
fn idle_system_sleep_does_not_become_an_event_loop_stall() {
    let mut readings = EventLoopReadings {
        delay_max_ns: 600_000 * 1_000_000,
        active_ms: 900,
        cpu_count: 18,
        load_average: Some(219.43),
    };
    assert_eq!(stall_ms(&readings), None);
    readings.active_ms = 600000;
    assert_eq!(stall_ms(&readings), Some(599000));
    let mut monitor = StallMonitor::default();
    monitor.sample(&readings, 1000);
    assert!(monitor.latest_noticeable_stall(301000).is_some());
    assert!(monitor.latest_noticeable_stall(301001).is_none());
}
#[test]
fn heap_spike_names_recent_large_spans_without_old_subscriptions() {
    const MB: u64 = 1024 * 1024;
    let mut heap = 300 * MB;
    let mut attribution = HeapAttribution::default();
    attribution.sample(heap);
    attribution.open(1, "subscription", heap);
    attribution.open(2, "old", heap);
    for step in 0..5 {
        heap += 80 * MB;
        for _ in 0..11 {
            attribution.sample(heap);
        }
        if step == 1 {
            assert_eq!(attribution.end(2, heap), Some(160 * MB));
        }
    }
    attribution.open(3, "small", heap);
    attribution.open(4, "big", heap);
    heap += 2 * MB;
    assert_eq!(attribution.end(3, heap), None);
    heap += 198 * MB;
    let spike = attribution.sample(heap).expect("spike");
    assert_eq!(
        spike.suspects,
        vec![HeapSuspect {
            name: "big".into(),
            growth_bytes: 200 * MB
        }]
    );
    assert_eq!(attribution.end(4, heap), Some(200 * MB));
    assert!(attribution.sample(heap + 500 * MB).is_none());
}
