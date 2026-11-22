//! Server-side rendering of the status page. No templates, no JavaScript
//! required: the page is complete HTML and refreshes itself.

use std::fmt::Write;

use super::{HistoryPoint, MonitorView, StatusSnapshot, HISTORY_POINTS};
use crate::config::Language;
use crate::i18n::{format_duration, texts, Texts};
use crate::monitor::Status;
use crate::notify::format_time;

/// Escape text for HTML content and attribute values.
pub fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

fn status_label(t: &Texts, s: Option<Status>) -> (&'static str, &'static str) {
    match s {
        Some(Status::Up) => ("up", t.up),
        Some(Status::Warn) => ("warn", t.warn),
        Some(Status::Down) => ("down", t.down),
        None => ("pending", t.pending),
    }
}

fn percent(v: Option<f64>) -> String {
    match v {
        None => "–".to_owned(),
        Some(v) if v >= 0.99995 => "100%".to_owned(),
        Some(v) => format!("{:.2}%", (v * 100.0).min(99.99)),
    }
}

fn ago(secs: i64, lang: Language) -> String {
    let d = format_duration(secs, lang);
    match lang {
        Language::En => format!("{d} ago"),
        Language::Tr => format!("{d} önce"),
    }
}

fn time_tag(ts: i64) -> String {
    format!(
        "<time datetime=\"{}\" data-ts=\"{ts}\">{}</time>",
        crate::notify::format_rfc3339(ts),
        format_time(ts)
    )
}

/// Inline SVG sparkline of response times; down checks are marked in red.
fn sparkline(points: &[HistoryPoint], label: &str) -> String {
    const W: f64 = 300.0;
    const H: f64 = 40.0;
    const PAD: f64 = 3.0;
    if points.is_empty() {
        return String::new();
    }
    let slot = W / HISTORY_POINTS as f64;
    let offset = (HISTORY_POINTS.saturating_sub(points.len())) as f64 * slot;
    let max = points
        .iter()
        .filter_map(|p| p.latency_ms)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let x = |i: usize| offset + i as f64 * slot + slot / 2.0;
    let y = |ms: i64| H - PAD - (ms as f64 / max) * (H - 2.0 * PAD);

    let mut svg = String::new();
    let _ = write!(
        svg,
        "<svg class=\"spark\" viewBox=\"0 0 {W} {H}\" preserveAspectRatio=\"none\" role=\"img\" aria-label=\"{}\">",
        esc(label)
    );
    for (i, p) in points.iter().enumerate() {
        if p.status == Status::Down {
            let _ = write!(
                svg,
                "<rect class=\"spark-down\" x=\"{:.1}\" y=\"0\" width=\"{:.1}\" height=\"{H}\"><title>{}</title></rect>",
                x(i) - slot / 2.0,
                slot,
                esc(&format_time(p.ts))
            );
        }
    }
    // Break the line at down checks (they have no meaningful latency).
    let mut segment: Vec<String> = Vec::new();
    let flush = |seg: &mut Vec<String>, svg: &mut String| {
        if seg.len() == 1 {
            let mut parts = seg[0].split(',');
            let (cx, cy) = (parts.next().unwrap_or("0"), parts.next().unwrap_or("0"));
            let _ = write!(
                svg,
                "<circle class=\"spark-dot\" cx=\"{cx}\" cy=\"{cy}\" r=\"1.5\"/>"
            );
        } else if seg.len() > 1 {
            let _ = write!(
                svg,
                "<polyline class=\"spark-line\" vector-effect=\"non-scaling-stroke\" points=\"{}\"/>",
                seg.join(" ")
            );
        }
        seg.clear();
    };
    for (i, p) in points.iter().enumerate() {
        match (p.status, p.latency_ms) {
            (Status::Down, _) | (_, None) => flush(&mut segment, &mut svg),
            (_, Some(ms)) => segment.push(format!("{:.1},{:.1}", x(i), y(ms))),
        }
    }
    flush(&mut segment, &mut svg);
    svg.push_str("</svg>");
    svg
}

fn latency_summary(points: &[HistoryPoint]) -> Option<(i64, i64, i64)> {
    let v: Vec<i64> = points
        .iter()
        .filter(|p| p.status != Status::Down)
        .filter_map(|p| p.latency_ms)
        .collect();
    if v.is_empty() {
        return None;
    }
    let min = *v.iter().min()?;
    let max = *v.iter().max()?;
    let avg = v.iter().sum::<i64>() / v.len() as i64;
    Some((min, avg, max))
}

