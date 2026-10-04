# Vendored crates

## tiberius 0.13.0 (SQL Server / TDS)

This is [tiberius](https://github.com/prisma/tiberius) 0.13.0 from crates.io (MIT/Apache-2.0), with a small patch.
`tiberius.patch` holds the exact diff against the published crate.

1. **rustls on ring, not aws-lc-rs.** The published `rustls` feature hard-codes
   `rustls::crypto::aws_lc_rs` and pulls `tokio-rustls` with its default features, so aws-lc-sys
   (C/cmake) always got built. Now `tokio-rustls` has `default-features = false, features = ["ring", "logging", "tls12"]`
   and the fallback provider is `ring`.
2. **`QueryStream::rows_affected()`.** This returns the row counts from DONE tokens (those with the
   COUNT bit), in order. A script can then report "N rows affected" and still return its result sets.
3. **Exact MONEY.** `money`/`smallmoney` decode to `Numeric` (scale 4) instead of a lossy `f64`.

The `Cargo.toml` also leaves out tiberius's tests, examples and dev-dependencies. The workspace
excludes this directory, so `cargo test` and `clippy --workspace` don't build it.

To update: unpack the new crate here, apply `tiberius.patch` (`patch -p1 -d vendor/tiberius < vendor/tiberius.patch`,
fixing up conflicts), then regenerate the patch. Once upstream has a ring option and DONE counts,
drop the copy and depend on crates.io.
