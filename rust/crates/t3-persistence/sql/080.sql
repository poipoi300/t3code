ALTER TABLE scheduled_tasks ADD COLUMN thread_mode TEXT NOT NULL DEFAULT 'new_thread';

UPDATE scheduled_tasks SET thread_mode = 'continue' WHERE thread_id IS NOT NULL;
