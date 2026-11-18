//! A link a terminal was clicked on, opened in the system's handler for it:
//! the user's default browser or mail app. A window the WebView opened would
//! be a browser of Luna's own, with none of the user's sign-ins.
//!
//! A terminal prints anything, so the link is untrusted: web and mail links
//! only, parsed and passed on in their normalized form, and no part of one
//! ever in an error, which `call` writes to the log (a link may carry a
//! sign-in code).

use tauri::Url;

/// Longer than any link a CLI prints for a person to click.
const MAX_LEN: usize = 8 * 1024;

#[tauri::command]
pub fn open_link(url: String) -> Result<(), String> {
    open_with(&url, |u| tauri_plugin_opener::open_url(u, None::<&str>))
}

/// `open_link` with the launch given, so a test can make it fail. The
/// opener's own error is dropped: off Windows it can quote the command it
/// ran, link and all.
fn open_with<E>(link: &str, open: impl FnOnce(&str) -> Result<(), E>) -> Result<(), String> {
    let url = web_or_mail(link)?;
    open(url.as_str()).map_err(|_| "the system could not open the link".to_string())
}

/// The link as the handler gets it: http, https or mailto, parsed, so what
/// reaches a handler's command line at its `%1` is percent-encoded (a
/// space or a quote cannot end the argument there). Refused: one that does
/// not parse, carries a control character (the parser would drop tabs and
/// newlines silently), runs past `MAX_LEN`, is a mail link that asks for an
/// attachment, which some mail apps take from the local disk, or still has
/// a space or a quote once parsed.
fn web_or_mail(link: &str) -> Result<Url, String> {
    if link.len() > MAX_LEN {
        return Err(format!("the link is longer than {MAX_LEN} bytes"));
    }
    if link.chars().any(char::is_control) {
        return Err("the link has a control character in it".into());
    }
    let url = Url::parse(link).map_err(|_| "not a link Luna can read".to_string())?;
    match url.scheme() {
        "http" | "https" => {}
        "mailto" => {
            if url.query_pairs().any(|(k, _)| k.eq_ignore_ascii_case("attach") || k.eq_ignore_ascii_case("attachment")) {
                return Err("a mail link that asks for an attachment".into());
            }
        }
        // A scheme the parser took is letters, digits, `+`, `-` and `.`
        // after a letter; a long one is named no more than a short word
        // would be, so a token shaped like one stays out.
        s if s.len() <= 32 => return Err(format!("not a web or mail link ({s}:)")),
        _ => return Err("not a web or mail link".into()),
    }
    // The parser percent-encodes neither a mail link's address nor a quote
    // in a web link's host, and Windows hands the link to the handler's
    // command line as it is (ShellExecuteExW, through the opener), where a
    // quote would end a quoted `"%1"`.
    if url.as_str().chars().any(|c| c.is_whitespace() || c == '"') {
        return Err("a link with a space or a quote in it".into());
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_web_and_mail_links_open() {
        for ok in ["https://example.com/a?b=c", "HTTP://example.com", "mailto:a@example.com?subject=Hi%20there"] {
            assert!(web_or_mail(ok).is_ok(), "{ok}");
        }
        for no in ["file:///C:/Windows/System32/calc.exe", "javascript:alert(1)", "ms-settings:", r"C:\x.exe", "example.com", ""] {
            assert!(web_or_mail(no).is_err(), "{no}");
        }
    }

    #[test]
    fn no_part_of_a_refused_link_reaches_the_error() {
        for link in [
            "vscode://open?code=SECRET",
            "example.com/reset?token=SECRET:x",
            "//example.com/?token=SECRET:rest",
            "https://example.com/?token=SECRET\n",
            "SECRETSECRETSECRETSECRETSECRETSECRET:x",
        ] {
            let e = web_or_mail(link).unwrap_err();
            assert!(!e.to_lowercase().contains("secret"), "{link:?}: {e}");
        }
        assert_eq!(web_or_mail("vscode://open?code=SECRET").unwrap_err(), "not a web or mail link (vscode:)");
    }

    #[test]
    fn a_launch_that_fails_says_nothing_of_the_link() {
        let e = open_with("https://example.com/?token=SECRET", |u| Err(format!("Launcher \"open\" failed with: {u}"))).unwrap_err();
        assert!(!e.contains("SECRET"), "{e}");
        let mut got = None;
        open_with("https://example.com/?token=SECRET", |u| {
            got = Some(u.to_string());
            Ok::<(), ()>(())
        })
        .unwrap();
        assert_eq!(got.as_deref(), Some("https://example.com/?token=SECRET"), "a link that opens is the handler's whole");
    }

    #[test]
    fn the_handler_gets_the_link_percent_encoded() {
        let u = web_or_mail(r#"https://example.com/a b" --flag?q=c d"e#f g"h"#).unwrap();
        assert!(!u.as_str().contains(' ') && !u.as_str().contains('"'), "{u}");
        assert!(web_or_mail(r#"mailto:a@example.com" --flag"#).is_err());
        // The parser keeps a quote in a host as it is.
        let host = r#"https://a"b.example.com/"#;
        assert!(Url::parse(host).unwrap().as_str().contains('"'));
        assert_eq!(web_or_mail(host).unwrap_err(), "a link with a space or a quote in it");
        let mail = web_or_mail("mailto:a@example.com?subject=a b").unwrap();
        assert_eq!(mail.as_str(), "mailto:a@example.com?subject=a%20b");
    }

    #[test]
    fn control_characters_and_overlong_links_are_refused() {
        for c in ['\n', '\r', '\t', '\0', '\u{1b}', '\u{7f}', '\u{85}'] {
            assert!(web_or_mail(&format!("https://example.com/{c}x")).is_err(), "{c:?}");
        }
        let long = format!("https://example.com/{}", "a".repeat(MAX_LEN));
        assert!(web_or_mail(&long).is_err());
        assert!(web_or_mail(&long[..MAX_LEN]).is_ok());
    }

    #[test]
    fn a_mail_link_may_not_ask_for_an_attachment() {
        for no in
            ["mailto:a@example.com?attach=C:/x", "mailto:a@example.com?subject=x&Attachment=/etc/passwd", "mailto:a@example.com?att%61ch=x"]
        {
            assert!(web_or_mail(no).is_err(), "{no}");
        }
    }
}
