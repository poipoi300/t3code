CREATE INDEX IF NOT EXISTS orchestration_v2_projection_threads_worktree_path_idx
    ON orchestration_v2_projection_threads(
      CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.worktreePath') END
    );

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_threads_root_thread_idx
    ON orchestration_v2_projection_threads(
      CASE WHEN json_valid(payload_json) THEN json_extract(payload_json, '$.lineage.rootThreadId') END
    );

CREATE INDEX IF NOT EXISTS orchestration_v2_projection_runs_run_status_idx
    ON orchestration_v2_projection_runs(run_id, status);
