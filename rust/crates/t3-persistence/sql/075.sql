ALTER TABLE orchestration_v2_projection_subagents ADD COLUMN completion_delivery_state TEXT;

UPDATE orchestration_v2_projection_subagents
    SET completion_delivery_state = json_extract(payload_json, '$.completionDelivery.state')
    WHERE json_type(payload_json, '$.completionDelivery') IS NOT NULL;

CREATE INDEX orchestration_v2_projection_subagents_settled_idx
    ON orchestration_v2_projection_subagents(thread_id, status, completion_delivery_state, run_id);
