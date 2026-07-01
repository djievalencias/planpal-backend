/// Compile-time secret backend selection.
///
/// Reads the `SECRET_SOURCE` environment variable at **build time** and emits
/// a `rustc-cfg` flag so the rest of the crate can use `#[cfg(secret_source = "…")]`
/// to pick a concrete `ActiveSecretManager` type with zero runtime overhead.
///
/// Usage:
///   SECRET_SOURCE=aws_secret_manager  cargo build
///   SECRET_SOURCE=vault               cargo build
///   SECRET_SOURCE=env                 cargo build   ← default (no external deps)
fn main() {
    // Declare all valid values so rustc's check-cfg lint doesn't warn about
    // unknown cfg condition names when we use #[cfg(secret_source = "…")].
    println!(r#"cargo::rustc-check-cfg=cfg(secret_source, values("aws", "vault", "env"))"#);

    // ── SECRET_SOURCE ─────────────────────────────────────────────────────────
    let source = std::env::var("SECRET_SOURCE").unwrap_or_default();

    let cfg_val = match source.to_lowercase().replace('-', "_").as_str() {
        "aws_secret_manager" | "aws" => "aws",
        "vault" => "vault",
        _ => "env",
    };

    println!("cargo:rustc-cfg=secret_source=\"{cfg_val}\"");
    println!("cargo:rerun-if-env-changed=SECRET_SOURCE");

}
