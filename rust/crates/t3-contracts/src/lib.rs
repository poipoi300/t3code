//! Wire and persistence types shared by the server and every client.
//!
//! Compiles to native and `wasm32-unknown-unknown`; keep it free of I/O.
//! See `PORTING.md` for wire conventions and per-module golden registration.
//! Values the TypeScript server persists (events, projections, settings) are
//! written with [`stored`], everything sent over RPC or HTTP with plain serde_json.

pub mod acp_registry;
pub mod agent_sessions;
pub mod application_event;
pub mod assets;
pub mod assistant_citations;
pub mod auth;
pub mod background;
pub mod base_schemas;
pub mod browser_import;
pub mod browser_profile;
pub mod chat_attachment;
pub mod checkpoint_diff;
pub mod client_rpc_permissions;
pub mod composer_context;
pub mod composer_context_clipboard;
pub mod desktop_app_activation;
pub mod desktop_bootstrap;
pub mod desktop_browser;
pub mod device;
pub mod editor;
pub mod environment;
pub mod environment_http;
pub mod filesystem;
pub mod git;
pub mod guest_share;
pub mod guest_stream;
pub mod index;
pub mod ipc;
pub mod keybindings;
pub mod managed_policy;
pub mod mcp_apps;
pub mod model;
pub mod model_selection;
pub mod orchestration_dispatch;
pub mod orchestration_project;
pub mod orchestration_v2;
pub mod orchestration_v2_id_table;
pub mod orchestrator_mcp;
pub mod peer_link;
pub mod preview;
pub mod preview_automation;
pub mod preview_capture;
pub mod project;
pub mod project_clone;
pub mod provider;
pub mod provider_instance;
pub mod provider_policy;
pub mod provider_runtime;
pub mod provider_setup;
pub mod provider_usage_limits;
pub mod pull_request;
pub mod pull_request_review_guide;
pub mod relay;
pub mod relay_client;
pub mod remote_access;
pub mod resource_telemetry;
pub mod review;
pub mod rpc;
pub mod scheduled_task;
pub mod secret_reference;
pub mod secret_request;
pub mod server;
pub mod settings;
pub mod settings_secret_value;
pub mod source_control;
pub mod speech;
pub mod stored;
pub mod t3_project_file;
pub mod terminal;
pub mod thread_metadata_mcp;
pub mod thread_pull_request;
pub mod thread_search;
pub mod thread_tags;
pub mod thread_title;
pub mod usage;
pub mod usage_limit_source_id;
pub mod vcs;
pub mod wire;
pub mod worktree_mcp;
pub mod worktree_setup;

// Preserve the original crate API; new types are imported from their owning module.
pub use auth::{AuthClientMetadata, AuthClientSession};
pub use background::BackgroundScope;
pub use base_schemas::{AuthSessionId, CommandId, ProjectId, ThreadId};
pub use provider_instance::ProviderInstanceId;
pub use wire::WireDateTime;
