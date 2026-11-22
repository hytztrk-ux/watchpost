//! English and Turkish texts for the status page and notifications.

use crate::config::Language;

pub struct Texts {
    pub lang_code: &'static str,
    // Status page.
    pub all_up: &'static str,
    pub some_down: &'static str,
    pub some_warn: &'static str,
    pub pending_overall: &'static str,
    pub up: &'static str,
    pub down: &'static str,
    pub warn: &'static str,
    pub pending: &'static str,
    pub uptime: &'static str,
    pub h24: &'static str,
    pub d7: &'static str,
    pub d30: &'static str,
    pub response_time: &'static str,
    pub last_check: &'static str,
    pub never: &'static str,
    pub recent_incidents: &'static str,
    pub no_incidents: &'static str,
    pub ongoing: &'static str,
    pub started: &'static str,
    pub duration: &'static str,
    pub monitor: &'static str,
    pub updated: &'static str,
    pub no_data: &'static str,
    pub cert_days: &'static str,
    pub powered_by: &'static str,
    // Notifications.
    pub n_down: &'static str,
    pub n_recovered: &'static str,
    pub n_cert: &'static str,
    pub n_still_down: &'static str,
    pub n_still_cert: &'static str,
    pub n_test: &'static str,
    pub n_test_body: &'static str,
    pub n_target: &'static str,
    pub n_detail: &'static str,
    pub n_time: &'static str,
    pub n_down_for: &'static str,
    pub n_status_page: &'static str,
    // Duration units: day, hour, minute, second.
    pub units: [&'static str; 4],
}

pub const EN: Texts = Texts {
    lang_code: "en",
    all_up: "All systems operational",
    some_down: "Some systems are down",
    some_warn: "Some systems need attention",
    pending_overall: "Waiting for the first checks",
    up: "Up",
    down: "Down",
    warn: "Warning",
    pending: "Pending",
    uptime: "Uptime",
    h24: "24 hours",
    d7: "7 days",
    d30: "30 days",
    response_time: "Response time",
    last_check: "Last check",
    never: "never",
    recent_incidents: "Recent incidents",
    no_incidents: "No incidents recorded.",
    ongoing: "Ongoing",
    started: "Started",
    duration: "Duration",
    monitor: "Monitor",
    updated: "Updated",
    no_data: "no data",
    cert_days: "days left",
    powered_by: "Powered by",
    n_down: "DOWN",
    n_recovered: "RECOVERED",
    n_cert: "CERTIFICATE EXPIRING",
    n_still_down: "STILL DOWN",
    n_still_cert: "CERTIFICATE STILL EXPIRING",
    n_test: "TEST",
    n_test_body:
        "This is a test notification from watchpost. If you can read this, the channel works.",
    n_target: "Target",
    n_detail: "Detail",
    n_time: "Time",
    n_down_for: "Down for",
    n_status_page: "Status page",
    units: ["d", "h", "m", "s"],
};

pub const TR: Texts = Texts {
    lang_code: "tr",
    all_up: "Tüm sistemler çalışıyor",
    some_down: "Bazı sistemlere erişilemiyor",
    some_warn: "Bazı sistemler dikkat gerektiriyor",
    pending_overall: "İlk kontroller bekleniyor",
    up: "Çalışıyor",
    down: "Erişilemiyor",
    warn: "Uyarı",
    pending: "Bekleniyor",
    uptime: "Erişilebilirlik",
    h24: "24 saat",
    d7: "7 gün",
    d30: "30 gün",
    response_time: "Yanıt süresi",
    last_check: "Son kontrol",
    never: "henüz yok",
    recent_incidents: "Son kesintiler",
    no_incidents: "Kayıtlı kesinti yok.",
    ongoing: "Devam ediyor",
    started: "Başlangıç",
    duration: "Süre",
    monitor: "İzleyici",
    updated: "Güncellendi",
    no_data: "veri yok",
    cert_days: "gün kaldı",
    powered_by: "Altyapı:",
    n_down: "ERİŞİLEMİYOR",
    n_recovered: "DÜZELDİ",
    n_cert: "SERTİFİKA SÜRESİ DOLUYOR",
    n_still_down: "HÂLÂ ERİŞİLEMİYOR",
    n_still_cert: "SERTİFİKA SÜRESİ HÂLÂ DOLUYOR",
    n_test: "TEST",
    n_test_body:
        "Bu, watchpost'tan gönderilen bir test bildirimidir. Bu mesajı görüyorsanız kanal çalışıyor.",
    n_target: "Hedef",
    n_detail: "Ayrıntı",
    n_time: "Zaman",
    n_down_for: "Kesinti süresi",
    n_status_page: "Durum sayfası",
    units: ["g", "sa", "dk", "sn"],
};

pub fn texts(lang: Language) -> &'static Texts {
    match lang {
        Language::En => &EN,
        Language::Tr => &TR,
    }
}

/// Compact duration such as `2h 5m` / `2sa 5dk`, using the two largest units.
pub fn format_duration(secs: i64, lang: Language) -> String {
    let units = texts(lang).units;
    let secs = secs.max(0);
    let parts = [
        secs / 86_400,
        (secs % 86_400) / 3600,
        (secs % 3600) / 60,
        secs % 60,
    ];
    let first = parts.iter().position(|&v| v > 0).unwrap_or(3);
    let mut out = Vec::new();
    for i in first..(first + 2).min(4) {
        if parts[i] > 0 || i == first {
            out.push(format!("{}{}", parts[i], units[i]));
        }
    }
    out.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(format_duration(0, Language::En), "0s");
        assert_eq!(format_duration(42, Language::En), "42s");
        assert_eq!(format_duration(125, Language::En), "2m 5s");
        assert_eq!(format_duration(3600, Language::En), "1h");
        assert_eq!(format_duration(90_061, Language::En), "1d 1h");
        assert_eq!(format_duration(7500, Language::Tr), "2sa 5dk");
        assert_eq!(format_duration(-5, Language::En), "0s");
    }
}
