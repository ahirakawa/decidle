use crate::check::Report;

const TEMPLATE: &str = include_str!("../templates/report.html");

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn folded(title: &str, items: &[String]) -> String {
    let body = if items.is_empty() {
        "<p class=\"none\">なし</p>".to_string()
    } else {
        let lis: String = items.iter().map(|i| format!("<li>{}</li>", esc(i))).collect();
        format!("<ul>{lis}</ul>")
    };
    format!(
        "<details><summary>{title}<span class=\"count\">{}</span></summary>{body}</details>\n",
        items.len()
    )
}

/// Fills the fixed template. `issues` are problems left after the rejection limit was reached.
pub fn render(report: &Report, meta: &str, issues: &[String]) -> String {
    let warnings = if issues.is_empty() {
        String::new()
    } else {
        let lis: String = issues.iter().map(|i| format!("<li>{}</li>", esc(i))).collect();
        format!("<div class=\"warn\">この報告には、点検で残った指摘がある。<ul>{lis}</ul></div>")
    };

    let decisions = if report.decisions.is_empty() {
        "<p class=\"none\">なし</p>".to_string()
    } else {
        report
            .decisions
            .iter()
            .map(|d| {
                let options: String = d
                    .options
                    .iter()
                    .map(|o| {
                        let recommended = o.label == d.recommend;
                        format!(
                            "<div class=\"option{}\"><span class=\"label\">{}</span>{}<div class=\"effect\">{}</div></div>",
                            if recommended { " recommended" } else { "" },
                            esc(&o.label),
                            if recommended { "<span class=\"badge\">推奨</span>" } else { "" },
                            esc(&o.effect)
                        )
                    })
                    .collect();
                format!(
                    "<section class=\"decision\"><h3>{}</h3>{options}<p class=\"reason\">{}</p></section>\n",
                    esc(&d.question),
                    esc(&d.reason)
                )
            })
            .collect()
    };

    let files: String = report
        .deliverables
        .iter()
        .map(|f| {
            let changes: String = f
                .changes
                .iter()
                .map(|c| {
                    format!(
                        "<div class=\"change\"><div><span class=\"k\">{}行目</span></div><div><span class=\"k\">変更前</span>{}</div><div><span class=\"k\">変更後</span>{}</div></div>",
                        c.line,
                        esc(&c.before),
                        esc(&c.after)
                    )
                })
                .collect();
            format!(
                "<div class=\"file\"><div class=\"path\">{}</div><div class=\"summary\">{}</div>{changes}</div>",
                esc(&f.path),
                esc(&f.summary)
            )
        })
        .collect();
    let deliverables = format!(
        "<details><summary>成果物<span class=\"count\">{}</span></summary>{}</details>\n",
        report.deliverables.len(),
        if files.is_empty() { "<p class=\"none\">なし</p>".to_string() } else { files }
    );

    let details = format!(
        "{}{}{}",
        folded("確かめていないこと", &report.unverified),
        folded("確かめたこと", &report.verified),
        deliverables
    );

    fill(&[
        ("title", &esc(&report.title)),
        ("meta", &esc(meta)),
        ("warnings", &warnings),
        ("conclusion", &esc(&report.conclusion)),
        ("decisions", &decisions),
        ("details", &details),
    ])
}

/// Substitutes `{{name}}` in one pass over the template.
/// Inserted values are never scanned again, so report text that looks like a placeholder stays text.
fn fill(values: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(TEMPLATE.len());
    let mut rest = TEMPLATE;
    while let Some(open) = rest.find("{{") {
        out.push_str(&rest[..open]);
        let after = &rest[open + 2..];
        let known = after
            .find("}}")
            .and_then(|close| values.iter().find(|(k, _)| *k == &after[..close]).map(|(_, v)| (close, *v)));
        match known {
            Some((close, value)) => {
                out.push_str(value);
                rest = &after[close + 2..];
            }
            None => {
                out.push_str("{{");
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// file:// URL for a local path, percent-encoding everything outside the unreserved set.
pub fn file_url(path: &str) -> String {
    let mut out = String::from("file://");
    for b in path.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
