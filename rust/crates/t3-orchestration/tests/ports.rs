#[path = "ports/adapters__acp_adapter_v2_test.rs"]
mod adapters_acp_adapter_v2_test;
#[path = "ports/adapters__acp_registry_adapter_v2_test.rs"]
mod adapters_acp_registry_adapter_v2_test;
#[path = "ports/adapters__antigravity_adapter_v2_test.rs"]
mod adapters_antigravity_adapter_v2_test;
#[path = "ports/adapters__claude_adapter_v2_test.rs"]
mod adapters_claude_adapter_v2_test;
#[path = "ports/adapters__claude_agent_sdk_query_runner_test.rs"]
mod adapters_claude_agent_sdk_query_runner_test;
#[path = "ports/adapters__claude_task_output_tail_test.rs"]
mod adapters_claude_task_output_tail_test;
#[path = "ports/adapters__claude_workflow_progress_test.rs"]
mod adapters_claude_workflow_progress_test;
#[path = "ports/adapters__codex_adapter_v2_test.rs"]
mod adapters_codex_adapter_v2_test;
#[path = "ports/adapters__fx_adapter_v2_test.rs"]
mod adapters_fx_adapter_v2_test;
#[path = "ports/adapters__grok_adapter_v2_test.rs"]
mod adapters_grok_adapter_v2_test;
#[path = "ports/adapters__kiro_adapter_v2_test.rs"]
mod adapters_kiro_adapter_v2_test;
#[path = "ports/adapters__open_code2_adapter_v2_test.rs"]
mod adapters_open_code2_adapter_v2_test;
#[path = "ports/assistant_streaming_test.rs"]
mod assistant_streaming_test;
#[path = "ports/attachment_claims_test.rs"]
mod attachment_claims_test;
#[path = "ports/background_work_stop_integration_test.rs"]
mod background_work_stop_integration_test;
#[path = "ports/bounded_snapshot_transport_test.rs"]
mod bounded_snapshot_transport_test;
#[path = "ports/checkpoint_capture_service_test.rs"]
mod checkpoint_capture_service_test;
#[path = "ports/checkpoint_diff_query_test.rs"]
mod checkpoint_diff_query_test;
#[path = "ports/checkpoint_restore_safety_test.rs"]
mod checkpoint_restore_safety_test;
#[path = "ports/checkpoint_rollback_service_test.rs"]
mod checkpoint_rollback_service_test;
#[path = "ports/checkpoint_scope_ownership_test.rs"]
mod checkpoint_scope_ownership_test;
#[path = "ports/checkpoint_service_test.rs"]
mod checkpoint_service_test;
#[path = "ports/claude_automatic_delivery_integration_test.rs"]
mod claude_automatic_delivery_integration_test;
#[path = "ports/command_output_hub_test.rs"]
mod command_output_hub_test;
#[path = "ports/command_policy_test.rs"]
mod command_policy_test;
#[path = "ports/context_handoff_budget_test.rs"]
mod context_handoff_budget_test;
#[path = "ports/context_handoff_service_test.rs"]
mod context_handoff_service_test;
#[path = "ports/delegated_completion_delivery_test.rs"]
mod delegated_completion_delivery_test;
#[path = "ports/dispatch_mode_limit_test.rs"]
mod dispatch_mode_limit_test;
#[path = "ports/effect_worker_test.rs"]
mod effect_worker_test;
#[path = "ports/foundation_persistence_test.rs"]
mod foundation_persistence_test;
#[path = "ports/guest_share_test.rs"]
mod guest_share_test;
#[path = "ports/http_test.rs"]
mod http_test;
#[path = "ports/idle_compaction_test.rs"]
mod idle_compaction_test;
#[path = "ports/legacy__legacy_v1_cutover_integration_test.rs"]
mod legacy_legacy_v1_cutover_integration_test;
#[path = "ports/legacy__legacy_v1_thread_importer_test.rs"]
mod legacy_legacy_v1_thread_importer_test;
#[path = "ports/live_stream_budget_test.rs"]
mod live_stream_budget_test;
#[path = "ports/model_switch_test.rs"]
mod model_switch_test;
#[path = "ports/native_child_work_query_test.rs"]
mod native_child_work_query_test;
#[path = "ports/open_code2_orchestrator_v2_integration_test.rs"]
mod open_code2_orchestrator_v2_integration_test;
#[path = "ports/orchestrator_control_reads_test.rs"]
mod orchestrator_control_reads_test;
#[path = "ports/orchestrator_migration_test.rs"]
mod orchestrator_migration_test;
#[path = "ports/orchestrator_receipt_race_test.rs"]
mod orchestrator_receipt_race_test;
#[path = "ports/persistence__orchestration_event_store_sequence_test.rs"]
mod persistence_orchestration_event_store_sequence_test;
#[path = "ports/persistence__orchestration_event_store_test.rs"]
mod persistence_orchestration_event_store_test;
#[path = "ports/persistence__repository_error_correlation_test.rs"]
mod persistence_repository_error_correlation_test;
#[path = "ports/project_commands_test.rs"]
mod project_commands_test;
#[path = "ports/project_settings_upgrade_integration_test.rs"]
mod project_settings_upgrade_integration_test;
#[path = "ports/project_store_test.rs"]
mod project_store_test;
#[path = "ports/projection_control_reads_test.rs"]
mod projection_control_reads_test;
#[path = "ports/projection_maintenance_test.rs"]
mod projection_maintenance_test;
#[path = "ports/projection_recovery_test.rs"]
mod projection_recovery_test;
#[path = "ports/projection_settlement_test.rs"]
mod projection_settlement_test;
#[path = "ports/projection_store_test.rs"]
mod projection_store_test;
#[path = "ports/provider_adapter_registry_test.rs"]
mod provider_adapter_registry_test;
#[path = "ports/provider_commit_batch_test.rs"]
mod provider_commit_batch_test;
#[path = "ports/provider_continuation_service_test.rs"]
mod provider_continuation_service_test;
#[path = "ports/provider_event_ingestor_test.rs"]
mod provider_event_ingestor_test;
#[path = "ports/provider_runtime_recovery_service_regression_test.rs"]
mod provider_runtime_recovery_service_regression_test;
#[path = "ports/provider_runtime_recovery_service_test.rs"]
mod provider_runtime_recovery_service_test;
#[path = "ports/provider_session_manager_test.rs"]
mod provider_session_manager_test;
#[path = "ports/provider_session_transition_policy_test.rs"]
mod provider_session_transition_policy_test;
#[path = "ports/provider_switch_service_test.rs"]
mod provider_switch_service_test;
#[path = "ports/provider_turn_control_service_test.rs"]
mod provider_turn_control_service_test;
#[path = "ports/provider_turn_start_service_test.rs"]
mod provider_turn_start_service_test;
#[path = "ports/pull_request_sync_reactor_test.rs"]
mod pull_request_sync_reactor_test;
#[path = "ports/pull_request_watch_test.rs"]
mod pull_request_watch_test;
#[path = "ports/queued_run_order_test.rs"]
mod queued_run_order_test;
#[path = "ports/remote_delegated_task_test.rs"]
mod remote_delegated_task_test;
#[path = "ports/restart_background_note_test.rs"]
mod restart_background_note_test;
#[path = "ports/restart_continuation_test.rs"]
mod restart_continuation_test;
#[path = "ports/run_completion_reads_test.rs"]
mod run_completion_reads_test;
#[path = "ports/run_execution_service_test.rs"]
mod run_execution_service_test;
#[path = "ports/run_finalization_queue_test.rs"]
mod run_finalization_queue_test;
#[path = "ports/run_finalization_service_test.rs"]
mod run_finalization_service_test;
#[path = "ports/run_metrics_test.rs"]
mod run_metrics_test;
#[path = "ports/runtime_layer_test.rs"]
mod runtime_layer_test;
#[path = "ports/runtime_policy_test.rs"]
mod runtime_policy_test;
#[path = "ports/runtime_request_service_test.rs"]
mod runtime_request_service_test;
#[path = "ports/selection_restart_integration_test.rs"]
mod selection_restart_integration_test;
#[path = "ports/shell_stream_test.rs"]
mod shell_stream_test;
#[path = "ports/steering_completion_integration_test.rs"]
mod steering_completion_integration_test;
#[path = "ports/subagent_projection_test.rs"]
mod subagent_projection_test;
#[path = "ports/testkit__acp_registry_replay_fixtures_integration_test.rs"]
mod testkit_acp_registry_replay_fixtures_integration_test;
#[path = "ports/testkit__claude_replay_fixtures_integration_test.rs"]
mod testkit_claude_replay_fixtures_integration_test;
#[path = "ports/testkit__codex_replay_fixtures_integration_test.rs"]
mod testkit_codex_replay_fixtures_integration_test;
#[path = "ports/testkit__cursor_replay_fixtures_integration_test.rs"]
mod testkit_cursor_replay_fixtures_integration_test;
#[path = "ports/testkit__fx_replay_fixtures_integration_test.rs"]
mod testkit_fx_replay_fixtures_integration_test;
#[path = "ports/testkit__grok_replay_fixtures_integration_test.rs"]
mod testkit_grok_replay_fixtures_integration_test;
#[path = "ports/testkit__kiro_replay_fixtures_integration_test.rs"]
mod testkit_kiro_replay_fixtures_integration_test;
#[path = "ports/testkit__muse_replay_fixtures_integration_test.rs"]
mod testkit_muse_replay_fixtures_integration_test;
#[path = "ports/testkit__open_code2_pending_snapshots_evidence_test.rs"]
mod testkit_open_code2_pending_snapshots_evidence_test;
#[path = "ports/testkit__open_code_replay_fixtures_integration_test.rs"]
mod testkit_open_code_replay_fixtures_integration_test;
#[path = "ports/testkit__orchestrator_replay_recovery_integration_test.rs"]
mod testkit_orchestrator_replay_recovery_integration_test;
#[path = "ports/testkit__orchestrator_replay_restart_background_note_integration_test.rs"]
mod testkit_orchestrator_replay_restart_background_note_integration_test;
#[path = "ports/testkit__pi_replay_fixtures_integration_test.rs"]
mod testkit_pi_replay_fixtures_integration_test;
#[path = "ports/testkit__provider_replay_harness_test.rs"]
mod testkit_provider_replay_harness_test;
#[path = "ports/testkit__provider_switch_integration_test.rs"]
mod testkit_provider_switch_integration_test;
#[path = "ports/testkit__thread_fork_integration_test.rs"]
mod testkit_thread_fork_integration_test;
#[path = "ports/testkit__thread_merge_back_integration_test.rs"]
mod testkit_thread_merge_back_integration_test;
#[path = "ports/thread_deletion_test.rs"]
mod thread_deletion_test;
#[path = "ports/thread_find_test.rs"]
mod thread_find_test;
#[path = "ports/thread_fork_execution_test.rs"]
mod thread_fork_execution_test;
#[path = "ports/thread_fork_service_test.rs"]
mod thread_fork_service_test;
#[path = "ports/thread_history_paging_test.rs"]
mod thread_history_paging_test;
#[path = "ports/thread_launch_service_test.rs"]
mod thread_launch_service_test;
#[path = "ports/thread_live_event_coalescer_test.rs"]
mod thread_live_event_coalescer_test;
#[path = "ports/thread_management_service_test.rs"]
mod thread_management_service_test;
#[path = "ports/thread_message_intake_test.rs"]
mod thread_message_intake_test;
#[path = "ports/thread_pull_request_service_test.rs"]
mod thread_pull_request_service_test;
#[path = "ports/thread_search_test.rs"]
mod thread_search_test;
#[path = "ports/thread_settlement_service_test.rs"]
mod thread_settlement_service_test;
#[path = "ports/thread_stop_test.rs"]
mod thread_stop_test;
#[path = "ports/thread_stream_test.rs"]
mod thread_stream_test;
#[path = "ports/thread_title_regeneration_service_test.rs"]
mod thread_title_regeneration_service_test;
#[path = "ports/thread_transport_performance_test.rs"]
mod thread_transport_performance_test;
#[path = "ports/tool_output_image_bytes_test.rs"]
mod tool_output_image_bytes_test;
#[path = "ports/turn_item_position_store_test.rs"]
mod turn_item_position_store_test;
#[path = "ports/turn_start_reads_test.rs"]
mod turn_start_reads_test;
#[path = "ports/usage_limit_recovery_worker_test.rs"]
mod usage_limit_recovery_worker_test;
#[path = "ports/user_facing_errors_test.rs"]
mod user_facing_errors_test;
#[path = "ports/wire_projection_test.rs"]
mod wire_projection_test;
#[path = "ports/workflow_agent_answers_test.rs"]
mod workflow_agent_answers_test;
#[path = "ports/workflow_agent_transcript_test.rs"]
mod workflow_agent_transcript_test;
#[path = "ports/workflow_script_query_test.rs"]
mod workflow_script_query_test;
#[path = "ports/worktree_branch_sync_test.rs"]
mod worktree_branch_sync_test;
