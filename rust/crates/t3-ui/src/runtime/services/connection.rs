//! Connection and onboarding lane: the client's saved-environment operations behind the UI's
//! connection contracts. Pure delegation; rules live in `t3_client`.

use std::rc::Rc;

use futures::{FutureExt, future::LocalBoxFuture};
use t3_client::composition::ClientRuntime;
use t3_client::connection::ConnectionDiagnosis;
use t3_client::platform::Platform;
use t3_client::types::EnvironmentId;

use crate::i18n::Text;
use crate::screens::connect::ConnectionActions;

/// The app's version as the server-update check compares it; development builds have none.
const CLIENT_VERSION: Option<&str> = option_env!("T3_APP_VERSION");

struct Actions<P: Platform + Clone + 'static>(ClientRuntime<P>);

pub fn actions<P: Platform + Clone + 'static>(runtime: &ClientRuntime<P>) -> Rc<dyn ConnectionActions> {
    Rc::new(Actions(runtime.clone()))
}

fn error(error: t3_client::ClientError) -> Text {
    match error {
        t3_client::ClientError::Rejected { message, detail } => {
            Text::content(t3_client::error::rejection_text(&message, detail.as_deref()))
        }
        error => Text::client_key(error.to_string()),
    }
}

impl<P: Platform + Clone + 'static> ConnectionActions for Actions<P> {
    fn remove(&self, environment: &EnvironmentId) -> LocalBoxFuture<'static, Result<(), Text>> {
        let removal = self.0.remove(environment);
        async move { removal.await.map_err(error) }.boxed_local()
    }

    fn reconnect(&self, environment: &EnvironmentId) -> LocalBoxFuture<'static, Result<(), Text>> {
        let attempt = self.0.reconnect(environment);
        async move { attempt.await.map_err(error) }.boxed_local()
    }

    fn diagnosis(&self, environment: &EnvironmentId) -> Option<ConnectionDiagnosis> {
        self.0.diagnosis(environment, CLIENT_VERSION)
    }
}
