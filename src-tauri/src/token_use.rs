//! A switch the test build alone has, for runs on clones of real accounts: with
//! `LUNA_TEST_NO_TOKEN_USE=1` in its environment Luna makes no request with an
//! account's credentials and runs no CLI on one to ask it something (usage
//! limits, model lists, context windows, a token renewal). A clone holds the
//! same OAuth grant as the account it came from, and one renewal rotates the
//! refresh token away from the other folder; with the switch on, only the
//! sessions a person starts touch the token. In any other build it is off.

#[cfg(any(test, luna_test))]
use std::ffi::OsString;

/// The variable that turns the switch on, with the value `1`.
#[cfg(any(test, luna_test))]
const VAR: &str = "LUNA_TEST_NO_TOKEN_USE";

/// Whether Luna may use the credentials in `account_path` for `what`, a
/// request or a CLI run. A refusal is logged each time, so a run can show
/// what it kept off the token.
pub fn allowed(what: &str, account_path: &str) -> bool {
    #[cfg(luna_test)]
    if switched_on(std::env::var_os) {
        crate::log::info("no-token", &format!("skipped {what} for {account_path} ({VAR}=1)"));
        return false;
    }
    let _ = (what, account_path);
    true
}

/// Whether the environment `env` reads has the switch on.
#[cfg(any(test, luna_test))]
fn switched_on(env: impl Fn(&'static str) -> Option<OsString>) -> bool {
    env(VAR).is_some_and(|v| v == "1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_one_in_the_variable_turns_the_switch_on() {
        let with = |value: Option<&'static str>| switched_on(move |name: &str| value.filter(|_| name == VAR).map(OsString::from));
        assert!(with(Some("1")));
        for off in [None, Some(""), Some("0"), Some("true"), Some(" 1")] {
            assert!(!with(off), "{off:?}");
        }
        assert!(!switched_on(|name: &str| (name != VAR).then(|| OsString::from("1"))), "another variable");
    }

    #[test]
    fn outside_the_test_build_the_tokens_stay_usable() {
        // Unit tests are not the test flavor: the switch is not there,
        // whatever the environment says.
        if !cfg!(luna_test) {
            assert!(allowed("usage limits", "/accounts/a"));
        }
    }
}
