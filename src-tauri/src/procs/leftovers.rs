//! What a session's turns left running past their end, as last read: what
//! `luna_session_get` names, and what keeps the session active for as long as
//! it is there.

/// A process a session's turns left running past their end, as last read.
#[derive(Clone, Debug, PartialEq)]
pub struct Leftover {
    pub pid: u32,
    pub exe: String,
    pub started_ms: u64,
}

impl Leftover {
    /// "tail.exe (pid 22104)".
    pub fn label(&self) -> String {
        format!("{} (pid {})", self.exe, self.pid)
    }
}

/// A few of them by `label`, for a notice.
pub fn left_names(left: &[Leftover]) -> String {
    const SHOWN: usize = 3;
    let mut names: Vec<String> = left.iter().take(SHOWN).map(Leftover::label).collect();
    if left.len() > SHOWN {
        names.push(format!("{} more", left.len() - SHOWN));
    }
    names.join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leftovers_are_named_a_few_at_a_time() {
        let left: Vec<Leftover> = (1..=4).map(|pid| Leftover { pid, exe: format!("p{pid}"), started_ms: 7 }).collect();
        assert_eq!(left_names(&left), "p1 (pid 1), p2 (pid 2), p3 (pid 3), 1 more");
        assert_eq!(left_names(&left[..1]), "p1 (pid 1)");
    }
}
