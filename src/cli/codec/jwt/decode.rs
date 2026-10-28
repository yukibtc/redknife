use jsonwebtoken::TokenData;
use serde_json::Value;

pub(super) fn run(jwt: String) -> Result<(), Box<dyn std::error::Error>> {
    let data: TokenData<Value> = jsonwebtoken::dangerous::insecure_decode(jwt)?;

    let header: String = serde_json::to_string_pretty(&data.header)?;
    let payload: String = serde_json::to_string_pretty(&data.claims)?;

    println!("Header:\n{header}");
    println!();
    println!("Payload:\n{payload}");

    Ok(())
}
