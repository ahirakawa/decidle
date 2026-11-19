use crate::check::Report;
use decidle::i18n::{f, t, Lang};

const TEMPLATE: &str = include_str!("../templates/report.html");

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn none(lang: Lang) -> String {
    format!("<p class=\"none\">{}</p>", t(lang, "page.none"))
}

fn folded(title: &str, items: &[String], lang: Lang) -> String {
    let body = if items.is_empty() {
        none(lang)
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
/// The page's words are in `lang`; the report's own text is left as the agent wrote it.
pub fn render(report: &Report, meta: &str, issues: &[String], lang: Lang) -> String {
    let warnings = if issues.is_empty() {
        String::new()
    } else {
        let lis: String = issues.iter().map(|i| format!("<li>{}</li>", esc(i))).collect();
        format!("<div class=\"warn\">{}<ul>{lis}</ul></div>", t(lang, "page.warn"))
    };

    let decisions = if report.decisions.is_empty() {
        none(lang)
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
                            if recommended { format!("<span class=\"badge\">{}</span>", t(lang, "page.recommended")) } else { String::new() },
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
        .map(|d| {
            let changes: String = d
                .changes
                .iter()
                .map(|c| {
                    format!(
                        "<div class=\"change\"><div><span class=\"k\">{}</span></div><div><span class=\"k\">{}</span>{}</div><div><span class=\"k\">{}</span>{}</div></div>",
                        f(lang, "page.line", &[("n", &c.line.to_string())]),
                        t(lang, "page.before"),
                        esc(&c.before),
                        t(lang, "page.after"),
                        esc(&c.after)
                    )
                })
                .collect();
            format!(
                "<div class=\"file\"><div class=\"path\">{}</div><div class=\"summary\">{}</div>{changes}</div>",
                esc(&d.path),
                esc(&d.summary)
            )
        })
        .collect();
    let deliverables = format!(
        "<details><summary>{}<span class=\"count\">{}</span></summary>{}</details>\n",
        t(lang, "page.deliverables"),
        report.deliverables.len(),
        if files.is_empty() { none(lang) } else { files }
    );

    let details = format!(
        "{}{}{}",
        folded(t(lang, "page.unverified"), &report.unverified, lang),
        folded(t(lang, "page.verified"), &report.verified, lang),
        deliverables
    );

    fill(&[
        ("lang", lang.code()),
        ("goal_label", t(lang, "page.goal")),
        ("reason_label", t(lang, "page.reason")),
        ("decisions_label", t(lang, "page.decisions")),
        ("title", &esc(&report.title)),
        ("meta", &esc(meta)),
        ("warnings", &warnings),
        ("goal", &esc(&report.goal)),
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
