use super::Registry;

mod acp_registry;
mod agent_sessions;
mod application_event;
mod assets;
mod assistant_citations;
mod auth;
mod background;
mod base_schemas;
mod browser_import;
mod browser_profile;
mod chat_attachment;
mod checkpoint_diff;
mod client_rpc_permissions;
mod composer_context;
mod composer_context_clipboard;
mod desktop_app_activation;
mod desktop_bootstrap;
mod desktop_browser;
mod device;
mod editor;
mod environment;
mod environment_http;
mod filesystem;
mod git;
mod guest_share;
mod guest_stream;
mod index;
mod ipc;
mod keybindings;
mod managed_policy;
mod mcp_apps;
mod model;
mod model_selection;
mod orchestration_dispatch;
mod orchestration_project;
mod orchestration_v2;
mod orchestration_v2_id_table;
mod orchestrator_mcp;
mod peer_link;
mod preview;
mod preview_automation;
mod preview_capture;
mod project;
mod project_clone;
mod provider;
mod provider_instance;
mod provider_policy;
mod provider_runtime;
mod provider_setup;
mod provider_usage_limits;
mod pull_request;
mod pull_request_review_guide;
mod relay;
mod relay_client;
mod remote_access;
mod resource_telemetry;
mod review;
mod rpc;
mod scheduled_task;
mod secret_reference;
mod secret_request;
mod server;
mod settings;
mod settings_secret_value;
mod source_control;
mod speech;
mod t3_project_file;
mod terminal;
mod thread_metadata_mcp;
mod thread_pull_request;
mod thread_search;
mod thread_tags;
mod thread_title;
mod usage;
mod usage_limit_source_id;
mod vcs;
mod worktree_mcp;
mod worktree_setup;

pub const MODULES: &[&str] = &[
    "acp_registry",
    "agent_sessions",
    "application_event",
    "assets",
    "assistant_citations",
    "auth",
    "background",
    "base_schemas",
    "browser_import",
    "browser_profile",
    "chat_attachment",
    "checkpoint_diff",
    "client_rpc_permissions",
    "composer_context",
    "composer_context_clipboard",
    "desktop_app_activation",
    "desktop_bootstrap",
    "desktop_browser",
    "device",
    "editor",
    "environment",
    "environment_http",
    "filesystem",
    "git",
    "guest_share",
    "guest_stream",
    "index",
    "ipc",
    "keybindings",
    "managed_policy",
    "mcp_apps",
    "model",
    "model_selection",
    "orchestration_dispatch",
    "orchestration_project",
    "orchestration_v2",
    "orchestration_v2_id_table",
    "orchestrator_mcp",
    "peer_link",
    "preview",
    "preview_automation",
    "preview_capture",
    "project",
    "project_clone",
    "provider",
    "provider_instance",
    "provider_policy",
    "provider_runtime",
    "provider_setup",
    "provider_usage_limits",
    "pull_request",
    "pull_request_review_guide",
    "relay",
    "relay_client",
    "remote_access",
    "resource_telemetry",
    "review",
    "rpc",
    "scheduled_task",
    "secret_reference",
    "secret_request",
    "server",
    "settings",
    "settings_secret_value",
    "source_control",
    "speech",
    "t3_project_file",
    "terminal",
    "thread_metadata_mcp",
    "thread_pull_request",
    "thread_search",
    "thread_tags",
    "thread_title",
    "usage",
    "usage_limit_source_id",
    "vcs",
    "worktree_mcp",
    "worktree_setup",
];

pub fn registry() -> Registry {
    let mut registry = Registry::new();
    acp_registry::register_module(&mut registry);
    agent_sessions::register_module(&mut registry);
    application_event::register_module(&mut registry);
    assets::register_module(&mut registry);
    assistant_citations::register_module(&mut registry);
    auth::register_module(&mut registry);
    background::register_module(&mut registry);
    base_schemas::register_module(&mut registry);
    browser_import::register_module(&mut registry);
    browser_profile::register_module(&mut registry);
    chat_attachment::register_module(&mut registry);
    checkpoint_diff::register_module(&mut registry);
    client_rpc_permissions::register_module(&mut registry);
    composer_context::register_module(&mut registry);
    composer_context_clipboard::register_module(&mut registry);
    desktop_app_activation::register_module(&mut registry);
    desktop_bootstrap::register_module(&mut registry);
    desktop_browser::register_module(&mut registry);
    device::register_module(&mut registry);
    editor::register_module(&mut registry);
    environment::register_module(&mut registry);
    environment_http::register_module(&mut registry);
    filesystem::register_module(&mut registry);
    git::register_module(&mut registry);
    guest_share::register_module(&mut registry);
    guest_stream::register_module(&mut registry);
    index::register_module(&mut registry);
    ipc::register_module(&mut registry);
    keybindings::register_module(&mut registry);
    managed_policy::register_module(&mut registry);
    mcp_apps::register_module(&mut registry);
    model::register_module(&mut registry);
    model_selection::register_module(&mut registry);
    orchestration_dispatch::register_module(&mut registry);
    orchestration_project::register_module(&mut registry);
    orchestration_v2::register_module(&mut registry);
    orchestration_v2_id_table::register_module(&mut registry);
    orchestrator_mcp::register_module(&mut registry);
    peer_link::register_module(&mut registry);
    preview::register_module(&mut registry);
    preview_automation::register_module(&mut registry);
    preview_capture::register_module(&mut registry);
    project::register_module(&mut registry);
    project_clone::register_module(&mut registry);
    provider::register_module(&mut registry);
    provider_instance::register_module(&mut registry);
    provider_policy::register_module(&mut registry);
    provider_runtime::register_module(&mut registry);
    provider_setup::register_module(&mut registry);
    provider_usage_limits::register_module(&mut registry);
    pull_request::register_module(&mut registry);
    pull_request_review_guide::register_module(&mut registry);
    relay::register_module(&mut registry);
    relay_client::register_module(&mut registry);
    remote_access::register_module(&mut registry);
    resource_telemetry::register_module(&mut registry);
    review::register_module(&mut registry);
    rpc::register_module(&mut registry);
    scheduled_task::register_module(&mut registry);
    secret_reference::register_module(&mut registry);
    secret_request::register_module(&mut registry);
    server::register_module(&mut registry);
    settings::register_module(&mut registry);
    settings_secret_value::register_module(&mut registry);
    source_control::register_module(&mut registry);
    speech::register_module(&mut registry);
    t3_project_file::register_module(&mut registry);
    terminal::register_module(&mut registry);
    thread_metadata_mcp::register_module(&mut registry);
    thread_pull_request::register_module(&mut registry);
    thread_search::register_module(&mut registry);
    thread_tags::register_module(&mut registry);
    thread_title::register_module(&mut registry);
    usage::register_module(&mut registry);
    usage_limit_source_id::register_module(&mut registry);
    vcs::register_module(&mut registry);
    worktree_mcp::register_module(&mut registry);
    worktree_setup::register_module(&mut registry);
    registry
}
