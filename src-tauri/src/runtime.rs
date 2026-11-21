//! Luna's async runtime: one tokio runtime, of Luna's own, for the work its
//! sync code hands off (`spawn_blocking`) and for the futures it waits on
//! (`block_on`), reqwest's above all (http.rs). Built on first use, with the
//! I/O and time drivers reqwest's connections and timeouts need, and its
//! threads named `luna-runtime`.
//!
//! Two workers: nothing of Luna's runs as a task on it, so they only drive
//! what reqwest spawns (its connections) and the timers. The blocking work
//! has a pool of its own, as tokio sizes it.
//!
//! It lives until the process exits and is never shut down: a static is not
//! dropped, so whatever runs on it at exit, a blocking task included, ends
//! with the process, unjoined.
//!
//! A panic in a `spawn_blocking` closure comes back as the `JoinError` its
//! handle answers with (`is_panic`), the runtime unharmed; one in a future
//! under `block_on` unwinds the thread that waits on it.

use std::future::Future;
use std::sync::OnceLock;

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("luna-runtime")
            .build()
            // Fails only where the OS gives no thread or no I/O driver, and
            // nothing of Luna's can wait on anything then.
            .expect("cannot start Luna's async runtime")
    })
}

/// Runs `f` on a thread of the runtime's blocking pool.
pub fn spawn_blocking<F, R>(f: F) -> tokio::task::JoinHandle<R>
where
    F: FnOnce() -> R + Send + 'static,
    R: Send + 'static,
{
    runtime().spawn_blocking(f)
}

/// Runs `f` to its end on the calling thread, the runtime's workers driving
/// the I/O and the timers it waits on.
///
/// For a thread of its own or a `spawn_blocking` one, which is where every
/// caller is. It panics on a thread already inside a runtime, within another
/// `block_on`'s future or in a task of any runtime: there it would hold a
/// thread the runtime needs, and could wait for good.
pub fn block_on<F: Future>(f: F) -> F::Output {
    runtime().block_on(f)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::time::Duration;

    #[test]
    fn waits_on_the_runtimes_timers_from_a_plain_thread() {
        let waited = block_on(async {
            let t = tokio::time::Instant::now();
            tokio::time::sleep(Duration::from_millis(20)).await;
            t.elapsed()
        });
        assert!(waited >= Duration::from_millis(20));
    }

    #[test]
    fn reads_a_socket_from_a_plain_thread() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            // Late, so the read waits on the I/O driver.
            std::thread::sleep(Duration::from_millis(20));
            conn.write_all(b"hello").unwrap();
        });
        let got = block_on(async {
            let stream = tokio::net::TcpStream::connect(addr).await.unwrap();
            let mut got = Vec::new();
            let mut buf = [0u8; 16];
            loop {
                stream.readable().await.unwrap();
                match stream.try_read(&mut buf) {
                    Ok(0) => break got,
                    Ok(n) => got.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => continue,
                    Err(e) => panic!("{e}"),
                }
            }
        });
        server.join().unwrap();
        assert_eq!(got, b"hello");
    }

    #[test]
    fn blocking_work_runs_on_the_runtimes_named_threads_and_may_wait() {
        // As http.rs is called from a `spawn_blocking` closure.
        let name = block_on(spawn_blocking(|| {
            block_on(async { tokio::time::sleep(Duration::from_millis(5)).await });
            std::thread::current().name().map(String::from)
        }))
        .unwrap();
        assert_eq!(name.as_deref(), Some("luna-runtime"));
    }

    #[test]
    fn a_panic_in_blocking_work_is_its_handles_error() {
        let got = block_on(spawn_blocking(|| -> u32 { panic!("on purpose") }));
        assert!(got.is_err_and(|e| e.is_panic()));
        assert_eq!(block_on(spawn_blocking(|| 7)).unwrap(), 7, "and the runtime runs on");
    }

    #[test]
    fn a_wait_inside_the_runtime_panics_rather_than_hangs() {
        // In a task on a worker, and in a future another `block_on` polls.
        let in_task = block_on(runtime().spawn(async { block_on(async {}) }));
        assert!(in_task.is_err_and(|e| e.is_panic()));
        let nested = std::panic::catch_unwind(|| block_on(async { block_on(async {}) }));
        assert!(nested.is_err());
    }
}
