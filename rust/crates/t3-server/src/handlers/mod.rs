pub mod cloud;
pub mod core;
pub mod device;
pub mod host;
pub mod optional_services;
pub mod orchestration;
pub mod preview;
pub mod projects;
pub mod providers;
pub mod source_control;
pub mod telemetry;
mod unsupported;
pub mod workspace_terminal;
use crate::{registry::HandlerRegistry, services::Domains};

/// The handler table with no domain service started.
pub fn registry() -> crate::Result<HandlerRegistry> {
    registry_with(&Domains::default())
}

/// The handler table for the domain services the composition root started. A module whose
/// service is absent registers its methods as unimplemented.
pub fn registry_with(domains: &Domains) -> crate::Result<HandlerRegistry> {
    let mut registry = HandlerRegistry::default();
    core::register(&mut registry)?;
    #[cfg(feature = "orchestration")]
    match domains.get::<crate::adapters::orchestration::OrchestrationAdapter>() {
        Some(adapter) => orchestration::register_with(&mut registry, adapter)?,
        None => orchestration::register_unavailable(&mut registry)?,
    }
    #[cfg(not(feature = "orchestration"))]
    orchestration::register_unavailable(&mut registry)?;
    workspace_terminal::register_with(&mut registry, domains)?;
    source_control::register(&mut registry)?;
    optional_services::register_with(&mut registry, domains)?;
    telemetry::register_with(&mut registry, domains)?;
    host::register_with(&mut registry, domains)?;
    device::register_with(&mut registry, domains)?;
    preview::register_with(&mut registry, domains)?;
    cloud::register_with(&mut registry, domains)?;
    projects::register_with(&mut registry, domains)?;
    providers::register_with(&mut registry, domains)?;
    unsupported::register(&mut registry)?;
    registry.validate()?;
    Ok(registry)
}
