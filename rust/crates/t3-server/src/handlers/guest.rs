use crate::{registry::HandlerRegistry, services::Domains};
use t3_contracts::rpc::WsGuestShareCreateRpc;

pub fn register_with(registry: &mut HandlerRegistry, domains: &Domains) -> crate::Result<()> {
    #[cfg(all(feature = "cloud", feature = "orchestration"))]
    if let Some(guest) = crate::adapters::guest::GuestAdapter::from_domains(domains) {
        use crate::registry::Failure;
        use t3_cloud::{Error, guest::GuestFailure};
        use t3_contracts::{
            guest_share::{
                GuestShareCreateError, GuestShareCreateErrorReason as Reason,
                GuestShareCreateErrorTag,
            },
            rpc::WsGuestShareCreateError,
        };
        let failure = |reason| {
            Failure::Typed(WsGuestShareCreateError::GuestShareCreateError(Box::new(
                GuestShareCreateError {
                    _tag: GuestShareCreateErrorTag::GuestShareCreateError,
                    reason,
                },
            )))
        };
        return registry.register::<WsGuestShareCreateRpc, _, _>(move |ctx, input| {
            let guest = guest.clone();
            async move {
                ctx.services
                    .lifecycle
                    .admit_command()
                    .map_err(|_| failure(Reason::ThreadUnavailable))?;
                guest
                    .sharing
                    .create(input)
                    .await
                    .map(|result| result.into_inner())
                    .map_err(|error| {
                        failure(match error {
                            Error::Disabled => Reason::Disabled,
                            Error::NotLinked | Error::Configuration => Reason::ConnectUnavailable,
                            Error::Superseded => Reason::Changed,
                            Error::Guest(GuestFailure::Rejected) => Reason::ThreadUnavailable,
                            _ => Reason::ReadFailed,
                        })
                    })
            }
        });
    }
    let _ = domains;
    registry.unimplemented::<WsGuestShareCreateRpc>()
}
