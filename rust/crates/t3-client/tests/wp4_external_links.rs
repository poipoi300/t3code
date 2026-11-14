use futures::executor::block_on;
use t3_client::{
    platform::{HostReply, Platform},
    testing::FakePlatform,
};

#[test]
fn external_open_rejects_non_web_addresses_before_calling_the_host() {
    block_on(async {
        for address in [
            "javascript:alert(1)",
            "data:text/html,hello",
            "file:///private/document",
            "mailto:test@example.com",
            "//example.com",
            "/relative",
            "not a URL",
        ] {
            let platform = FakePlatform::default();
            platform.push_host(Ok(HostReply::Unit));
            assert!(
                platform.open_url(address.into()).await.is_err(),
                "{address}"
            );
            platform
                .open_url("https://example.com".into())
                .await
                .unwrap();
        }
        for address in [
            "https://example.com/sign-in",
            "http://localhost:3000",
            " HTTPS://example.com ",
        ] {
            let platform = FakePlatform::default();
            platform.push_host(Ok(HostReply::Unit));
            platform.open_url(address.into()).await.unwrap();
        }
    });
}
