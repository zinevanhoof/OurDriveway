use std::sync::LazyLock;

use argon2::{
    Argon2, PasswordHash, PasswordHasher, PasswordVerifier,
    password_hash::{SaltString, rand_core::OsRng},
};
use shared::error::myerror::{ContextExt, MyResult};
use tokio::sync::Semaphore;

/// One Argon2 at a time per CPU. See "Why a semaphore" on [`hash`].
static IN_FLIGHT: LazyLock<Semaphore> = LazyLock::new(|| {
    Semaphore::new(std::thread::available_parallelism().map_or(1, |n| n.get()))
});

/// Hashing happens in Rust, on the write side, once — and the result goes into
/// this service's own row and nowhere else. It is never put in an event: nothing
/// downstream has any business knowing it, which is why `UserRegistered` and
/// `UserPasswordChanged` carry no password at all.
///
/// The original reason for hashing here was determinism — Argon2 salts randomly,
/// so a projection computing it would give a different answer on every replica.
/// That still holds; it just no longer implies the hash has to travel.
///
/// ## Why a semaphore
///
/// `Argon2::default()` is Argon2id at 19 MiB and two passes — about 35ms of CPU and one
/// 19 MiB buffer per call. It runs inline on the tokio worker, behind [`IN_FLIGHT`]:
/// one permit per CPU, so memory is capped at `cores × 19 MiB` however many people log
/// in at once, and a request waiting its turn waits on the semaphore — parked, off the
/// run queue — instead of queueing on the workers in front of everything else.
///
/// That second half is what the permit is for. Without it, a login burst put ~100
/// hashes per pod on 4 workers, and `/healthz` waited behind all of them: p99 1.29s
/// against the liveness probe's 1s timeout, measured at 200 concurrent logins on 4
/// cores. With it, anything else waits for at most one hash in flight.
///
/// Not `spawn_blocking`: that pool grows to 512 threads, which is ~10 GB of Argon2
/// buffers under a burst unless something like this semaphore bounds it anyway.
///
/// The cap only holds if freed buffers go back to the OS. glibc keeps them in per-thread
/// arenas by default, and a burst left each pod holding ~400–800 MiB for good —
/// `MALLOC_MMAP_THRESHOLD_` in this service's Dockerfile is what prevents that.
pub async fn hash(password: &str) -> MyResult<String> {
    // Never closed, so this cannot fail; mapped rather than unwrapped all the same.
    let _permit = IN_FLIGHT
        .acquire()
        .await
        .context_internal("Could not hash password")?;
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        // Was `MyError::Bus`, which is for the event log and rendered the argon2
        // error straight into the response body. This is a 500 either way, and the
        // client learns nothing about the hasher.
        .context_internal("Could not hash password")
}

/// Constant-time verification against a stored PHC string. A malformed or
/// missing hash is a failed login, never an error the caller has to handle —
/// otherwise the response would distinguish "no such user" from "wrong password".
///
/// Behind the same [`IN_FLIGHT`] permit as [`hash`] — a login costs exactly what a
/// signup does.
pub async fn verify(phc: &str, password: &str) -> bool {
    let Ok(_permit) = IN_FLIGHT.acquire().await else {
        return false;
    };
    PasswordHash::new(phc)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_and_rejects() {
        let phc = hash("Sup3rSecret!x").await.unwrap();
        assert!(verify(&phc, "Sup3rSecret!x").await);
        assert!(!verify(&phc, "wrong").await);
        // Salted: the same password hashes differently every time, which is
        // exactly why this can't live in a projection.
        assert_ne!(phc, hash("Sup3rSecret!x").await.unwrap());
        // A corrupt stored hash is a failed login, not a panic.
        assert!(!verify("not-a-phc-string", "Sup3rSecret!x").await);
    }

    /// The memory cap is the permit count: with every permit taken, a hash waits
    /// instead of allocating its 19 MiB alongside the others.
    #[tokio::test]
    async fn a_hash_waits_for_a_permit() {
        // The full count, not `available_permits()`: another test may be holding one,
        // and releasing it mid-check would let this hash through.
        let total = std::thread::available_parallelism().map_or(1, |n| n.get()) as u32;
        let all = IN_FLIGHT.acquire_many(total).await.unwrap();

        let waited =
            tokio::time::timeout(std::time::Duration::from_millis(300), hash("Sup3rSecret!x"))
                .await;
        assert!(waited.is_err(), "hashed without a permit");

        drop(all);
        assert!(hash("Sup3rSecret!x").await.is_ok());
    }
}
