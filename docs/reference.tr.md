# watchpost başvuru belgesi

[English](reference.md) | **Türkçe** · [README'ye dön](../README.tr.md)

- [Komutlar](#komutlar)
- [Yapılandırma başvurusu](#yapılandırma-başvurusu)
- [Bildirim kurulumu](#bildirim-kurulumu)
- [Durum sayfası ve API](#durum-sayfası-ve-api)
- [systemd servisi olarak çalıştırma](#systemd-servisi-olarak-çalıştırma)
- [Docker ile çalıştırma](#docker-ile-çalıştırma)

## Komutlar

| Komut | Ne yapar |
|---|---|
| `watchpost init [--force]` | Açıklamalı örnek yapılandırmayı `watchpost.toml` dosyasına (ya da `--config` ile verilen yola) yazar. |
| `watchpost check [--config-only]` | Yapılandırmayı doğrular, ardından her izleyiciyi bir kez çalıştırıp sonuçları tablo olarak yazar. Erişilemeyen yoksa çıkış kodu `0`, varsa `1`, yapılandırma hatasında `2` olur. |
| `watchpost run` | Zamanlayıcıyı, bildirimleri ve durum sayfasını, durdurulana kadar çalıştırır. |
| `watchpost test-notify [AD]` | Tüm bildirim kanallarına ya da yalnızca `AD` adlı kanala bir test mesajı gönderir. |

Genel seçenekler: `-c, --config <YOL>` (varsayılan `watchpost.toml` ya da
`WATCHPOST_CONFIG` ortam değişkeni), `--log-level error|warn|info|debug|trace`
(verilmezse `RUST_LOG` kullanılır; o da yoksa `run` için `info`) ve
`--log-format text|json`.

## Yapılandırma başvurusu

Eksiksiz ve açıklamalı bir örnek [`watchpost.example.toml`](../watchpost.example.toml)
dosyasındadır. `watchpost init` de aynı dosyayı yazar.

Süreler `"30s"`, `"5m"`, `"1h"` veya `"1h 30m"` gibi metinlerle yazılır. Düz
bir tam sayı saniye anlamına gelir. Bilinmeyen anahtarlar reddedilir, böylece
yazım hataları erkenden yakalanır. `watchpost check` tüm sorunları tek seferde
listeler.

### Yapılandırmada ortam değişkenleri

Herhangi bir metin değerinde `${AD}` yazılabilir. Bu ifade, `AD` ortam
değişkeninin değeriyle değiştirilir. Değişken tanımlı değilse yapılandırma
yüklenmez ve hata mesajında değişkenin adı geçer. Diğer biçimler:

- `${AD:-varsayılan}`: `AD` tanımlı değilse ya da boşsa `varsayılan` kullanılır.
- `$${`: doğrudan `${` yazar.

Değiştirme yalnızca değerlerde yapılır, yorum satırlarına dokunulmaz.

```toml
bot_token = "${TELEGRAM_BOT_TOKEN}"
listen    = "${WATCHPOST_LISTEN:-127.0.0.1:8787}"
```

### En üst düzey

| Anahtar | Varsayılan | Açıklama |
|---|---|---|
| `language` | `"en"` | `"en"` veya `"tr"`. Durum sayfası ve bildirimler bu dilde olur. |
| `title` | `"watchpost"` | Durum sayfasında ve bildirim konularında görünür. |

### `[web]`

| Anahtar | Varsayılan | Açıklama |
|---|---|---|
| `enabled` | `true` | Web sunucusu olmadan çalıştırmak için `false` yapın. |
| `listen` | `"127.0.0.1:8787"` | Adres ve port. Tüm arayüzleri dinlemek için `"0.0.0.0:8787"` yazın. |
| `public_url` | – | Verilirse bildirimlere durum sayfasının bağlantısı eklenir. |

### `[web.auth]` (isteğe bağlı)

| Anahtar | Açıklama |
|---|---|
| `username` | Kullanıcı adı. |
| `password` | Parola. Burada `${ENV}` kullanın. |

Bu bölüm tanımlıysa sayfa, statik dosyalar ve `/api/status` kimlik doğrulama
ister. `/healthz` her zaman açıktır. Basic auth parolayı her istekte
gönderir. Sayfaya dışarıdan erişilebiliyorsa onu yalnızca HTTPS arkasında
(bir ters vekil sunucuyla) kullanın.

### `[storage]`

| Anahtar | Varsayılan | Açıklama |
|---|---|---|
| `path` | `"watchpost.db"` | SQLite dosyası. Göreli bir yol, yapılandırma dosyasının bulunduğu dizine göre çözülür. Eksik üst dizinler oluşturulur. |
| `retention_days` | `30` | Bundan eski kontrol sonuçları ve kapanmış kesintiler saatte bir silinir. Saatlik erişilebilirlik sayaçları en az 31 gün saklanır, böylece saklama süresi kısa olsa da 30 günlük oran hesaplanabilir. |

### `[defaults]`

Tüm izleyiciler için varsayılanlardır. Her izleyici bunları kendi değerleriyle
geçersiz kılabilir.

| Anahtar | Varsayılan | Açıklama |
|---|---|---|
| `interval` | `"60s"` | İki kontrol arasındaki süre (en az `1s`). |
| `timeout` | `"10s"` | Tek bir kontrol için süre sınırı. |
| `retries` | `1` | İzleyici "erişilemiyor" sayılmadan önce kaç ardışık başarısızlığa daha göz yumulacağı. `0` ise ilk başarısızlık yeterlidir. `1` ise art arda iki kez başarısız olması gerekir. |
| `repeat_reminder` | `"0s"` (kapalı) | İzleyiciye erişilemediği (ya da sertifikanın süresi dolmak üzere olduğu) sürece bu aralıkla hatırlatma gönderilir. En az `1m`. |

### `[[monitor]]`: tüm türlerde ortak anahtarlar

| Anahtar | Zorunlu | Açıklama |
|---|---|---|
| `name` | evet | Benzersiz ad. Geçmiş bu adla saklanır; ad değiştirilirse yeni bir geçmiş başlar. |
| `type` | evet | `"http"`, `"tcp"` veya `"tls"`. |
| `interval`, `timeout`, `retries`, `repeat_reminder` | hayır | `[defaults]` değerlerini geçersiz kılar. |
| `notify` | hayır | Bildirim kanalı adlarının listesi. Yazılmazsa tüm kanallar kullanılır. `notify = []` bu izleyicinin bildirimlerini kapatır. |

### `type = "http"`

| Anahtar | Varsayılan | Açıklama |
|---|---|---|
| `url` | – | `http://` veya `https://` adresi. |
| `method` | `"GET"` | Herhangi bir HTTP metodu. |
| `expected_status` | `["2xx"]` | Kabul edilen durum kodları. Kod (`200`), sınıf (`"3xx"`) veya aralık (`"200-204"`) olarak yazılabilir. |
| `keyword` | – | Yanıt gövdesi bu metni içermelidir. Gövdenin ilk 1 MiB'ı aranır. |
| `headers` | `{}` | Ek istek başlıkları, ör. `{ Authorization = "Bearer ${TOKEN}" }`. |
| `body` | – | İstek gövdesi (`POST`/`PUT` için). |
| `follow_redirects` | `true` | En fazla 10 yönlendirme izlenir. `false` ise `3xx` yanıtı `expected_status` ile karşılaştırılır. |
| `skip_tls_verify` | `false` | Geçersiz veya kendinden imzalı sertifikaları kabul eder. |

Her kontrol yeni bir bağlantı açar. Bu yüzden ölçülen süre, yeni bir
ziyaretçinin göreceği gibi DNS, TCP ve TLS kurulumunu da içerir. HTTP
izleyicileri standart `HTTP_PROXY` / `HTTPS_PROXY` / `NO_PROXY` ortam
değişkenlerine uyar.

### `type = "tcp"`

| Anahtar | Açıklama |
|---|---|
| `host` | Sunucu adı veya IP adresi. |
| `port` | Port. Bir TCP bağlantısı açılabiliyorsa kontrol başarılı sayılır. |

### `type = "tls"`

| Anahtar | Varsayılan | Açıklama |
|---|---|---|
| `host` | – | Bağlanılacak sunucu. |
| `port` | `443` | Port. |
| `server_name` | `host` | SNI olarak gönderilen ve sertifikada aranan ad. |
| `warn_days` | `14` | Kalan gün sayısı bunun altına düşünce uyarı verir (`warn` durumu ve "sertifika süresi doluyor" bildirimi). |
| `skip_verify` | `false` | Yalnızca son kullanma tarihine bakar; sertifika zincirini ve adı doğrulamaz. |
| `ca_file` | – | Ek güvenilir CA sertifikalarını içeren PEM dosyası (kurum içi CA'lar için). |

Sonuçlar: sertifika en az `warn_days` gün daha geçerliyse `up`, daha erken
doluyorsa `warn`, süresi dolmuşsa, güvenilir değilse (`skip_verify`
kapalıyken) ya da el sıkışma başarısız olursa `down` olur. Sertifikalar sık
değişmediği için `"6h"` gibi bir aralık yeterlidir. Yerleşik güven deposu
Mozilla kök sertifika listesidir (`webpki-roots`); işletim sistemine bağlı
değildir.

### `[[notifier]]`

| Anahtar | Zorunlu | Açıklama |
|---|---|---|
| `type` | evet | `"telegram"`, `"discord"`, `"webhook"` veya `"email"`. |
| `name` | hayır | `notify = [...]` içinde kullanılan benzersiz ad. Varsayılanı türün adıdır. |

Türe özel anahtarlar bir sonraki bölümde.

## Bildirim kurulumu

Bildirimler durum değiştiğinde gönderilir:

| Olay | Ne zaman |
|---|---|
| `down` | Art arda `retries + 1` başarısız kontrolden sonra. |
| `recovered` | Kesintiden sonraki ilk başarılı kontrolde. Kesintinin ne kadar sürdüğünü de içerir. |
| `cert_expiring` | Bir TLS izleyicisi uyarı aralığına girdiğinde. |
| `reminder` | Yalnızca `repeat_reminder` ayarlıysa: kesinti ya da sertifika sorunu hâlâ sürüyorsa. |

Program başladıktan sonraki ilk başarılı kontrol sessizdir. Baştan beri
erişilemeyen bir izleyici ise bildirim gönderir. Yeniden başlatmadan sonra hâlâ
erişilemeyen bir izleyici tekrar duyurulmaz (açık kesinti veritabanında
saklanır). Gönderim arka planda yapılır ve iki kez daha denenir (2 sn ve 10 sn
sonra). Yine başarısız olursa günlüğe yazılır.

Bir kanalı ayarladıktan sonra `watchpost test-notify` çalıştırın.

### Telegram

1. [@BotFather](https://t.me/BotFather) ile konuşun, `/newbot` gönderin ve
   size verilen token'ı kopyalayın.
2. Yeni botunuza herhangi bir mesaj gönderin (ya da onu bir gruba ekleyin).
3. Sohbet kimliğini (chat id) bulun:
   `https://api.telegram.org/bot<TOKEN>/getUpdates` adresini açın ve
   `"chat":{"id":...}` kısmına bakın. Grup kimlikleri eksi işaretlidir.

```toml
[[notifier]]
name = "telegram"
type = "telegram"
bot_token = "${TELEGRAM_BOT_TOKEN}"
chat_id = "123456789"
# api_url = "https://api.telegram.org"   # kendi Bot API sunucunuz varsa
```

Mesajlar düz metin olarak gönderilir; böylece özel karakter içeren adlar
biçimlendirmeyi bozamaz.

### Discord

Kanal ayarlarında **Entegrasyonlar → Webhook'lar → Yeni Webhook** yolunu
izleyin ve adresi kopyalayın.

```toml
[[notifier]]
name = "discord"
type = "discord"
webhook_url = "${DISCORD_WEBHOOK_URL}"
# username = "watchpost"
```

### Genel webhook

watchpost adresinize JSON içeren bir `POST` isteği gönderir. `2xx` dönen her yanıt
başarılı sayılır.

```toml
[[notifier]]
name = "webhook"
type = "webhook"
url = "https://hooks.example.com/watchpost"
headers = { Authorization = "Bearer ${WEBHOOK_TOKEN}" }
```

Gövde alanları (yerel bir test alıcısına gelen gerçek bir örnek için
İngilizce belgedeki [Generic webhook](reference.md#generic-webhook) bölümüne bakın):
`event` (`down`, `recovered`, `cert_expiring`, `reminder`, `test`), `status`
(`up`, `warn`, `down`), `monitor`, `type`, `target`, `message`, `timestamp`
(RFC 3339), `unix_time`, `down_since`, `duration_seconds`, `title`,
`subject`, `text` (hazır, okunabilir mesaj) ve `status_page`. Uygun olmayan
alanlar `null` gelir.

### E-posta (SMTP)

```toml
[[notifier]]
name = "email"
type = "email"
host = "smtp.example.com"
tls = "starttls"          # "starttls" (varsayılan, port 587), "tls" (port 465) veya "none" (port 25)
# port = 587              # varsayılanı tls kipine göre belirlenir
username = "alerts@example.com"
password = "${SMTP_PASSWORD}"
from = "watchpost <alerts@example.com>"
to = ["ops@example.com", "oncall@example.com"]
```

`starttls` kipinde sunucunun STARTTLS desteklemesi zorunludur; düz metne geri
dönülmez. `"none"` kipini yalnızca aynı makinedeki ya da güvenilir bir ağdaki
bir aktarıcı için kullanın. E-postalar düz metindir.

## Durum sayfası ve API

- `GET /`: durum sayfası (HTML). 30 saniyede bir kendini yeniler ve
  JavaScript olmadan da çalışır. Gömülü küçük bir betik yalnızca zamanları
  ziyaretçinin yerel saatine çevirir.
- `GET /api/status`: aynı verilerin JSON hâli. `overall` alanı `up`, `warn`,
  `down` veya `pending` (henüz hiçbir kontrol bitmedi) olur. Zamanlar Unix
  saniyesi olarak verilir. Erişilebilirlik oranları 0 ile 1 arasındadır; veri
  yoksa `null` olur. `history` alanı son 60 kontrolü içerir. Örnek çıktı
  [İngilizce belgede](reference.md#status-page-and-api).
- `GET /healthz`: süreç çalıştığı sürece `200 ok` döner; hiçbir zaman parola
  istemez.

**Erişilebilirlik nasıl hesaplanır:** `down` olmayan kontrollerin oranıdır
(`warn` çalışıyor sayılır). Hesap saatlik sayaçlarla yapılır, bu yüzden
"24 saat" penceresi içinde bulunulan saati ve ondan önceki 23 saati kapsar.

## systemd servisi olarak çalıştırma

Korumalı (sandbox) ayarlarla hazırlanmış örnek bir birim dosyası projeyle
gelir: [`watchpost.service`](../watchpost.service).

```sh
sudo install -m 0755 watchpost /usr/local/bin/watchpost
sudo mkdir -p /etc/watchpost
sudo watchpost --config /etc/watchpost/watchpost.toml init
sudo editor /etc/watchpost/watchpost.toml        # [storage] path = "/var/lib/watchpost/watchpost.db" yapın
# gizli bilgiler, her satıra bir tane, ör. TELEGRAM_BOT_TOKEN=123:abc
sudo install -m 0600 /dev/null /etc/watchpost/watchpost.env && sudo editor /etc/watchpost/watchpost.env
sudo install -m 0644 watchpost.service /etc/systemd/system/watchpost.service
sudo systemctl daemon-reload
sudo systemctl enable --now watchpost
journalctl -u watchpost -f
```

Birim dosyası `DynamicUser=yes` ve `StateDirectory=watchpost` kullanır. Böylece
watchpost, yalnızca `/var/lib/watchpost` dizinine yazabilen, yetkisiz ve geçici bir
kullanıcıyla çalışır. Yapılandırmayı değiştirdikten sonra önce
`watchpost --config /etc/watchpost/watchpost.toml check`, ardından
`sudo systemctl restart watchpost` çalıştırın.

## Docker ile çalıştırma

[`Dockerfile`](../Dockerfile) statik bir program derler ve onu boş (`scratch`)
bir imaja kopyalar. İmaj UID 65532 ile çalışır ve `/data/watchpost.toml`
dosyasını okur.

```sh
docker build -t watchpost .

mkdir -p watchpost-data
docker run --rm -v "$PWD/watchpost-data:/data" watchpost --config /data/watchpost.toml init
# Konteyner içinde tüm arayüzleri dinleyin:
#   [web]
#   listen = "0.0.0.0:8787"
sudo chown -R 65532:65532 watchpost-data     # konteyner kullanıcısı watchpost.db'yi yazabilmeli

docker run -d --name watchpost --restart unless-stopped \
  -p 127.0.0.1:8787:8787 \
  -v "$PWD/watchpost-data:/data" \
  -e TELEGRAM_BOT_TOKEN=123:abc \
  watchpost
```

İpucu: yapılandırmada `listen = "${WATCHPOST_LISTEN:-127.0.0.1:8787}"` yazıp
konteyneri `-e WATCHPOST_LISTEN=0.0.0.0:8787` ile başlatırsanız aynı dosya hem
konteyner içinde hem dışında çalışır. İmajda kabuk ya da curl yoktur; sağlık
kontrolünü dışarıdan yapın, ör. `curl -f http://127.0.0.1:8787/healthz`.
