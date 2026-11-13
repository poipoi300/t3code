UPDATE orchestration_v2_projection_threads
    SET payload_json = json_set(payload_json, '$.settledOverride',
      IIF(json_type(payload_json, '$.settledOverride') = 'true', 'settled', NULL))
    WHERE json_valid(payload_json)
      AND json_type(payload_json, '$.settledOverride') IN ('true', 'false');
