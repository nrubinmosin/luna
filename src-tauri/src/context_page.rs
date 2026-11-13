//! docs/user-guide/context.md tells the person how much of a model's context
//! each fixed text of Luna's takes. Its "Measured sizes" table is checked here
//! against the texts themselves, so a change that grows or cuts one fails
//! until the page says so: the page is the one place the cost is shown, and a
//! number nobody keeps is a hidden cost again. The page is compiled into the
//! tests alone, never into the core (fingerprint.rs `INPUTS`).

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    const PAGE: &str = include_str!("../../docs/user-guide/context.md");

    /// How far the page's count may be from the text's, in percent of the
    /// text, and at least in characters: an edit of a few words need not touch
    /// the page.
    const SLACK_PCT: usize = 5;
    const SLACK_CHARS: usize = 20;

    /// The page's table: each row's name with the characters and tokens it states.
    fn stated() -> BTreeMap<String, (usize, usize)> {
        let number = |cell: &str| cell.trim().replace(',', "").parse::<usize>().ok();
        let table = PAGE.split("\n## Measured sizes").nth(1).unwrap_or_default();
        let table = table.split("\n## ").next().unwrap_or_default();
        let mut rows = BTreeMap::new();
        for line in table.lines() {
            let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
            let [name, chars, tokens, ..] = cells[..] else { continue };
            let Some(name) = name.trim().strip_prefix('`').and_then(|n| n.strip_suffix('`')) else { continue };
            if let (Some(chars), Some(tokens)) = (number(chars), number(tokens)) {
                rows.insert(name.to_string(), (chars, tokens));
            }
        }
        rows
    }

    /// Every text the table measures, by its name there, in characters; and
    /// what each role carries with every request, from them.
    fn measured() -> BTreeMap<String, usize> {
        let mut texts: Vec<(String, String)> = Vec::new();
        texts.extend(crate::mcp::context_texts());
        texts.extend(crate::library::context_texts());
        texts.extend(crate::agents::handoff::context_texts());
        texts.push(("notice-tag".into(), format!("{} ", crate::agents::NOTICE_TAG)));
        texts.push(("guard".into(), crate::guard::REFUSAL.into()));
        let mut out: BTreeMap<String, usize> = texts.into_iter().map(|(name, text)| (name, text.chars().count())).collect();
        let sum = |names: &[&str]| names.iter().map(|n| out[*n]).sum::<usize>();
        let chat = sum(&["instructions", "tools", "handoff-tool"]);
        let lead = sum(&["instructions", "tools", "handoff-tool", "role-lead"]);
        let helper = sum(&["role-helper"]);
        out.insert("per-request-chat".into(), chat);
        out.insert("per-request-lead".into(), lead);
        out.insert("per-request-helper".into(), helper);
        out
    }

    #[test]
    fn the_context_page_states_each_text_as_it_is() {
        let page = stated();
        let texts = measured();
        let mut wrong = Vec::new();
        for (name, &n) in &texts {
            match page.get(name) {
                None => wrong.push(format!("`{name}`: {n} characters, not on the page")),
                Some(&(chars, tokens)) => {
                    if chars.abs_diff(n) > (n * SLACK_PCT / 100).max(SLACK_CHARS) {
                        wrong.push(format!("`{name}`: the page says {chars} characters, the text has {n}"));
                    }
                    // Tokens are the page's characters over four, rounded.
                    if tokens != (chars + 2) / 4 {
                        wrong.push(format!("`{name}`: {chars} characters are {} tokens, not {tokens}", (chars + 2) / 4));
                    }
                }
            }
        }
        for name in page.keys().filter(|name| !texts.contains_key(*name)) {
            wrong.push(format!("`{name}`: on the page, measured nowhere"));
        }
        assert!(wrong.is_empty(), "docs/user-guide/context.md, Measured sizes, is out of date:\n{}", wrong.join("\n"));
    }
}
