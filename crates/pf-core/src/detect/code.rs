//! Heuristic source-code detector and language guesser (returns GtkSourceView language IDs).

/// Detect whether `text` looks like source code / structured config and return its
/// GtkSourceView language ID (e.g. `"rust"`, `"python"`, `"js"`, `"json"`, `"sql"`, `"sh"`, etc.).
pub fn detect_code_language(text: &str) -> Option<&'static str> {
    let trimmed = text.trim();
    if trimmed.len() < 8 {
        return None;
    }

    // 1. JSON object or array
    if ((trimmed.starts_with('{') && trimmed.ends_with('}'))
        || (trimmed.starts_with('[') && trimmed.ends_with(']')))
        && serde_json::from_str::<serde_json::Value>(trimmed).is_ok()
    {
        return Some("json");
    }

    // 2. Shebangs
    if let Some(first_line) = trimmed.lines().next() {
        if first_line.starts_with("#!") {
            if first_line.contains("python") {
                return Some("python");
            }
            if first_line.contains("node") || first_line.contains("deno") {
                return Some("js");
            }
            if first_line.contains("sh") || first_line.contains("bash") || first_line.contains("zsh") {
                return Some("sh");
            }
        }
    }

    // 3. HTML / XML
    if (trimmed.starts_with("<!DOCTYPE html")
        || trimmed.starts_with("<html")
        || trimmed.starts_with("<div")
        || trimmed.starts_with("<span")
        || trimmed.starts_with("<section"))
        && trimmed.contains("</")
    {
        return Some("html");
    }
    if trimmed.starts_with("<?xml") && trimmed.contains("?>") {
        return Some("xml");
    }

    // 4. Score language-specific signals
    let mut best: Option<(&'static str, u32)> = None;
    let mut consider = |lang: &'static str, score: u32| {
        if score >= 2 {
            match best {
                Some((_, s)) if s >= score => {}
                _ => best = Some((lang, score)),
            }
        }
    };

    // Rust
    let rust_score = count_hits(
        trimmed,
        &[
            "fn ",
            "let mut ",
            "impl ",
            "pub struct ",
            "pub enum ",
            "pub fn ",
            "match ",
            "println!(",
            "use std::",
            "Result<",
            "Option<",
            "::",
            "->",
        ],
    );
    consider("rust", rust_score);

    // Python
    let py_score = count_hits(
        trimmed,
        &[
            "def ",
            "import ",
            "from ",
            "elif ",
            "if __name__ ==",
            "self.",
            "lambda ",
            "async def ",
            "print(",
        ],
    );
    consider("python", py_score);

    // TypeScript / JavaScript
    let ts_score = count_hits(
        trimmed,
        &[
            "interface ",
            ": string",
            ": number",
            ": boolean",
            "type ",
            "export const ",
            "export function ",
            "async function ",
            "console.log(",
            "=> {",
            "const ",
            "import {",
        ],
    );
    if ts_score >= 2 {
        let is_ts = trimmed.contains("interface ")
            || trimmed.contains(": string")
            || trimmed.contains(": number")
            || trimmed.contains(": boolean");
        consider(if is_ts { "typescript" } else { "js" }, ts_score);
    }

    // SQL
    let upper = trimmed.to_ascii_uppercase();
    let sql_score = count_hits(
        &upper,
        &[
            "SELECT ",
            " FROM ",
            " WHERE ",
            "INSERT INTO ",
            "CREATE TABLE ",
            "UPDATE ",
            "DELETE FROM ",
            "JOIN ",
            "GROUP BY ",
            "ORDER BY ",
        ],
    );
    consider("sql", sql_score);

    // Go
    let go_score = count_hits(
        trimmed,
        &[
            "package ",
            "func ",
            "fmt.Print",
            ":= ",
            "go func(",
            "defer ",
            "chan ",
        ],
    );
    consider("go", go_score);

    // C / C++
    let c_score = count_hits(
        trimmed,
        &[
            "#include <",
            "#include \"",
            "int main(",
            "std::",
            "printf(",
            "nullptr",
            "sizeof(",
        ],
    );
    consider("cpp", c_score);

    // CSS
    let css_score = count_hits(
        trimmed,
        &[
            "display:",
            "background-color:",
            "font-size:",
            "margin:",
            "padding:",
            "border-radius:",
            "@media ",
        ],
    );
    consider("css", css_score);

    // Shell / Bash
    let sh_score = count_hits(
        trimmed,
        &[
            "export ",
            "sudo ",
            "grep ",
            "chmod ",
            "systemctl ",
            "#!/",
            " | ",
            " && ",
            "echo ",
            "fi\n",
            "done\n",
        ],
    );
    consider("sh", sh_score);

    // TOML / INI
    let toml_score = count_hits(trimmed, &["[package]", "[dependencies]", "[workspace", " = \""]);
    consider("toml", toml_score);

    best.map(|(lang, _)| lang)
}

fn count_hits(haystack: &str, needles: &[&str]) -> u32 {
    needles.iter().filter(|&&n| haystack.contains(n)).count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_languages() {
        assert_eq!(
            detect_code_language("pub fn main() -> Result<()> {\n    println!(\"hi\");\n}"),
            Some("rust")
        );
        assert_eq!(
            detect_code_language("import os\ndef greet(name):\n    print(name)"),
            Some("python")
        );
        assert_eq!(
            detect_code_language("{\"hello\": \"world\", \"count\": 42}"),
            Some("json")
        );
        assert_eq!(
            detect_code_language("SELECT id, name FROM clips WHERE pinned = 1 ORDER BY id DESC;"),
            Some("sql")
        );
        assert_eq!(
            detect_code_language("export const add = (a: number, b: number) => { return a + b; };"),
            Some("typescript")
        );
        assert_eq!(
            detect_code_language("#!/bin/bash\nexport FOO=1\necho $FOO && sudo systemctl status"),
            Some("sh")
        );
        assert_eq!(
            detect_code_language("Just a normal English sentence about groceries and coffee."),
            None
        );
    }
}
