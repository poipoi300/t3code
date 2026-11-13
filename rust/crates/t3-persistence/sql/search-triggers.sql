CREATE TRIGGER orchestration_v2_message_search_insert
      AFTER INSERT ON orchestration_v2_projection_messages
      WHEN 
  
  new.streaming = 0
  AND (
    new.role = 'assistant'
    OR (
      new.role = 'user'
      AND COALESCE(json_extract(new.payload_json, '$.createdBy'), 'user') = 'user'
    )
  )
  AND NOT EXISTS (
    SELECT 1 FROM orchestration_v2_projection_threads AS subagent_threads
    WHERE subagent_threads.thread_id = new.thread_id
      AND json_extract(subagent_threads.payload_json, '$.lineage.relationshipToParent') = 'subagent'
  )

  AND 
  CASE
    WHEN substr(json_extract(new.payload_json, '$.text'), 1, 11) = 'Ultrathink:' COLLATE NOCASE
      THEN ltrim(substr(json_extract(new.payload_json, '$.text'), 12), '	
                  　﻿')
    ELSE json_extract(new.payload_json, '$.text')
  END COLLATE BINARY <> '[User attached one or more files without additional text. Respond using the conversation context and the attached files.]'


      BEGIN
        INSERT INTO orchestration_v2_message_search (rowid, text)
        SELECT new.rowid, replace(replace(COALESCE(new.search_text, json_extract(new.payload_json, '$.text')), '’', ''''), '‘', '''')
        WHERE json_extract(new.payload_json, '$.text') IS NOT NULL;
      END;

CREATE TRIGGER orchestration_v2_message_search_update
      AFTER UPDATE ON orchestration_v2_projection_messages
      WHEN (old.streaming = 0 OR new.streaming = 0)
        AND (
          old.streaming IS NOT new.streaming
          OR old.role IS NOT new.role
          OR replace(replace(COALESCE(old.search_text, json_extract(old.payload_json, '$.text')), '’', ''''), '‘', '''') IS NOT replace(replace(COALESCE(new.search_text, json_extract(new.payload_json, '$.text')), '’', ''''), '‘', '''')
          OR json_extract(old.payload_json, '$.createdBy')
            IS NOT json_extract(new.payload_json, '$.createdBy')
        )
      BEGIN
        DELETE FROM orchestration_v2_message_search WHERE rowid = old.rowid AND old.streaming = 0;
        INSERT INTO orchestration_v2_message_search (rowid, text)
        SELECT new.rowid, replace(replace(COALESCE(new.search_text, json_extract(new.payload_json, '$.text')), '’', ''''), '‘', '''')
        WHERE 
  
  new.streaming = 0
  AND (
    new.role = 'assistant'
    OR (
      new.role = 'user'
      AND COALESCE(json_extract(new.payload_json, '$.createdBy'), 'user') = 'user'
    )
  )
  AND NOT EXISTS (
    SELECT 1 FROM orchestration_v2_projection_threads AS subagent_threads
    WHERE subagent_threads.thread_id = new.thread_id
      AND json_extract(subagent_threads.payload_json, '$.lineage.relationshipToParent') = 'subagent'
  )

  AND 
  CASE
    WHEN substr(json_extract(new.payload_json, '$.text'), 1, 11) = 'Ultrathink:' COLLATE NOCASE
      THEN ltrim(substr(json_extract(new.payload_json, '$.text'), 12), '	
                  　﻿')
    ELSE json_extract(new.payload_json, '$.text')
  END COLLATE BINARY <> '[User attached one or more files without additional text. Respond using the conversation context and the attached files.]'


          AND json_extract(new.payload_json, '$.text') IS NOT NULL;
      END;

CREATE TRIGGER orchestration_v2_message_search_delete
      AFTER DELETE ON orchestration_v2_projection_messages
      WHEN old.streaming = 0
      BEGIN
        DELETE FROM orchestration_v2_message_search WHERE rowid = old.rowid;
      END;
