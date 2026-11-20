//! Luna's async runtime: one tokio runtime, of Luna's own, for the work its
//! sync code hands off (`spawn_blocking`) and for the futures it waits on
//! (`block_on`), reqwest's above all (http.rs). Built on first use, with the
//! I/O and time drivers reqwest's connections and timeouts need, a worker a
//! core as tokio sets it, and its threads named `luna-runtime`.
//!
//! It lives until the process exits and is never shut down: a static is not
//! dropped, so whatever runs on it at exit, a blocking task included, ends
//! with the process, unjoined. Nothing here spawns a task of Luna's onto it;
//! its workers run only what reqwest spawns (its connections).
//!
//! A panic in a `spawn_blocking` closure comes back as the `JoinError` its
//! handle answers with (`is_panic`), the runtime unharmed; one in a future
//! under `block_on` unwinds the thread that waits on it.

use std::future::Future;
use std::pin::pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

fn runtime() -> &'static tokio::runtime::Runtime {
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
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

/// Runs `f` to its end on the calling thread, the runtime at hand for what
/// it makes (a socket, a timer, a task, `spawn_blocking`) and its workers
/// driving the I/O and the timers it waits on.
///
/// Not tokio's own `block_on`, which panics on a thread already inside a
/// runtime: within another `block_on`'s future, or in a task of any runtime,
/// Tauri's commands among them. This one waits wherever it is called, parking
/// the thread between polls. Its callers today are threads of their own and
/// `spawn_blocking` ones, where tokio's would do as well. A task that calls it
/// holds its worker meanwhile, as any blocking call there does.
pub fn block_on<F: Future>(f: F) -> F::Output {
    let _runtime = runtime().enter();
    let woken = Arc::new(Woken { flag: AtomicBool::new(false), thread: thread::current() });
    let waker = Waker::from(Arc::clone(&woken));
    let mut cx = Context::from_waker(&waker);
    // Unconstrained: inside a task, tokio's budget is that task's, and once
    // spent it makes every resource answer "not yet" and wake at once until
    // the task yields, which a thread waiting here never does.
    let mut f = pin!(tokio::task::unconstrained(f));
    loop {
        if let Poll::Ready(out) = f.as_mut().poll(&mut cx) {
            return out;
        }
        // The flag, not the park alone: a `block_on` nested in `f`'s poll
        // parks this same thread, and may take the unpark meant for this one.
        while !woken.flag.swap(false, Ordering::Acquire) {
            thread::park();
        }
    }
}

/// Wakes the thread a `block_on` waits on.
struct Woken {
    flag: AtomicBool,
    thread: Thread,
}

impl Wake for Woken {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.flag.store(true, Ordering::Release);
        self.thread.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn blocking_work_runs_on_the_runtimes_named_threads() {
        let name = block_on(spawn_blocking(|| thread::current().name().map(String::from))).unwrap();
        assert_eq!(name.as_deref(), Some("luna-runtime"));
    }

    #[test]
    fn waits_inside_blocking_work_and_inside_another_wait() {
        // As http.rs is called: from a `spawn_blocking` closure, and from a
        // future `block_on` already polls.
        let inside = block_on(spawn_blocking(|| block_on(async { 1 }))).unwrap();
        let nested = block_on(async {
            let inner = block_on(async {
                tokio::time::sleep(Duration::from_millis(5)).await;
                2
            });
            tokio::time::sleep(Duration::from_millis(5)).await;
            inner + 1
        });
        assert_eq!((inside, nested), (1, 3));
    }

    #[test]
    fn waits_inside_a_task_of_another_runtime_past_its_budget() {
        // Where tokio's own `block_on` panics. More handles answered than a
        // task's budget allows (128), each answer taking from it: under the
        // task's budget the wait would spin on "not yet" for good.
        let other = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let got = other.block_on(async {
            block_on(async {
                let mut sum = 0;
                for i in 0..300u32 {
                    sum += spawn_blocking(move || i).await.unwrap();
                }
                sum
            })
        });
        assert_eq!(got, (0..300).sum::<u32>());
    }

    #[test]
    fn a_panic_in_blocking_work_is_its_handles_error() {
        let got = block_on(spawn_blocking(|| -> u32 { panic!("on purpose") }));
        assert!(got.is_err_and(|e| e.is_panic()));
        assert_eq!(block_on(spawn_blocking(|| 7)).unwrap(), 7, "and the runtime runs on");
    }
}
