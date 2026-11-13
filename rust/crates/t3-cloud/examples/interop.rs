use serde::Deserialize;
use serde_json::json;
use t3_cloud::{
    Secret,
    crypto::{self, EnvironmentKey},
};
use t3_contracts::relay::RelayCloudEnvironmentHealthProofPayload;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    now: i64,
    public_key: String,
    proof: Secret,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let directory = std::env::args_os()
        .nth(1)
        .ok_or("expected a temporary directory")?;
    let directory = std::path::Path::new(&directory);
    let input: Input = serde_json::from_slice(&std::fs::read(directory.join("typescript.json"))?)?;
    let request: RelayCloudEnvironmentHealthProofPayload = crypto::verify(
        &input.public_key,
        &input.proof,
        crypto::HEALTH_REQUEST,
        "https://interop.test",
        "t3-env:interop",
        input.now,
    )?;
    let key = EnvironmentKey::generate()?;
    let proof=key.sign(crypto::HEALTH_RESPONSE,&json!({"iss":"t3-env:interop","aud":"https://interop.test","sub":"interop","jti":"rust-response","iat":input.now,"exp":input.now+300,"environmentId":"interop","requestNonce":request.nonce,"status":"online"}))?;
    std::fs::write(
        directory.join("rust.json"),
        serde_json::to_vec(&json!({"publicKey":key.public_key,"proof":proof}))?,
    )?;
    println!("Rust verified the TypeScript request and signed a synthetic response.");
    Ok(())
}
