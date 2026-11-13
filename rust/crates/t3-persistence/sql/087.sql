UPDATE projection_projects
    SET has_custom_title = 1
    WHERE project_id IN (
      SELECT stream_id FROM orchestration_events
      WHERE aggregate_kind = 'project'
        AND event_type = 'project.meta-updated'
        AND json_type(payload_json, '$.title') = 'text'
    );
