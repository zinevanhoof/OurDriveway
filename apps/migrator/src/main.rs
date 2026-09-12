//! The one process that migrates. Runs as a Compose one-shot in dev and a Helm hook Job
//! in production; never as part of a service.
//!
//! Exit code is the whole contract: `0` means every database is at its latest migration,
//! non-zero means at least one is not and the deployment must stop. There is no retry loop
//! here — a Job's `backoffLimit` is the retry policy, and a rerun is a no-op for
//! everything already applied.

fn main() -> std::process::ExitCode {
    // `.env` is a dev convenience and absent in containers, where the five URLs are
    // injected directly. Missing file is not an error; a missing URL is, and `run_one`
    // reports which one by name.
    let _ = dotenvy::dotenv();

    // Same one-liner every service uses. `env-filter` is not enabled on the workspace's
    // tracing-subscriber, so `RUST_LOG` is honoured through the default env filter rather
    // than a builder call.
    tracing_subscriber::fmt::init();

    // Multi-threaded on purpose, not `#[tokio::main(flavor = "current_thread")]`:
    // `AsyncMigrationHarness` drives diesel's synchronous migration machinery through
    // `block_in_place`, which panics on the current-thread runtime.
    let runtime = match tokio::runtime::Runtime::new() {
        Ok(r) => r,
        Err(e) => {
            tracing::error!(error = %e, "could not start the runtime");
            return std::process::ExitCode::FAILURE;
        }
    };

    tracing::info!("starting database migrations");

    match runtime.block_on(migrator::run_all()) {
        Ok(()) => {
            tracing::info!("all database migrations completed successfully");
            std::process::ExitCode::SUCCESS
        }
        // One line, naming the database and the migration. Nothing after the failure is
        // attempted and nothing already applied is rolled back — see `run_all`.
        Err(e) => {
            tracing::error!(error = %e, "migration failed");
            std::process::ExitCode::FAILURE
        }
    }
}
