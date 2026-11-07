//! What tests that start processes share: real ones, ended however a test
//! ends, a panic included, and none that runs on unbounded if one is missed.

/// Processes a test starts, ended when it is dropped. A test binary killed
/// outright takes them along by the job `job_of` ties them in.
pub struct Kids(pub Vec<std::process::Child>);

impl Drop for Kids {
    fn drop(&mut self) {
        for c in &mut self.0 {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

pub fn hidden(exe: &str, args: &[&str]) -> std::process::Child {
    use std::os::windows::process::CommandExt;
    std::process::Command::new(exe)
        .args(args)
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap()
}

/// A ping that sits there for at most `secs`.
pub fn idler(secs: u32) -> std::process::Child {
    hidden("ping", &["-n", &secs.to_string(), "127.0.0.1"])
}

/// A loop that burns a core for at most `secs`.
pub fn burner(secs: u32) -> std::process::Child {
    let script = format!("$end = [DateTime]::Now.AddSeconds({secs}); while ([DateTime]::Now -lt $end) {{ }}");
    hidden("powershell", &["-NoProfile", "-NonInteractive", "-Command", &script])
}

/// A job for a test's processes, tied as a helper's is: they go with the
/// test binary should it die.
pub fn job_of(first: &std::process::Child) -> std::sync::Arc<super::Job> {
    let job = super::adopt(first.id()).unwrap();
    super::tie(&job, "test");
    std::sync::Arc::new(job)
}

pub fn ended_within(c: &mut std::process::Child, ms: u64) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(ms);
    while std::time::Instant::now() < deadline {
        if c.try_wait().unwrap().is_some() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    false
}
