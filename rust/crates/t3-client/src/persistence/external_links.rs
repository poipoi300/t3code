//! External opener policy; provider-auth decoding applies its own wire limits.
pub fn is_web_url(address: &str) -> bool {
    url::Url::parse(address).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}