fn monitor_card(out: &mut String, m: &MonitorView, t: &Texts, lang: Language, now: i64) {
    let l = &m.live;
    let (class, label) = status_label(t, l.status);
    let _ = write!(
        out,
        "<article class=\"card\"><div class=\"card-head\"><div class=\"card-title\"><span class=\"dot s-{class}\" aria-hidden=\"true\"></span><h3>{}</h3></div><span class=\"badge s-{class}\">{label}</span></div>",
        esc(&l.name)
    );
    let _ = write!(
        out,
        "<p class=\"target\"><span class=\"kind\">{}</span> {}</p>",
        l.kind,
        esc(&l.target)
    );

    out.push_str("<dl class=\"stats\">");
    for (name, value) in [
        (t.h24, m.uptime.h24),
        (t.d7, m.uptime.d7),
        (t.d30, m.uptime.d30),
    ] {
        let _ = write!(
            out,
            "<div><dt>{} · {name}</dt><dd>{}</dd></div>",
            t.uptime,
            percent(value)
        );
    }
    let last = match l.last_check {
        Some(ts) => format!(
            "<span title=\"{}\">{}</span>",
            esc(&format_time(ts)),
            ago(now - ts, lang)
        ),
        None => t.never.to_owned(),
    };
    let _ = write!(out, "<div><dt>{}</dt><dd>{last}</dd></div>", t.last_check);
    out.push_str("</dl>");

    let summary = latency_summary(&m.history);
    let spark_label = match summary {
        Some((min, avg, max)) => format!(
            "{}: min {min} ms, avg {avg} ms, max {max} ms",
            t.response_time
        ),
        None => format!("{}: {}", t.response_time, t.no_data),
    };
    out.push_str("<div class=\"spark-wrap\">");
    let _ = write!(
        out,
        "<div class=\"spark-head\"><span>{}</span><span>{}</span></div>",
        t.response_time,
        match (l.latency_ms, summary) {
            (Some(ms), Some((_, avg, _))) => format!("{ms} ms · ⌀ {avg} ms"),
            (Some(ms), None) => format!("{ms} ms"),
            _ => t.no_data.to_owned(),
        }
    );
    out.push_str(&sparkline(&m.history, &spark_label));
    out.push_str("</div>");

    let mut detail = l.message.clone().unwrap_or_default();
    if let (Some(days), "tls") = (l.cert_days_left, l.kind) {
        if !detail.contains("days") {
            detail = format!("{detail} ({days} {})", t.cert_days);
        }
    }
    if !detail.is_empty() {
        let _ = write!(out, "<p class=\"detail\">{}</p>", esc(&detail));
    }
    out.push_str("</article>");
}

pub fn render(snap: &StatusSnapshot, lang: Language) -> String {
    let t = texts(lang);
    let now = snap.generated_at;
    let (overall_class, overall_text) = match snap.overall {
        "down" => ("down", t.some_down),
        "warn" => ("warn", t.some_warn),
        "pending" => ("pending", t.pending_overall),
        _ => ("up", t.all_up),
    };

    let mut out = String::with_capacity(16 * 1024);
    let _ = write!(
        out,
        "<!DOCTYPE html>\n<html lang=\"{lang}\"><head><meta charset=\"utf-8\">\
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
<meta http-equiv=\"refresh\" content=\"30\">\
<meta name=\"color-scheme\" content=\"light dark\">\
<title>{title}</title>\
<link rel=\"icon\" href=\"/favicon.svg\" type=\"image/svg+xml\">\
<link rel=\"stylesheet\" href=\"/assets/style.css\">\
<script src=\"/assets/app.js\" defer></script>\
</head><body><main>",
        lang = t.lang_code,
        title = esc(&snap.title)
    );
    let _ = write!(
        out,
        "<header class=\"top\"><h1>{}</h1><div class=\"banner s-{overall_class}\" role=\"status\"><span class=\"dot s-{overall_class}\" aria-hidden=\"true\"></span>{overall_text}</div></header>",
        esc(&snap.title)
    );

    out.push_str("<section class=\"grid\">");
    for m in &snap.monitors {
        monitor_card(&mut out, m, t, lang, now);
    }
    out.push_str("</section>");

    let _ = write!(
        out,
        "<section class=\"incidents\"><h2>{}</h2>",
        t.recent_incidents
    );
    if snap.incidents.is_empty() {
        let _ = write!(out, "<p class=\"empty\">{}</p>", t.no_incidents);
    } else {
        let _ = write!(
            out,
            "<div class=\"table-wrap\"><table><thead><tr><th>{}</th><th>{}</th><th>{}</th><th>{}</th></tr></thead><tbody>",
            t.monitor, t.started, t.duration, t.n_detail
        );
        for i in &snap.incidents {
            let duration = if i.ongoing {
                format!(
                    "<span class=\"badge s-down\">{}</span> {}",
                    t.ongoing,
                    format_duration(i.duration_secs, lang)
                )
            } else {
                format_duration(i.duration_secs, lang)
            };
            let _ = write!(
                out,
                "<tr><td>{}</td><td>{}</td><td>{duration}</td><td class=\"msg\">{}</td></tr>",
                esc(&i.monitor),
                time_tag(i.started_at),
                esc(&i.message)
            );
        }
        out.push_str("</tbody></table></div>");
    }
    out.push_str("</section>");

    let _ = writeln!(
        out,
        "<footer>{} {} · {} <a href=\"https://github.com/hytztrk-ux/watchpost\" rel=\"noopener\">watchpost</a> v{}</footer></main></body></html>",
        t.updated,
        time_tag(now),
        t.powered_by,
        env!("CARGO_PKG_VERSION")
    );
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escapes_html() {
        assert_eq!(
            esc("<a href=\"x\">&'"),
            "&lt;a href=&quot;x&quot;&gt;&amp;&#39;"
        );
    }

    #[test]
    fn percent_formatting() {
        assert_eq!(percent(None), "–");
        assert_eq!(percent(Some(1.0)), "100%");
        assert_eq!(percent(Some(0.99999)), "100%");
        assert_eq!(percent(Some(0.9999)), "99.99%");
        assert_eq!(percent(Some(0.5)), "50.00%");
        assert_eq!(percent(Some(0.0)), "0.00%");
    }

    #[test]
    fn sparkline_marks_down_checks() {
        let pts = vec![
            HistoryPoint {
                ts: 1,
                status: Status::Up,
                latency_ms: Some(10),
            },
            HistoryPoint {
                ts: 2,
                status: Status::Up,
                latency_ms: Some(20),
            },
            HistoryPoint {
                ts: 3,
                status: Status::Down,
                latency_ms: Some(1),
            },
            HistoryPoint {
                ts: 4,
                status: Status::Up,
                latency_ms: Some(5),
            },
        ];
        let svg = sparkline(&pts, "rt");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("spark-down"));
        assert!(svg.contains("<polyline"));
        assert!(svg.contains("<circle"), "single point after the gap");
        assert!(sparkline(&[], "x").is_empty());
    }
}
