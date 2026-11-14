fn tool_definition(name: &str) -> Option<ToolDefinition> {
    match name {
        "t3_theme_read" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "themes"],
            icon: "t3-code",
            action: "theme-read",
        }),
        "t3_theme_set" => Some(ToolDefinition {
            labels: ["Set", "Setting", "Set", "the theme"],
            icon: "t3-code",
            action: "theme-set",
        }),
        "link_pull_request" => Some(ToolDefinition {
            labels: ["Link", "Linking", "Linked", "a pull request"],
            icon: "pull-request",
            action: "link-pr",
        }),
        "link_pull_requests" => Some(ToolDefinition {
            labels: ["Link", "Linking", "Linked", "pull requests"],
            icon: "pull-request",
            action: "link-pr",
        }),
        "unlink_pull_request" => Some(ToolDefinition {
            labels: ["Unlink", "Unlinking", "Unlinked", "a pull request"],
            icon: "pull-request",
            action: "unlink-pr",
        }),
        "unlink_pull_requests" => Some(ToolDefinition {
            labels: ["Unlink", "Unlinking", "Unlinked", "pull requests by state"],
            icon: "pull-request",
            action: "unlink-pr",
        }),
        "list_thread_pull_requests" => Some(ToolDefinition {
            labels: ["Check", "Checking", "Checked", "linked pull requests"],
            icon: "pull-request",
            action: "list-prs",
        }),
        "watch_pull_request" => Some(ToolDefinition {
            labels: ["Watch", "Watching", "Watching", "a pull request"],
            icon: "pull-request",
            action: "watch-pr",
        }),
        "unwatch_pull_request" => Some(ToolDefinition {
            labels: [
                "Stop watching",
                "Unwatching",
                "Stopped watching",
                "a pull request",
            ],
            icon: "pull-request",
            action: "unwatch-pr",
        }),
        "write_pull_request_guides" => Some(ToolDefinition {
            labels: ["Write", "Writing", "Started writing", "review guides"],
            icon: "pull-request",
            action: "write-pr-guides",
        }),
        "read_pull_request_guide" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "a review guide"],
            icon: "pull-request",
            action: "read-pr-guide",
        }),
        "orchestrator_capabilities" => Some(ToolDefinition {
            labels: ["Get", "Getting", "Got", "orchestration capabilities"],
            icon: "t3-code",
            action: "capabilities",
        }),
        "delegate_task" => Some(ToolDefinition {
            labels: ["Delegate", "Delegating", "Delegated", "a child task"],
            icon: "t3-code",
            action: "delegate",
        }),
        "task_status" => Some(ToolDefinition {
            labels: ["Get", "Getting", "Got", "delegated task status"],
            icon: "t3-code",
            action: "task-status",
        }),
        "task_cancel" => Some(ToolDefinition {
            labels: [
                "Cancel",
                "Canceling",
                "Requested cancellation of",
                "a delegated task",
            ],
            icon: "t3-code",
            action: "task-cancel",
        }),
        "schedule_task" => Some(ToolDefinition {
            labels: ["Schedule", "Scheduling", "Scheduled", "a task"],
            icon: "t3-code",
            action: "schedule-create",
        }),
        "list_scheduled_tasks" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "scheduled tasks"],
            icon: "t3-code",
            action: "schedule-list",
        }),
        "update_scheduled_task" => Some(ToolDefinition {
            labels: ["Update", "Updating", "Updated", "a scheduled task"],
            icon: "t3-code",
            action: "schedule-update",
        }),
        "delete_scheduled_task" => Some(ToolDefinition {
            labels: [
                "Delete",
                "Deleting",
                "Requested deletion of",
                "a scheduled task",
            ],
            icon: "t3-code",
            action: "schedule-delete",
        }),
        "request_secret" => Some(ToolDefinition {
            labels: ["Ask for", "Asking for", "Asked for", "a secret"],
            icon: "t3-code",
            action: "secret-request",
        }),
        "create_threads" => Some(ToolDefinition {
            labels: ["Create", "Creating", "Created", "T3 threads"],
            icon: "t3-code",
            action: "thread-create",
        }),
        "t3_thread_start" => Some(ToolDefinition {
            labels: ["Start", "Starting", "Started", "a T3 thread"],
            icon: "t3-code",
            action: "thread-create",
        }),
        "t3_thread_list" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "T3 threads"],
            icon: "t3-code",
            action: "thread-list",
        }),
        "t3_thread_read" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "a T3 thread"],
            icon: "t3-code",
            action: "thread-read",
        }),
        "t3_thread_send" => Some(ToolDefinition {
            labels: ["Send", "Sending", "Sent", "to a T3 thread"],
            icon: "t3-code",
            action: "thread-send",
        }),
        "t3_thread_wait" => Some(ToolDefinition {
            labels: ["Wait", "Waiting", "Waited", "for a T3 thread"],
            icon: "t3-code",
            action: "thread-wait",
        }),
        "t3_workflow_stop" => Some(ToolDefinition {
            labels: ["Stop", "Stopping", "Requested a stop for", "a workflow"],
            icon: "t3-code",
            action: "workflow-stop",
        }),
        "t3_background_wait" => Some(ToolDefinition {
            labels: ["Update", "Updating", "Updated", "background waits"],
            icon: "t3-code",
            action: "background-wait",
        }),
        "t3_thread_interrupt" => Some(ToolDefinition {
            labels: [
                "Interrupt",
                "Interrupting",
                "Requested an interrupt of",
                "a T3 thread",
            ],
            icon: "t3-code",
            action: "thread-interrupt",
        }),
        "t3_worktree_handoff" => Some(ToolDefinition {
            labels: [
                "Hand off",
                "Handing off",
                "Handed off",
                "thread to a git worktree",
            ],
            icon: "t3-code",
            action: "worktree-handoff",
        }),
        "t3_worktree_status" => Some(ToolDefinition {
            labels: ["Get", "Getting", "Got", "thread worktree status"],
            icon: "t3-code",
            action: "worktree-status",
        }),
        "preview_status" => Some(ToolDefinition {
            labels: ["Get", "Getting", "Got", "preview browser status"],
            icon: "browser",
            action: "browser",
        }),
        "preview_open" => Some(ToolDefinition {
            labels: ["Open", "Opening", "Opened", "a page in the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_navigate" => Some(ToolDefinition {
            labels: ["Navigate", "Navigating", "Navigated", "the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_dialog" => Some(ToolDefinition {
            labels: [
                "Respond",
                "Responding",
                "Responded",
                "to a preview browser dialog",
            ],
            icon: "browser",
            action: "browser",
        }),
        "preview_snapshot" => Some(ToolDefinition {
            labels: [
                "Take a snapshot of",
                "Taking a snapshot of",
                "Took a snapshot of",
                "the preview page",
            ],
            icon: "browser",
            action: "browser",
        }),
        "preview_act" => Some(ToolDefinition {
            labels: ["Use", "Using", "Used", "the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_click" => Some(ToolDefinition {
            labels: ["Click", "Clicking", "Clicked", "in the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_press" => Some(ToolDefinition {
            labels: [
                "Press",
                "Pressing",
                "Pressed",
                "a key in the preview browser",
            ],
            icon: "browser",
            action: "browser",
        }),
        "preview_type" => Some(ToolDefinition {
            labels: ["Type", "Typing", "Typed", "in the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_hover" => Some(ToolDefinition {
            labels: ["Hover", "Hovering", "Hovered", "in the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_select" => Some(ToolDefinition {
            labels: [
                "Choose",
                "Choosing",
                "Chose",
                "an option in the preview browser",
            ],
            icon: "browser",
            action: "browser",
        }),
        "preview_drag" => Some(ToolDefinition {
            labels: ["Drag", "Dragging", "Dragged", "in the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_upload" => Some(ToolDefinition {
            labels: [
                "Upload",
                "Uploading",
                "Uploaded",
                "files to the preview browser",
            ],
            icon: "browser",
            action: "browser",
        }),
        "preview_scroll" => Some(ToolDefinition {
            labels: ["Scroll", "Scrolling", "Scrolled", "the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_resize" => Some(ToolDefinition {
            labels: ["Resize", "Resizing", "Resized", "the preview browser"],
            icon: "browser",
            action: "browser",
        }),
        "preview_evaluate" => Some(ToolDefinition {
            labels: [
                "Evaluate",
                "Evaluating",
                "Evaluated",
                "script in the preview browser",
            ],
            icon: "browser",
            action: "browser",
        }),
        "preview_wait_for" => Some(ToolDefinition {
            labels: ["Wait", "Waiting", "Waited", "for the preview page"],
            icon: "browser",
            action: "browser",
        }),
        "preview_set_appearance" => Some(ToolDefinition {
            labels: ["Set", "Setting", "Set", "preview browser appearance"],
            icon: "browser",
            action: "browser",
        }),
        "preview_recording_start" => Some(ToolDefinition {
            labels: [
                "Start",
                "Starting",
                "Started",
                "recording the preview browser",
            ],
            icon: "browser",
            action: "browser",
        }),
        "preview_recording_stop" => Some(ToolDefinition {
            labels: [
                "Stop",
                "Stopping",
                "Stopped",
                "recording the preview browser",
            ],
            icon: "browser",
            action: "browser",
        }),
        "device_list" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "simulators and emulators"],
            icon: "device",
            action: "device",
        }),
        "device_open" => Some(ToolDefinition {
            labels: ["Open", "Opening", "Opened", "a device in the Device panel"],
            icon: "device",
            action: "device",
        }),
        "device_screenshot" => Some(ToolDefinition {
            labels: [
                "Take a screenshot of",
                "Taking a screenshot of",
                "Took a screenshot of",
                "the device",
            ],
            icon: "device",
            action: "device",
        }),
        "device_close" => Some(ToolDefinition {
            labels: ["Close", "Closing", "Closed", "a device"],
            icon: "device",
            action: "device",
        }),
        "run_scheduled_task_now" => Some(ToolDefinition {
            labels: ["Run", "Running", "Requested a run of", "a scheduled task"],
            icon: "t3-code",
            action: "schedule-run",
        }),
        "t3_queue" => Some(ToolDefinition {
            labels: ["Manage", "Managing", "Managed", "queued messages"],
            icon: "t3-code",
            action: "queue-edit",
        }),
        "t3_pending_request" => Some(ToolDefinition {
            labels: ["Handle", "Handling", "Handled", "pending questions"],
            icon: "t3-code",
            action: "question-respond",
        }),
        "t3_queue_list" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "queued messages"],
            icon: "t3-code",
            action: "queue-list",
        }),
        "t3_queue_read" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "a queued message"],
            icon: "t3-code",
            action: "queue-read",
        }),
        "t3_queue_edit" => Some(ToolDefinition {
            labels: ["Edit", "Editing", "Edited", "a queued message"],
            icon: "t3-code",
            action: "queue-edit",
        }),
        "t3_queue_cancel" => Some(ToolDefinition {
            labels: [
                "Cancel",
                "Canceling",
                "Requested cancellation of",
                "a queued run",
            ],
            icon: "t3-code",
            action: "queue-cancel",
        }),
        "t3_queue_reorder" => Some(ToolDefinition {
            labels: ["Reorder", "Reordering", "Reordered", "a queued run"],
            icon: "t3-code",
            action: "queue-reorder",
        }),
        "t3_queue_promote_to_steer" => Some(ToolDefinition {
            labels: [
                "Steer with",
                "Steering with",
                "Requested steering with",
                "a queued message",
            ],
            icon: "t3-code",
            action: "queue-steer",
        }),
        "t3_pending_request_list" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "pending questions"],
            icon: "t3-code",
            action: "question-list",
        }),
        "t3_pending_request_read" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "pending questions"],
            icon: "t3-code",
            action: "question-read",
        }),
        "t3_pending_request_respond" => Some(ToolDefinition {
            labels: ["Answer", "Answering", "Answered", "pending questions"],
            icon: "t3-code",
            action: "question-respond",
        }),
        "t3_thread_configuration" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "thread configuration"],
            icon: "t3-code",
            action: "thread-configuration",
        }),
        "t3_thread_configure" => Some(ToolDefinition {
            labels: ["Use", "Using", "Used", "thread model settings"],
            icon: "t3-code",
            action: "thread-configure",
        }),
        "t3_thread_transfer" => Some(ToolDefinition {
            labels: ["Use", "Using", "Used", "thread transfers"],
            icon: "t3-code",
            action: "thread-fork",
        }),
        "t3_thread_fork" => Some(ToolDefinition {
            labels: ["Fork", "Forking", "Requested a fork of", "this thread"],
            icon: "t3-code",
            action: "thread-fork",
        }),
        "t3_thread_merge_back" => Some(ToolDefinition {
            labels: ["Merge", "Merging", "Requested a merge of", "thread context"],
            icon: "t3-code",
            action: "thread-merge",
        }),
        "t3_thread_search" => Some(ToolDefinition {
            labels: ["Search", "Searching", "Searched", "thread content"],
            icon: "t3-code",
            action: "thread-search",
        }),
        "t3_thread_transfers" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "thread transfers"],
            icon: "t3-code",
            action: "thread-transfers",
        }),
        "t3_thread_organize" => Some(ToolDefinition {
            labels: ["Organize", "Organizing", "Organized", "a thread"],
            icon: "t3-code",
            action: "thread-organize",
        }),
        "t3_thread_update" => Some(ToolDefinition {
            labels: ["Update", "Updating", "Updated", "T3 thread metadata"],
            icon: "t3-code",
            action: "thread-update",
        }),
        "t3_worktree_list" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "workspace branches"],
            icon: "t3-code",
            action: "worktree-list",
        }),
        "t3_preview_list" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "preview tabs"],
            icon: "browser",
            action: "browser",
        }),
        "t3_preview_close" => Some(ToolDefinition {
            labels: ["Close", "Closing", "Closed", "a preview tab"],
            icon: "browser",
            action: "browser",
        }),
        "t3_environment_read" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "environment preferences"],
            icon: "t3-code",
            action: "environment-read",
        }),
        "t3_environment_links" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "linked environments"],
            icon: "t3-code",
            action: "environment-links",
        }),
        "t3_environment_link" => Some(ToolDefinition {
            labels: [
                "Ask to link",
                "Asking to link",
                "Asked to link",
                "an environment",
            ],
            icon: "t3-code",
            action: "environment-link",
        }),
        "t3_environment_unlink" => Some(ToolDefinition {
            labels: ["Forget", "Forgetting", "Forgot", "a linked environment"],
            icon: "t3-code",
            action: "environment-unlink",
        }),
        "t3_environment_preferences_update" => Some(ToolDefinition {
            labels: ["Update", "Updating", "Updated", "environment preferences"],
            icon: "t3-code",
            action: "environment-update",
        }),
        "t3_thread_launch" => Some(ToolDefinition {
            labels: ["Launch", "Launching", "Launched", "a project thread"],
            icon: "t3-code",
            action: "thread-create",
        }),
        "t3_project_list" => Some(ToolDefinition {
            labels: ["List", "Listing", "Listed", "projects"],
            icon: "t3-code",
            action: "project-list",
        }),
        "t3_project_read" => Some(ToolDefinition {
            labels: ["Read", "Reading", "Read", "a project"],
            icon: "t3-code",
            action: "project-read",
        }),
        "t3_project_create" => Some(ToolDefinition {
            labels: ["Register", "Registering", "Registered", "a project"],
            icon: "t3-code",
            action: "project-create",
        }),
        "t3_project_update" => Some(ToolDefinition {
            labels: ["Update", "Updating", "Updated", "a project"],
            icon: "t3-code",
            action: "project-update",
        }),
        "t3_project_delete" => Some(ToolDefinition {
            labels: ["Delete", "Deleting", "Deleted", "a project"],
            icon: "t3-code",
            action: "project-delete",
        }),
        "t3_project_clone" => Some(ToolDefinition {
            labels: ["Clone", "Cloning", "Cloned", "a repository"],
            icon: "t3-code",
            action: "project-clone",
        }),
        "t3_attachment_prepare_upload" => Some(ToolDefinition {
            labels: ["Prepare", "Preparing", "Prepared", "an attachment upload"],
            icon: "t3-code",
            action: "attachment-prepare",
        }),
        "t3_attachment_discard" => Some(ToolDefinition {
            labels: ["Discard", "Discarding", "Discarded", "a pending attachment"],
            icon: "t3-code",
            action: "attachment-discard",
        }),
        "t3_thread_send_attachments" => Some(ToolDefinition {
            labels: ["Send", "Sending", "Sent", "attachments"],
            icon: "t3-code",
            action: "attachment-send",
        }),
        "html_preview" => Some(ToolDefinition {
            labels: ["Preview", "Previewing", "Previewed", "an HTML page"],
            icon: "t3-code",
            action: "html-preview",
        }),
        "html_render" => Some(ToolDefinition {
            labels: ["Render", "Rendering", "Rendered", "an HTML page"],
            icon: "t3-code",
            action: "html-render",
        }),
        "send_to_user" => Some(ToolDefinition {
            labels: ["Send", "Sending", "Sent", "a message"],
            icon: "t3-code",
            action: "send-to-user",
        }),
        _ => None,
    }
}
