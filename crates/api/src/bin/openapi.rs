//! Prints the OpenAPI document; `make gen-api` turns it into TypeScript types.

fn main() {
    println!(
        "{}",
        clipos_api::openapi::spec()
            .to_pretty_json()
            .expect("serializing OpenAPI spec")
    );
}
