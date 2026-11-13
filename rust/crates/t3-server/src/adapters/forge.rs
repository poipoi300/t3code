//! Forge primitives do not provide application repository, PR context, caching,
//! sign-in, watch or project-setup services. Those RPCs stay in `unsupported.rs`.
//!
//! GitWorkflowService needs a ReviewHost backed by an environment-owned registry,
//! resolved settings/secrets and authoritative checkout context before it can serve PRs.
pub use t3_forge::registry::ForgeRegistry;
