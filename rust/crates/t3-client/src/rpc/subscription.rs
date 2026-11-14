//! Per-subscription recovery; the environment owner still replaces failed sessions.
use crate::{
    ClientError,
    platform::{Platform, PlatformError},
};
use futures::{StreamExt, stream::LocalBoxStream};
use std::rc::Rc;

pub const RPC_RESPONSE_SCHEMA_ERROR: &str = "RPC response schema";

pub type DefectHandler = Rc<dyn Fn(&ClientError)>;
#[derive(Clone, Default)]
pub struct SubscriptionOptions {
    pub retry_after_ms: Option<u64>,
    pub on_defect: Option<DefectHandler>,
}
pub fn subscription_retry_delay(base_ms: u64, retries: u32) -> u64 {
    base_ms
        .saturating_mul(2_u64.saturating_pow(retries))
        .min(30_000)
}
pub fn is_chunk_decode_error(error: &ClientError) -> bool {
    matches!(error, ClientError::InvalidPayload(message) if message == RPC_RESPONSE_SCHEMA_ERROR)
}
fn is_transport_error(error: &ClientError) -> bool {
    matches!(
        error,
        ClientError::Disconnected
            | ClientError::Platform(PlatformError::Network(_) | PlatformError::Closed)
    )
}

pub fn recover_subscription<'a, T: 'a, P: Platform + ?Sized>(
    platform: &'a P,
    method: &str,
    subscribe: impl FnMut() -> LocalBoxStream<'static, Result<T, ClientError>> + 'a,
    options: SubscriptionOptions,
) -> LocalBoxStream<'a, Result<T, ClientError>> {
    let finite = method == "orchestration.searchThreadStream";
    let initial = (
        subscribe,
        None::<LocalBoxStream<'static, Result<T, ClientError>>>,
        0_u32,
        false,
    );
    futures::stream::unfold(initial, move |(mut subscribe, mut stream, mut retries, ended)| {
        let options = options.clone();
        async move {
            if ended { return None; }
            loop {
                let active = stream.get_or_insert_with(&mut subscribe);
                match active.next().await? {
                    Ok(value) => return Some((Ok(value), (subscribe, stream, retries, false))),
                    Err(error) => {
                        let transport = is_transport_error(&error);
                        let decode = is_chunk_decode_error(&error);
                        let defect = decode || matches!(&error, ClientError::InvalidPayload(message) if message == "RPC handler defect");
                        if defect && let Some(handler) = &options.on_defect { handler(&error); }
                        stream = None;
                        if !finite && (transport || (decode && options.on_defect.is_none())) {
                            platform.sleep(subscription_retry_delay(options.retry_after_ms.unwrap_or(1000), retries)).await;
                            retries = retries.saturating_add(1);
                            continue;
                        }
                        if (finite && transport) || (defect && options.on_defect.is_some()) {
                            futures::future::pending::<()>().await;
                        }
                        return Some((Err(error), (subscribe, None, retries, true)));
                    }
                }
            }
        }
    }).boxed_local()
}
