CREATE INDEX orchestration_v2_projection_turn_items_queued_input_idx
    ON orchestration_v2_projection_turn_items(run_id)
    WHERE type = 'user_message' AND json_extract(payload_json, '$.inputIntent') = 'queued_turn';
