#![allow(clippy::disallowed_macros)]

use anyhow::Result;
use clap::Parser;
use duct::cmd;

/// Checks the portable datastore wasm boundary.
///
/// Ensures the table, datastore portable feature, portable datastore crate, and
/// wasm adapter compile for wasm32-unknown-unknown, then runs the portable
/// datastore unit tests natively.
#[derive(Parser)]
struct Cli {}

fn main() -> Result<()> {
    Cli::parse();

    cmd!(
        "cargo",
        "check",
        "-p",
        "spacetimedb-table",
        "--target",
        "wasm32-unknown-unknown"
    )
    .run()?;
    cmd!(
        "cargo",
        "check",
        "-p",
        "spacetimedb-datastore",
        "--no-default-features",
        "--features",
        "portable",
        "--target",
        "wasm32-unknown-unknown"
    )
    .run()?;
    cmd!(
        "cargo",
        "clippy",
        "-p",
        "spacetimedb-datastore",
        "--no-default-features",
        "--features",
        "portable",
        "--target",
        "wasm32-unknown-unknown",
        "--",
        "-D",
        "warnings",
    )
    .run()?;
    cmd!(
        "cargo",
        "check",
        "-p",
        "spacetimedb-portable-datastore",
        "--target",
        "wasm32-unknown-unknown"
    )
    .run()?;
    cmd!(
        "cargo",
        "check",
        "-p",
        "spacetimedb-portable-datastore-wasm",
        "--target",
        "wasm32-unknown-unknown"
    )
    .run()?;
    // The commitlog record format, the durability traits, datastore replay and the engine's
    // SQL/RLS layer without their filesystem, Tokio and host dependencies.
    // The engine is checked together with the datastore's `portable` feature, which the
    // datastore needs on wasm32.
    let portable_builds: [(&[&str], &str); 4] = [
        (&["spacetimedb-commitlog"], ""),
        (&["spacetimedb-durability"], ""),
        (&["spacetimedb-datastore"], "portable,execution,replay"),
        (
            &["spacetimedb-engine", "spacetimedb-datastore"],
            "spacetimedb-datastore/portable",
        ),
    ];
    for (packages, features) in portable_builds {
        for subcommand in ["check", "clippy"] {
            let mut args = vec![subcommand];
            for package in packages {
                args.extend(["-p", package]);
            }
            args.extend(["--no-default-features", "--target", "wasm32-unknown-unknown"]);
            if !features.is_empty() {
                args.extend(["--features", features]);
            }
            if subcommand == "clippy" {
                // `--no-deps`: lint these packages, not every workspace crate they pull in.
                args.extend(["--no-deps", "--", "-D", "warnings"]);
            }
            cmd("cargo", args).run()?;
        }
    }
    // The portable watch only compiles without `local`, so the default test run never sees it.
    cmd!(
        "cargo",
        "test",
        "-p",
        "spacetimedb-durability",
        "--no-default-features",
        "--lib"
    )
    .run()?;
    cmd!("cargo", "test", "-p", "spacetimedb-portable-datastore").run()?;
    cmd!(
        "node",
        "crates/bindings-typescript/scripts/generate-test-utils-wasm.mjs",
        "--clean"
    )
    .run()?;

    Ok(())
}
