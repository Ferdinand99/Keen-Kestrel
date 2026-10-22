# Keen Kestrel – glossary (Turkish, `tr`)

Draft: not yet reviewed by native speakers.

Kestrel's own strings are written from English with these words. Core strings shared with Vigilant Vole come from
vole-kit and follow [Vigilant Vole's glossary](https://github.com/Ferdinand99/Vigilant-Vole/blob/main/locales/glossary/tr.md).

## Style
- Address: neutral polite imperative (Girin, Seçin, Ekleyin); never "sen" forms. Buttons use the short imperative
  (Kaydet, Sil, İptal).
- Quotation marks: “ ” and ’ as in English. Ellipsis … kept where English has it.
- Sentence case. Respect İ/ı. Month and weekday names are capitalised (Ekim, Salı).
- Suffixes are never attached to `{placeholders}`; sentences are reshaped ("Düzenle: {name}", "“{name}” silinsin mi?").
- Numbers: nouns stay singular after numerals (`{n} sorun`), so `one` and `other` are identical.
- Kept in English: DSN, dSYM, SDK, `sentry-cli`, symbolication, JSON, UTM, webhook, and other product, protocol and
  technical names.

## Terms

| English | Türkçe | Note |
|---|---|---|
| issue | sorun | as in Crashlytics |
| error | hata | VV: “Hata” |
| event | etkinlik | VV: “Etkinlikler” (as in Google Analytics); “olay” stays reserved for incidents, as in VV |
| release | sürüm | same word as app version: in Kestrel a release *is* the app version |
| app version | sürüm | VV |
| environment | ortam | |
| resolve / resolved | çözüldü olarak işaretle / çözüldü | VV: “Çözüldü”; button short form “Çözüldü” |
| resolve in next release | sonraki sürümde çöz | |
| unresolved | çözülmemiş | filter tab “Çözülmemiş” |
| ignore / ignored | yok say / yok sayıldı | VV: “Yok sayılır” |
| merge (issues) | birleştir | VV: “Birleştir” |
| regression / regressed | gerileme / yeniden açıldı | pill “Yeniden açıldı” |
| new / ongoing | yeni / devam ediyor | issue substatus |
| crash | çökme | verb: çökmek; Google uses “kilitlenme”, which also suggests a freeze |
| crash-free sessions / users | çökmesiz oturumlar / çökmesiz kullanıcılar | short label “Çökmesiz” |
| release health | kararlılık | tab “Kararlılık”, heading “Sürüm kararlılığı” |
| stack trace | yığın izi | |
| frame / in-app frame | çerçeve / uygulama çerçevesi | toggle “Yalnızca uygulama kodu” |
| breadcrumbs | iz kaydı | help text: “hatadan önceki adımlar” |
| grouping | gruplama | “Neden gruplandı” |
| fingerprint | parmak izi | the SDK field `fingerprint` stays as code |
| debug files | hata ayıklama dosyaları | |
| symbolicated / unsymbolicated | sembollü / sembolsüz | status labels; the noun stays “symbolication” |
| project | proje | |
| key (project key) | anahtar | “proje anahtarı” where it must be specific |
| alert / alert rule | uyarı / uyarı kuralı | VV: “Uyarılar” |
| spike | ani artış | alert type |
| first seen / last seen | ilk görülme / son görülme | |
| user (of the app) | kullanıcı | same word as account users in VV; context separates them |
| active users (daily / weekly / monthly) | aktif kullanıcılar (günlük / haftalık / aylık) | avoid DAU/WAU/MAU in the UI |
| signal | sinyal | analytics event from an app |
| pageview | sayfa görüntüleme | VV; short “Görüntüleme” |
| visitor / unique visitors | ziyaretçi / tekil ziyaretçi | VV (singular after numbers and in headings) |
| referrer | yönlendiren | VV: “En çok yönlendirenler” |
| session | oturum | VV: “Oturumlar” |
| retention (data) | saklama | VV: “… saklama süresi”; never for cohort retention (not in v1) |
| sample / sampling | örnek / örnekleme | sample rate: “örnekleme oranı” |
| tag | etiket | VV: “Etiketler” |
| analytics | analiz | VV: “Analiz” |
| DSN, dSYM, SDK, `sentry-cli` | unchanged | see README; suffixes with an apostrophe only if unavoidable (“SDK’yı”), better rephrase |
| symbolication | symbolication | unchanged noun; prefer “sembollü” phrasing in sentences |
| notification channel / channel | bildirim kanalı / kanal | VV: “bildirim kanalı”; short “Kanal” in tables and buttons |
| digest | özet | “Özet olarak gönder”; status pill “Özette” |
| cooldown (at most every … per issue) | “sorun başına en sık … aralıkla” | label “Sorun başına en sık şu aralıkla”, then the duration |
| threshold / alert above | eşik / uyarı eşiği | form label “Şundan fazla”, addon “etkinlik / saat” |
| multiplier (× the usual) | normalin … katı | |
| adoption | benimsenme | the release's share of sessions in the last 7 days |
| errored (session) | hatalı, çökme yok | |
| abnormal (session) | anormal sonlandı | |
| finalized (release) | tamamlandı | lowercase status after a date |
| culprit (alert message field) | hata kaynağı | as in the issue search help |
| attempt (delivery) | deneme | “{n} deneme” |
| header (webhook) | başlık | VV kit: “Özel başlıklar” |
| instance | örnek | as in VV; “bu örneğin dili” = the instance's language |
