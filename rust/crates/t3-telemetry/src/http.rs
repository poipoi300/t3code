//! Outbound HTTP the way Node's fetch does it, so existing deployments keep working: ambient
//! `HTTP(S)_PROXY`/`ALL_PROXY` variables are ignored, and the bundled Mozilla roots are trusted
//! plus every certificate in the PEM file `NODE_EXTRA_CA_CERTS` names. As in Node, an unreadable
//! file adds nothing rather than failing.

fn extra_roots() -> Vec<reqwest::Certificate> {
    std::env::var_os("NODE_EXTRA_CA_CERTS")
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|pem| reqwest::Certificate::from_pem_bundle(&pem).ok())
        .unwrap_or_default()
}

pub(crate) fn client_builder() -> reqwest::ClientBuilder {
    extra_roots()
        .into_iter()
        .fold(reqwest::Client::builder().no_proxy(), |builder, root| {
            builder.add_root_certificate(root)
        })
}

pub(crate) fn blocking_client_builder() -> reqwest::blocking::ClientBuilder {
    extra_roots().into_iter().fold(
        reqwest::blocking::Client::builder().no_proxy(),
        |builder, root| builder.add_root_certificate(root),
    )
}
